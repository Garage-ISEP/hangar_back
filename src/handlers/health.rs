use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

use crate::{error::AppError, state::AppState};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus
{
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Serialize, Clone)]
pub struct ComponentHealth
{
    pub status: HealthStatus,
    pub response_time_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthCheckResponse
{
    pub status: HealthStatus,
    pub timestamp: String,
    pub components: HealthComponents,
}

#[derive(Debug, Serialize)]
pub struct HealthComponents
{
    pub postgres: ComponentHealth,
    pub mariadb: ComponentHealth,
    pub docker: ComponentHealth,
}

impl HealthCheckResponse
{
    fn compute_global_status(components: &HealthComponents) -> HealthStatus
    {
        let statuses = [components.postgres.status,
            components.mariadb.status,
            components.docker.status];

        if statuses.contains(&HealthStatus::Unhealthy)
        {
            HealthStatus::Unhealthy
        }
        else if statuses.contains(&HealthStatus::Degraded)
        {
            HealthStatus::Degraded
        }
        else
        {
            HealthStatus::Healthy
        }
    }
}

pub async fn health_check_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError>
{
    debug!("Starting comprehensive health check");

    let start = Instant::now();

    let (postgres_health, mariadb_health, docker_health) = tokio::join!(
        check_postgres_health(&state),
        check_mariadb_health(&state),
        check_docker_health(&state),
    );

    let components = HealthComponents
    {
        postgres: postgres_health,
        mariadb: mariadb_health,
        docker: docker_health,
    };

    let global_status = HealthCheckResponse::compute_global_status(&components);

    let response = HealthCheckResponse
    {
        status: global_status,
        timestamp: OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string()),
        components,
    };

    let elapsed_us = start.elapsed().as_micros();
    debug!(
        "Health check completed in {}µs with status: {:?}",
        elapsed_us,
        global_status
    );

    let status_code = match global_status
    {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Degraded => StatusCode::OK,
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    };

    Ok((status_code, Json(response)))
}

async fn check_postgres_health(state: &AppState) -> ComponentHealth
{
    let start = Instant::now();

    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1 as health_check").fetch_one(&state.db_pool),
    )
    .await
    {
        Ok(Ok(_)) =>
        {
            let response_time_us = start.elapsed().as_micros() as u64;
            debug!("PostgreSQL health check passed in {}µs", response_time_us);

            let status = if response_time_us > 1_000_000
            {
                warn!("PostgreSQL response time is slow: {}µs", response_time_us);
                HealthStatus::Degraded
            }
            else
            {
                HealthStatus::Healthy
            };

            ComponentHealth
            {
                status,
                response_time_us,
                details: Some("Connected to PostgreSQL".to_string()),
                error: None,
            }
        }
        Ok(Err(e)) =>
        {
            error!("PostgreSQL health check failed: {}", e);
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: start.elapsed().as_micros() as u64,
                details: None,
                error: Some(format!("Database error: {}", e)),
            }
        }
        Err(_) =>
        {
            error!("PostgreSQL health check timed out");
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: 5_000_000,
                details: None,
                error: Some("Connection timeout (5s)".to_string()),
            }
        }
    }
}

async fn check_mariadb_health(state: &AppState) -> ComponentHealth
{
    let start = Instant::now();

    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1 as health_check").fetch_one(&state.mariadb_pool),
    )
    .await
    {
        Ok(Ok(_)) =>
        {
            let response_time_us = start.elapsed().as_micros() as u64;
            debug!("MariaDB health check passed in {}µs", response_time_us);

            let status = if response_time_us > 1_000_000
            {
                warn!("MariaDB response time is slow: {}µs", response_time_us);
                HealthStatus::Degraded
            }
            else
            {
                HealthStatus::Healthy
            };

            ComponentHealth
            {
                status,
                response_time_us,
                details: Some("Connected to MariaDB".to_string()),
                error: None,
            }
        }
        Ok(Err(e)) =>
        {
            error!("MariaDB health check failed: {}", e);
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: start.elapsed().as_micros() as u64,
                details: None,
                error: Some(format!("Database error: {}", e)),
            }
        }
        Err(_) =>
        {
            error!("MariaDB health check timed out");
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: 5_000_000,
                details: None,
                error: Some("Connection timeout (5s)".to_string()),
            }
        }
    }
}

async fn check_docker_health(state: &AppState) -> ComponentHealth
{
    let start = Instant::now();

    match tokio::time::timeout(
        Duration::from_secs(5),
        state.docker_client.ping(),
    )
    .await
    {
        Ok(Ok(_)) =>
        {
            let response_time_us = start.elapsed().as_micros() as u64;
            debug!("Docker health check passed in {}µs", response_time_us);

            let status = if response_time_us > 2_000_000
            {
                warn!("Docker response time is slow: {}µs", response_time_us);
                HealthStatus::Degraded
            }
            else
            {
                HealthStatus::Healthy
            };

            ComponentHealth
            {
                status,
                response_time_us,
                details: None,
                error: None,
            }
        }
        Ok(Err(e)) =>
        {
            error!("Docker health check failed: {}", e);
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: start.elapsed().as_micros() as u64,
                details: None,
                error: Some(format!("Docker daemon error: {}", e)),
            }
        }
        Err(_) =>
        {
            error!("Docker health check timed out");
            ComponentHealth
            {
                status: HealthStatus::Unhealthy,
                response_time_us: 5_000_000,
                details: None,
                error: Some("Connection timeout (5s)".to_string()),
            }
        }
    }
}
