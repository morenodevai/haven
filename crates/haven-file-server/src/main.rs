mod cleanup;
mod db;
mod fast_transfer;
mod routes;
mod storage;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use socket2::{Domain, Protocol, Socket, Type};

use axum::{Router, extract::DefaultBodyLimit, routing::{get, post, put, delete}};
use axum::http::{Method, header::{AUTHORIZATION, CONTENT_TYPE, RANGE}};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::db::FileDb;
use crate::routes::AppState;
use crate::storage::Storage;

/// Placeholder JWT secrets that MUST NOT be used.
const PLACEHOLDER_SECRETS: &[&str] = &[
    "change-me-to-a-random-string",
    "dev-secret-change-me",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "haven_file_server=debug,haven_fast_transfer=info,tower_http=debug".into()),
        )
        .init();

    // Config
    let jwt_secret = std::env::var("HAVEN_JWT_SECRET").unwrap_or_default();
    if jwt_secret.is_empty() || PLACEHOLDER_SECRETS.contains(&jwt_secret.as_str()) {
        eprintln!("FATAL: HAVEN_JWT_SECRET is unset or still a placeholder.");
        eprintln!("       This must match the messaging server's secret.");
        eprintln!("       Set it in your .env file and restart.");
        std::process::exit(1);
    }

    let host = std::env::var("HAVEN_FILE_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("HAVEN_FILE_PORT")
        .unwrap_or_else(|_| "3211".into())
        .parse()?;
    let storage_dir: PathBuf = std::env::var("HAVEN_FILE_STORAGE_DIR")
        .unwrap_or_else(|_| "./file-storage".into())
        .into();
    let db_path: PathBuf = std::env::var("HAVEN_FILE_DB_PATH")
        .unwrap_or_else(|_| "haven-files.db".into())
        .into();
    let retention_hours: u64 = std::env::var("HAVEN_FILE_RETENTION_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(168); // 7 days

    // Init DB and storage
    let db = Arc::new(FileDb::open(&db_path)?);
    let storage = Arc::new(Storage::new(storage_dir).await?);

    // Bind UDP on same port as HTTP (TCP and UDP don't conflict)
    let udp_bind_addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    let udp_socket = {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_recv_buffer_size(32 * 1024 * 1024)?;
        sock.set_nonblocking(false)?;
        sock.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
        sock.bind(&udp_bind_addr.into())?;
        let std_sock: std::net::UdpSocket = sock.into();
        Arc::new(std_sock)
    };
    info!("UDP fast transfer socket bound on {}", udp_bind_addr);

    // Background cleanup task (runs every hour)
    let cleanup_db = db.clone();
    let cleanup_storage = storage.clone();
    tokio::spawn(cleanup::run_cleanup_loop(cleanup_db, cleanup_storage, 3600));

    let state = AppState {
        db,
        storage,
        jwt_secret,
        retention_hours,
        udp_socket,
        udp_port: port,
    };

    // CORS — permissive for file server (clients connect from various origins)
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE, RANGE])
        .allow_credentials(false);

    let app = Router::new()
        .route("/transfers", post(routes::create_transfer))
        .route("/transfers/{id}/data", put(routes::upload_data))
        .route("/transfers/{id}/chunks/{index}", put(routes::upload_chunk))
        .route("/transfers/{id}/data", get(routes::download_data))
        .route("/transfers/{id}", get(routes::get_transfer_status))
        .route("/transfers/{id}/confirm", post(routes::confirm_transfer))
        .route("/transfers/{id}", delete(routes::delete_transfer))
        .route("/fast-transfer", get(routes::fast_transfer_ws))
        .route("/health", get(routes::health))
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 * 1024)) // 4 GB max
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Haven file server listening on {}", addr);
    info!("Retention: {} hours ({} days)", retention_hours, retention_hours / 24);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => info!("Received Ctrl+C, shutting down..."),
            _ = sigterm.recv() => info!("Received SIGTERM, shutting down..."),
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        info!("Received Ctrl+C, shutting down...");
    }
}
