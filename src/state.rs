use std::sync::Arc;
use bollard::Docker;
use sqlx::{MySqlPool, PgPool};
use crate::{config::Config, sse::manager::SseManager};

pub type AppState = Arc<InnerState>;

pub struct InnerState 
{
    pub config : Config,
    pub http_client: reqwest::Client,
    pub docker_client: Docker,
    pub db_pool: PgPool,
    pub mariadb_pool: MySqlPool,
    pub sse_manager: SseManager,
}

impl InnerState 
{
    pub fn new(config: Config, docker_client: Docker, db_pool: PgPool, mariadb_pool: MySqlPool) -> AppState 
    {
        Arc::new(Self 
        {
            config,
            http_client: reqwest::Client::new(),
            docker_client,
            db_pool,
            mariadb_pool,
            sse_manager: SseManager::new(),
        })
    }
}