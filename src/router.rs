use crate::{handlers, state::AppState, middleware};
use axum::{error_handling::HandleErrorLayer, http::StatusCode, middleware as axum_middleware, routing::{delete, get, post, put}, BoxError, Router};
use tower::{timeout::TimeoutLayer, ServiceBuilder};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use std::time::Duration;

pub fn create_router(state: AppState) -> Router 
{
    let common_layer = ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive())
                .layer(CompressionLayer::new())
                .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::REQUEST_TIMEOUT}))
                .layer(TimeoutLayer::new(Duration::from_secs(state.config.timeout_normal)));

    let long_running_layer = ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive())
                .layer(CompressionLayer::new())
                .layer(HandleErrorLayer::new(|_: BoxError| async {StatusCode::REQUEST_TIMEOUT}))
                .layer(TimeoutLayer::new(Duration::from_secs(state.config.timeout_long)));
    
    let sse_layer = ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive());
    
    let sse_routes = Router::new()
        .route("/api/sse/admin", get(handlers::sse_handler::sse_admin_handler))
        .route("/api/sse/all", get(handlers::sse_handler::sse_all_handler))
        .route("/api/sse/projects/{project_id}", get(handlers::sse_handler::sse_project_handler))
        .route("/api/sse/creation", get(handlers::sse_handler::sse_creation_handler))
        .route_layer(axum_middleware::from_fn_with_state(state.clone(), middleware::auth))
        .layer(sse_layer);

    let admin_routes = Router::new()
        .route("/api/admin/projects", get(handlers::admin_handler::list_all_projects_handler))
        .route("/api/admin/metrics", get(handlers::admin_handler::get_global_metrics_handler))
        .route("/api/admin/projects/down", get(handlers::admin_handler::get_down_projects_handler))
        .route_layer(axum_middleware::from_fn(middleware::admin_auth))
        .route_layer(axum_middleware::from_fn_with_state(state.clone(), middleware::auth))
        .route_layer(common_layer.clone());

    let public_routes = Router::new()
        .route("/api/health", get(handlers::health::health_check_handler))
        .route("/api/auth/callback", get(handlers::auth_handler::auth_callback_handler))
        .route_layer(common_layer.clone());

    let protected_routes = Router::new()
        .route("/api/auth/me", get(handlers::auth_handler::get_current_user_handler))
        .route("/api/auth/logout", get(handlers::auth_handler::logout_handler))
        .route("/api/projects/owned", get(handlers::project_handler::list_owned_projects_handler))
        .route("/api/projects/participations", get(handlers::project_handler::list_participating_projects_handler))
        .route("/api/projects/{project_id}", get(handlers::project_handler::get_project_details_handler))
        .route("/api/projects/{project_id}/status", get(handlers::project_handler::get_project_status_handler))
        .route("/api/projects/{project_id}/start", post(handlers::project_handler::start_project_handler))
        .route("/api/projects/{project_id}/stop", post(handlers::project_handler::stop_project_handler))
        .route("/api/projects/{project_id}/restart", post(handlers::project_handler::restart_project_handler))
        .route("/api/projects/{project_id}/logs", get(handlers::project_handler::get_project_logs_handler))
        .route("/api/projects/{project_id}/metrics", get(handlers::project_handler::get_project_metrics_handler))
        .route("/api/projects/{project_id}/participants", post(handlers::project_handler::add_participant_handler))
        .route("/api/projects/{project_id}/participants/{participant_id}", delete(handlers::project_handler::remove_participant_handler))
        .route("/api/databases/mine", get(handlers::database_handler::get_my_database_handler))
        .route("/api/databases", post(handlers::database_handler::create_database_handler))
        .route("/api/databases/{db_id}", delete(handlers::database_handler::delete_my_database_handler))
        .route("/api/projects/{project_id}/database/{db_id}", put(handlers::database_handler::link_database_handler))
        .route("/api/projects/{project_id}/database", delete(handlers::database_handler::unlink_database_handler))
        .route("/api/projects/{project_id}/database/delete", delete(handlers::database_handler::delete_linked_database_handler))
        .route_layer(axum_middleware::from_fn_with_state(state.clone(), middleware::auth))
        .route_layer(common_layer.clone());

    let long_running_protected_routes = Router::new()
        .route("/api/projects/deploy", post(handlers::project_handler::deploy_project_handler))
        .route("/api/projects/{project_id}", delete(handlers::project_handler::purge_project_handler))
        .route("/api/projects/{project_id}/image", put(handlers::project_handler::update_project_image_handler))
        .route("/api/projects/{project_id}/env", put(handlers::project_handler::update_env_vars_handler))
        .route("/api/projects/{project_id}/rebuild", put(handlers::project_handler::rebuild_project_handler))
        .route_layer(axum_middleware::from_fn_with_state(state.clone(), middleware::auth))
        .route_layer(long_running_layer);

    Router::new()
        .merge(public_routes)
        .merge(sse_routes)
        .merge(protected_routes)
        .merge(admin_routes)
        .merge(long_running_protected_routes)
        .with_state(state)
}

