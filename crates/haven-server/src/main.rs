use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade, DefaultBodyLimit},
    http::{Method, HeaderValue},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use socket2::{Domain, Protocol, Socket, Type};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use haven_api::auth::{self, AppState, AppStateInner, AuthRateLimiter};
use haven_api::files;
use haven_api::messages;
use haven_api::middleware::{require_auth, JwtSecret};
use haven_api::reactions;
use haven_gateway::connection;
use haven_gateway::dispatcher::Dispatcher;

/// Placeholder values that MUST NOT be used as the JWT secret.
const PLACEHOLDER_SECRETS: &[&str] = &[
    "change-me-to-a-random-string",
    "dev-secret-change-me",
];

#[derive(Clone)]
struct ServerState {
    app: AppState,
    dispatcher: Dispatcher,
    jwt_secret: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present
    let _ = dotenvy::dotenv();

    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "haven=debug,tower_http=debug".into()),
        )
        .init();

    // Config -- JWT secret is MANDATORY
    let jwt_secret = std::env::var("HAVEN_JWT_SECRET").unwrap_or_default();

    if jwt_secret.is_empty() || PLACEHOLDER_SECRETS.contains(&jwt_secret.as_str()) {
        eprintln!("FATAL: HAVEN_JWT_SECRET is unset or still a placeholder.");
        eprintln!("       Generate a strong random value: openssl rand -base64 48");
        eprintln!("       Set it in your .env file and restart.");
        std::process::exit(1);
    }

    let db_path = std::env::var("HAVEN_DB_PATH").unwrap_or_else(|_| "haven.db".into());
    let host = std::env::var("HAVEN_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("HAVEN_PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()?;

    // Init database
    let db = haven_db::Database::open(&PathBuf::from(&db_path))?;

    // Shared state
    let dispatcher = Dispatcher::new();
    let app_state: AppState = Arc::new(AppStateInner {
        db,
        jwt_secret: jwt_secret.clone(),
        dispatcher: dispatcher.clone(),
        auth_rate_limiter: AuthRateLimiter::new(),
    });

    let jwt_extension = JwtSecret(Arc::from(jwt_secret.as_str()));

    let state = ServerState {
        app: app_state.clone(),
        dispatcher: dispatcher.clone(),
        jwt_secret: jwt_secret.clone(),
    };

    // CORS -- restrict to known origins; extend via HAVEN_CORS_ORIGINS env var
    let cors = build_cors_layer();

    // Routes
    let public_routes = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .with_state(app_state.clone());

    // Create uploads directory for file storage
    std::fs::create_dir_all("./uploads").ok();

    let protected_routes = Router::new()
        .route("/auth/refresh", post(auth::refresh_token))
        .route("/channels/{channel_id}/messages", get(messages::get_messages))
        .route("/channels/{channel_id}/messages", post(messages::send_message))
        .route("/channels/{channel_id}/messages/{message_id}/reactions", post(reactions::toggle_reaction))
        .route("/files", post(files::upload_file))
        .route("/files/{file_id}", get(files::download_file))
        .layer(middleware::from_fn(require_auth))
        .with_state(app_state);

    let ws_route = Router::new()
        .route("/gateway", get(ws_upgrade))
        .with_state(state);

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .merge(ws_route)
        .layer(axum::Extension(jwt_extension))
        // 50 MB body limit -- handles images and video file uploads.
        // Tune via HAVEN_MAX_BODY_SIZE env var if needed for larger files.
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Haven server listening on {}", addr);

    // Create listener via socket2 so we can set TCP_NODELAY on the listening socket.
    // This ensures accepted connections inherit the NODELAY flag, eliminating
    // Nagle's algorithm latency for small WebSocket frames.
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_nodelay(true)?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(socket.into())?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Build a CORS layer that allows the Tauri client and localhost dev server.
/// Additional origins can be supplied via the HAVEN_CORS_ORIGINS env var
/// (comma-separated list of origins, e.g. "http://192.168.1.5:1420,https://my.domain").
fn build_cors_layer() -> CorsLayer {
    let mut origins: Vec<HeaderValue> = vec![
        "tauri://localhost".parse().unwrap(),
        "http://tauri.localhost".parse().unwrap(),
        "https://tauri.localhost".parse().unwrap(),
        "http://localhost:1420".parse().unwrap(),
        "http://localhost".parse().unwrap(),
    ];

    if let Ok(extra) = std::env::var("HAVEN_CORS_ORIGINS") {
        for raw in extra.split(',') {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                if let Ok(val) = trimmed.parse::<HeaderValue>() {
                    origins.push(val);
                } else {
                    eprintln!("WARNING: ignoring invalid CORS origin: {trimmed}");
                }
            }
        }
    }

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers(tower_http::cors::Any)
        .allow_credentials(false)
}

async fn ws_upgrade(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.max_frame_size(1_048_576)       // 1 MB max frame
        .max_message_size(2_097_152)   // 2 MB max message
        .on_upgrade(move |socket| {
            connection::handle_connection(socket, state.dispatcher, state.jwt_secret)
        })
}
