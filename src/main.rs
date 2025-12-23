use hangar_back::config::Config;
use hangar_back::state::InnerState;
use hangar_back::router;

use std::net::{SocketAddr, Ipv4Addr};
use sqlx::postgres::PgPoolOptions;
use sqlx::mysql::MySqlPoolOptions;
use tokio::net::TcpListener;
use tracing::info;

#[tokio::main]
async fn main()
{
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    let config = match Config::from_env() 
    {
        Ok(config) => config,
        Err(e) => 
        {
            tracing::error!("❌ Configuration error: {}", e);
            std::process::exit(1); // On quitte proprement
        }
    };

    let db_pool = match PgPoolOptions::new().max_connections(config.db_max_connections).connect(&config.db_url).await
    {
        Ok(pool) => 
        {
            info!("✅ Database connection pool created successfully.");
            pool
        }
        Err(e) => 
        {
            tracing::error!("❌ Failed to create database connection pool: {}", e);
            std::process::exit(1);
        }
    };
    
    info!("🚀 Applying database migrations...");
    match sqlx::migrate!("./migrations").run(&db_pool).await 
    {
        Ok(_) => info!("✅ Database migrations applied successfully."),
        Err(e) => 
        {
            tracing::error!("❌ Failed to apply database migrations: {}", e);
            std::process::exit(1);
        }
    }

    let mariadb_pool = match MySqlPoolOptions::new().max_connections(config.db_max_connections).connect(&config.mariadb_url).await
    {
        Ok(pool) => 
        {
            info!("✅ MariaDB connection pool created successfully.");
            pool
        }
        Err(e) => 
        {
            tracing::error!("❌ Failed to create MariaDB connection pool: {}", e);
            std::process::exit(1);
        }
    };


    let docker_client = match bollard::Docker::connect_with_local_defaults() 
    {
        Ok(client) => client,
        Err(e) => 
        {
            tracing::error!("❌ Docker connection error: {}", e);
            std::process::exit(1);
        }
    };

    let app_state = InnerState::new(config.clone(), docker_client, db_pool, mariadb_pool);
    let app = router::create_router(app_state);

    let addr = SocketAddr::from((config.host.parse::<Ipv4Addr>().unwrap(), config.port));
    info!("🚀 Server listening on http://{}", addr);

    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("🔗 Listening on: {}", addr);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}