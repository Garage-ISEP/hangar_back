use crate::model::project::ProjectMetrics;
use crate::sse::types::{ContainerStatus, ContainerStatusEvent, DeploymentEvent, DeploymentStage, MetricsEvent, SseEvent};
use crate::state::AppState;

pub async fn emit_creation_deployment_stage(
    state: &AppState,
    user_login: &str,
    project_name: String,
    stage: DeploymentStage,
    project_id: Option<i32>
)
{
    let event = SseEvent::Deployment(DeploymentEvent::new(
        project_id.unwrap_or(0),
        project_name,
        stage,
    ));
    
    state.sse_manager.emit_to_creation(user_login, event.clone()).await;
    if let Some(id) = project_id 
    {
        state.sse_manager.emit_to_project(id, event).await;
    }
}

pub async fn emit_deployment_stage(
    state: &AppState,
    project_id: i32,
    project_name: String,
    stage: DeploymentStage,
)
{
    let event = SseEvent::Deployment(DeploymentEvent::new(
        project_id,
        project_name,
        stage,
    ));
    
    state.sse_manager.emit_to_project(project_id, event).await;
}


pub async fn emit_container_status(
    state: &AppState,
    project_id: i32,
    project_name: String,
    container_name: String,
    status: ContainerStatus,
)
{
    let event = SseEvent::ContainerStatus(ContainerStatusEvent::new(
        project_id,
        project_name,
        container_name,
        status,
    ));
    
    state.sse_manager.emit_to_project(project_id, event).await;
}

pub async fn emit_metrics(
    state: &AppState,
    project_id: i32,
    project_name: String,
    metrics: ProjectMetrics,
)
{
    let event = SseEvent::Metrics(MetricsEvent::new(
        project_id,
        project_name,
        metrics,
    ));
    
    state.sse_manager.emit_to_project(project_id, event).await;
}