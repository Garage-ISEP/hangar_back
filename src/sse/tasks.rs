use std::collections::HashMap;
use std::time::Duration;

use bollard::query_parameters::EventsOptions;
use tokio::time::{interval, sleep};
use tokio_stream::StreamExt;
use tracing::{info, warn};
use tracing::{debug, error};

use crate::sse::emitter::{emit_container_crashed_to_admin, emit_container_status};
use crate::sse::emitter::emit_metrics;
use crate::sse::types::ContainerStatus;
use crate::{services::project_service, state::AppState};
use crate::services::docker_service;

const EMIT_METRICS_INTERVAL_SECS: u64 = 5;

pub async fn start_docker_events_listener(state: AppState, mut shutdown_signal: tokio::sync::broadcast::Receiver<()>)
{
    info!("Starting Docker events listener task");
    
    let docker = state.docker_client.clone();

    let filters = HashMap::from([("label".to_string(), vec![format!("app={}", state.config.app_prefix)])]);
    
    let options = Some(EventsOptions 
    {
        filters: Some(filters),
        ..Default::default()
    });

    loop
    {
        let mut stream = docker.events(options.clone());

        loop 
        {
            tokio::select! 
            {
                _ = shutdown_signal.recv() => 
                {
                    info!("Shutdown signal received, stopping Docker events listener");
                    return;
                }
                event_result = stream.next() => 
                {
                    match event_result 
                    {
                        Some(Ok(event)) => 
                        {
                            handle_docker_event(&state, event).await;
                        }
                        Some(Err(e)) => 
                        {
                            error!("Docker events stream error: {}. Reconnecting in 5s...", e);
                            break;
                        }
                        None => 
                        {
                            warn!("Docker events stream ended. Reconnecting in 5s...");
                            break;
                        }
                    }
                }
            }
        }
        
        if shutdown_signal.try_recv().is_ok() 
        {
            info!("Shutdown signal received during reconnection wait");
            return;
        }
        
        sleep(Duration::from_secs(5)).await;
    }
}

async fn handle_docker_event(state: &AppState, event: bollard::models::EventMessage)
{
    let action = match event.action.as_deref() 
    {
        Some("create") => ContainerStatus::Created,
        Some("restart") => ContainerStatus::Restarting,
        Some("start" | "unpause") => ContainerStatus::Running,
        Some("stop" | "die") => ContainerStatus::Exited,
        Some("kill" | "oom") => ContainerStatus::Dead,
        Some("pause") => ContainerStatus::Paused,
        _ => return,
    };

    if let Some(actor) = event.actor 
    {
        let container_name = actor.attributes
            .and_then(|attrs| attrs.get("name").cloned())
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_default();

        if container_name.is_empty() { return; }
        
        if let Ok(Some(project)) = project_service::get_project_by_container_name(&state.db_pool, &container_name).await
        {
            debug!("Container '{}' changed status to {:?}", container_name, action);
            
            emit_container_status(
                state,
                project.id,
                project.name.clone(),
                container_name.clone(),
                action.clone(),
            ).await;
            
            if action == ContainerStatus::Dead 
            {
                emit_container_crashed_to_admin(
                    state,
                    project.id,
                    project.name,
                    container_name,
                );
            }
        }
    }
}

/// Lance une tâche qui collecte périodiquement les métriques des containers
/// et les émet via SSE
pub async fn start_metrics_collector(state: AppState, mut shutdown_signal: tokio::sync::broadcast::Receiver<()>)
{
    let mut interval = interval(Duration::from_secs(EMIT_METRICS_INTERVAL_SECS));
    
    info!("Starting metrics collector task");
    
    loop
    {
        tokio::select! 
        {
            _ = shutdown_signal.recv() => 
            {
                info!("Metrics collector task shutting down");
                break;
            }
            _ = interval.tick() => {}
        }
        
        if let Err(e) = collect_all_metrics(&state).await
        {
            error!("Error in metrics collector: {}", e);
        }
    }
}

async fn collect_all_metrics(state: &AppState) -> Result<(), Box<dyn std::error::Error>>
{
    let active_ids = state.sse_manager.get_active_project_ids().await;

    if active_ids.is_empty() 
    {
        return Ok(());
    }

    let projects = project_service::get_projects_by_ids(&state.db_pool, &active_ids).await?;
    
    for project in projects
    {        
        match docker_service::get_container_metrics(&state.docker_client, &project.container_name).await
        {
            Ok(metrics) =>
            {
                emit_metrics(
                    state,
                    project.id,
                    project.name.clone(),
                    metrics,
                ).await;
            }
            Err(e) =>
            {
                debug!("Could not get metrics for container '{}': {}", project.container_name, e);
            }
        }
    }
    
    Ok(())
}