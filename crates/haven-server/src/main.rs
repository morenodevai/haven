use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade},
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use haven_api::auth::{self, AppState, AppStateInner};
use haven_api::messages;
use haven_api::middleware::require_auth;
use haven_api::reactions;
use haven_gateway::connection;
use haven_gateway::dispatcher::Dispatcher;

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

    // Config
    let jwt_secret =
        std::env::var("HAVEN_JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-me".into());
    let db_path = std::env::var("HAVEN_DB_PATH").unwrap_or_else(|_| "haven.db".into());
    let host = std::env::var("HAVEN_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("HAVEN_PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()?;

    // Init database
    let db = haven_db::Database::open(&PathBuf::from(&db_path))?;

    // Shared state
    let dispatcher = Dispatcher::new();
    let app_state: AppState = Arc::new(AppStateInner { db, jwt_secret: jwt_secret.clone(), dispatcher: dispatcher.clone() });

    let state = ServerState {
        app: app_state.clone(),
        dispatcher: dispatcher.clone(),
        jwt_secret: jwt_secret.clone(),
    };

    // Routes
    let public_routes = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .with_state(app_state.clone());

    let protected_routes = Router::new()
        .route("/channels/{channel_id}/messages", get(messages::get_messages))
        .route("/channels/{channel_id}/messages", post(messages::send_message))
        .route("/channels/{channel_id}/messages/{message_id}/reactions", post(reactions::toggle_reaction))
        .layer(middleware::from_fn(require_auth))
        .with_state(app_state);

    let ws_route = Router::new()
        .route("/gateway", get(ws_upgrade))
        .with_state(state);

    let app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .merge(ws_route)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Haven server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_upgrade(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        connection::handle_connection(socket, state.dispatcher, state.jwt_secret)
    })
}
