use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Query, State, WebSocketUpgrade, DefaultBodyLimit},
    http::{Method, HeaderValue, header::AUTHORIZATION, header::CONTENT_TYPE},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;
use socket2::{Domain, Protocol, Socket, Type};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use haven_api::auth::{self, AppState, AppStateInner, AuthRateLimiter};
use haven_api::files;
use haven_api::messages;
use haven_api::middleware::{require_auth, JwtSecret, Claims};
use haven_api::reactions;
use haven_gateway::connection;
use haven_gateway::dispatcher::Dispatcher;
use haven_gateway::tcp_relay::TcpRelayState;

/// Placeholder values that MUST NOT be used as the JWT secret.
const PLACEHOLDER_SECRETS: &[&str] = &[
    "change-me-to-a-random-string",
    "dev-secret-change-me",
];

#[derive(Clone)]
struct ServerState {
    #[allow(dead_code)]
    app: AppState,
    dispatcher: Dispatcher,
    jwt_secret: String,
}

/// Query parameters for the WebSocket upgrade endpoint.
#[derive(Debug, Deserialize)]
struct GatewayQuery {
    token: Option<String>,
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

    // #14: Configurable uploads directory
    let uploads_dir: PathBuf = std::env::var("HAVEN_UPLOADS_DIR")
        .unwrap_or_else(|_| "./uploads".into())
        .into();
    let uploads_dir = std::fs::canonicalize(&uploads_dir).unwrap_or_else(|_| {
        std::fs::create_dir_all(&uploads_dir).ok();
        std::fs::canonicalize(&uploads_dir).unwrap_or_else(|_| uploads_dir.clone())
    });

    // #5: Configurable body limit (default 10 MB)
    let max_body_size: usize = std::env::var("HAVEN_MAX_BODY_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10 * 1024 * 1024);

    // Init database
    let db = haven_db::Database::open(&PathBuf::from(&db_path))?;

    // Shared state
    let dispatcher = Dispatcher::new();
    let app_state: AppState = Arc::new(AppStateInner {
        db,
        jwt_secret: jwt_secret.clone(),
        dispatcher: dispatcher.clone(),
        auth_rate_limiter: AuthRateLimiter::new(),
        uploads_dir: uploads_dir.clone(),
    });

    let jwt_extension = JwtSecret(Arc::from(jwt_secret.as_str()));

    let state = ServerState {
        app: app_state.clone(),
        dispatcher: dispatcher.clone(),
        jwt_secret: jwt_secret.clone(),
    };

    // CORS -- restrict to known origins; extend via HAVEN_CORS_ORIGINS env var
    // #23: CSRF protection is not needed because this is a Tauri desktop app that
    // uses Bearer token authentication (not cookies). Browsers automatically attach
    // cookies on cross-origin requests (enabling CSRF), but Bearer tokens in the
    // Authorization header are only sent by explicit JavaScript code. The CSP and
    // CORS policies prevent third-party scripts from running in the app context.
    let cors = build_cors_layer();

    // Routes
    let public_routes = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .with_state(app_state.clone());

    // Create uploads directory for file storage
    std::fs::create_dir_all(&uploads_dir).ok();

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
        .layer(DefaultBodyLimit::max(max_body_size))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Haven server listening on {}", addr);

    // Create listener via socket2 for custom backlog, address reuse, and TCP_NODELAY.
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_nodelay(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(socket.into())?;

    // TCP relay for native file transfers (bypasses WebSocket/browser bottleneck)
    let relay_port: u16 = std::env::var("HAVEN_RELAY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| port + 1);
    {
        let relay_addr: SocketAddr = format!("{}:{}", host, relay_port).parse()?;
        let relay_socket = Socket::new(
            Domain::for_address(relay_addr),
            Type::STREAM,
            Some(Protocol::TCP),
        )?;
        relay_socket.set_reuse_address(true)?;
        relay_socket.set_nodelay(true)?;
        relay_socket.bind(&relay_addr.into())?;
        relay_socket.listen(1024)?;
        relay_socket.set_nonblocking(true)?;
        let relay_listener = tokio::net::TcpListener::from_std(relay_socket.into())?;
        info!("TCP file relay listening on {}", relay_addr);
        let relay_state = TcpRelayState::new(jwt_secret.clone());
        tokio::spawn(relay_state.run(relay_listener));
    }

    // #12: into_make_service_with_connect_info to provide SocketAddr for rate limiting
    // #15: graceful shutdown on Ctrl+C / SIGTERM
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

/// #15: Listen for Ctrl+C / SIGTERM to trigger graceful shutdown.
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
        // #27: Only allow headers actually used by the client (not Any)
        .allow_headers([AUTHORIZATION, CONTENT_TYPE])
        .allow_credentials(false)
}

/// #6: WebSocket upgrade with JWT authentication BEFORE upgrading.
/// The token is extracted from `?token=` query param or Authorization header.
/// If invalid, a 401 is returned without upgrading the connection.
async fn ws_upgrade(
    State(state): State<ServerState>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    Query(query): Query<GatewayQuery>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, axum::http::StatusCode> {
    // Extract token from query param or Authorization header
    let token = query.token.or_else(|| {
        headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_string())
    });

    let token = token.ok_or(axum::http::StatusCode::UNAUTHORIZED)?;

    // Validate JWT before upgrading
    let token_data = decode::<Claims>(
        &token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| axum::http::StatusCode::UNAUTHORIZED)?;

    let user_id = token_data.claims.sub;
    let username = token_data.claims.username;

    info!("{} ({}) pre-authenticated for WebSocket upgrade", username, user_id);

    Ok(ws
        .max_frame_size(4 * 1024 * 1024)    // 4 MB max frame (supports larger chunk sizes)
        .max_message_size(8 * 1024 * 1024) // 8 MB max message
        .on_upgrade(move |socket| {
            connection::handle_connection_authenticated(socket, state.dispatcher, user_id, username)
        }))
}
