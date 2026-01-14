use hangar_back::config::Config;
use hangar_back::sse::manager::start_cleanup_task;
use hangar_back::sse::tasks::{start_docker_events_listener, start_metrics_collector};
use hangar_back::state::InnerState;
use hangar_back::router;

use std::net::{SocketAddr, Ipv4Addr};
use sqlx::postgres::PgPoolOptions;
use sqlx::mysql::MySqlPoolOptions;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{info, warn};

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
        Ok(()) => info!("✅ Database migrations applied successfully."),
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

    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    tokio::spawn(start_cleanup_task(
        app_state.sse_manager.clone(), 
        shutdown_tx.subscribe()
    ));

    tokio::spawn(start_docker_events_listener(
        app_state.clone(), 
        shutdown_tx.subscribe()
    ));

    tokio::spawn(start_metrics_collector(
        app_state.clone(), 
        shutdown_tx.subscribe()
    ));

    let app = router::create_router(app_state);

    let addr = SocketAddr::from((config.host.parse::<Ipv4Addr>().unwrap(), config.port));
    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("🔗 Listening on: {}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await
        .unwrap();
}

async fn shutdown_signal(shutdown_tx: tokio::sync::broadcast::Sender<()>) 
{
    let ctrl_c = async 
    {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async 
    {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! 
    {
        () = ctrl_c => {},
        () = terminate => {},
    }

    warn!("Shutdown signal received, stopping background tasks...");
    let _ = shutdown_tx.send(());
}