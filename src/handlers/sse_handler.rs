use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::{debug, error, warn};

use crate::error::AppError;
use crate::services::jwt::Claims;
use crate::services::project_service;
use crate::state::AppState;
use crate::sse::types::{SseEvent, SystemEvent, SystemEventLevel};

/// Handler SSE pour le canal admin
///
/// Accessible uniquement aux admins. Reçoit les événements admin uniquement.
/// Endpoint: GET /api/sse/admin
pub async fn sse_admin_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError>
{
    if !claims.is_admin
    {
        return Err(AppError::Unauthorized(
            "Admin privileges required for admin SSE stream.".to_string(),
        ));
    }

    let client_id: u128 = rand::random();
    let rx = state.sse_manager.subscribe_admin();
    let stream = create_sse_stream(rx, client_id);
    debug!("Admin '{}' connected to admin SSE stream (client: {})", claims.sub, client_id);
    Ok(Sse::new(stream).keep_alive(create_keep_alive()))
}

/// Handler SSE pour le canal "all" (tous les utilisateurs)
///
/// Reçoit les annonces globales, maintenance, etc.
/// Endpoint: GET /api/sse/all
pub async fn sse_all_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError>
{
    let client_id: u128 = rand::random();
    let rx = state.sse_manager.subscribe_all();
    let stream = create_sse_stream(rx, client_id);
    debug!("User '{}' connected to 'all' SSE stream (client: {})", claims.sub, client_id);
    Ok(Sse::new(stream).keep_alive(create_keep_alive()))
}

/// Handler SSE pour les événements d'un projet spécifique
///
/// L'utilisateur doit être owner ou participant du projet.
/// Endpoint: GET /`api/sse/projects/{project_id`}
pub async fn sse_project_handler(
    State(state): State<AppState>,
    claims: Claims,
    Path(project_id): Path<i32>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError>
{
    let user_login = claims.sub;

    let project = project_service::get_project_by_id_for_user(
        &state.db_pool,
        project_id,
        &user_login,
        claims.is_admin,
    ).await?.ok_or_else(|| 
    {
        AppError::NotFound(format!("Project {project_id} not found or you don't have access."))
    })?;

    let client_id: u128 = rand::random();
    let rx = state.sse_manager.subscribe_to_project(project_id).await;
    let stream = create_sse_stream(rx, client_id);
    debug!("User '{}' connected to SSE stream for project '{}' (client: {})", user_login, project.name, client_id);
    Ok(Sse::new(stream).keep_alive(create_keep_alive()))
}

/// Handler SSE pour le canal de création temporaire
///
/// Utilisé pendant /projects/create pour recevoir les événements
/// de création en temps réel (pulling, scanning, building, etc.)
/// Endpoint: GET /api/sse/creation
pub async fn sse_creation_handler(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError>
{
    let user_login = claims.sub;
    let client_id: u128 = rand::random();
    let rx = state.sse_manager.subscribe_to_creation(&user_login).await;
    let stream = create_sse_stream(rx, client_id);
    debug!("User '{}' connected to creation SSE stream (client: {})", user_login, client_id);
    Ok(Sse::new(stream).keep_alive(create_keep_alive()))
}

/// Crée le stream SSE à partir d'un broadcast receiver
fn create_sse_stream(
    rx: tokio::sync::broadcast::Receiver<SseEvent>,
    client_id: u128,
) -> impl Stream<Item = Result<Event, Infallible>>
{
    BroadcastStream::new(rx).filter_map(move |result|
    {
        match result
        {
            Ok(sse_event) => match event_to_sse(sse_event)
            {
                Ok(event) => Some(Ok(event)),
                Err(e) =>
                {
                    error!("Failed to serialize SSE event for client {}: {}", client_id, e);
                    None
                }
            },
            Err(BroadcastStreamRecvError::Lagged(n)) =>
            {
                warn!("Client {} lagged behind, {} messages lost. Sending warning.", client_id, n);

                let system_event = SseEvent::System(SystemEvent 
                {
                    level: SystemEventLevel::Warning,
                    message: format!("Connection slow: {n} messages missed"),
                    context: None,
                    timestamp: time::OffsetDateTime::now_utc(),
                });

                event_to_sse(system_event).map_or_else(|_| None, |event| Some(Ok(event)))
            }
        }
    })
}

/// Convertit un `SseEvent` en axum SSE Event
fn event_to_sse(sse_event: SseEvent) -> Result<Event, serde_json::Error>
{
    let event_type = sse_event.event_type();
    let event_id = sse_event.generate_id();
    let json = serde_json::to_string(&sse_event)?;

    Ok(Event::default().event(event_type).id(event_id).data(json))
}

/// Crée la configuration de keep-alive
fn create_keep_alive() -> KeepAlive
{
    KeepAlive::new().interval(Duration::from_secs(5)).text("keep-alive")
}
