use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, DefaultBodyLimit},
    http::{Method, HeaderValue, HeaderMap, StatusCode, header, header::AUTHORIZATION, header::CONTENT_TYPE},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use futures_util::{TryStreamExt, SinkExt, StreamExt};
use reqwest::Client;
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;
use socket2::{Domain, Protocol, Socket, Type};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use haven_api::admin;
use haven_api::auth::{self, AppState, AppStateInner, AuthRateLimiter};
use haven_api::files;
use haven_api::messages;
use haven_api::middleware::{require_auth, JwtSecret, Claims};
use haven_api::reactions;
use haven_gateway::connection;
use haven_gateway::dispatcher::Dispatcher;
use haven_gateway::turn::{TurnConfig, TurnServer as TurnRelay};

use haven_types::PLACEHOLDER_SECRETS;

/// RFC 5764: STUN/TURN messages have first byte in 0x00..=0x3F (first 2 bits = 00).
/// HTTP requests start with ASCII letters (0x41+). Used for TCP multiplexing.
const STUN_FIRST_BYTE_MAX: u8 = 0x3F;

#[derive(Clone)]
struct ServerState {
    app: AppState,
    dispatcher: Dispatcher,
    jwt_secret: String,
    file_server_url: Option<String>,
    file_server_internal_url: Option<String>,
    http_client: Client,
    turn_servers: Option<Vec<haven_types::events::TurnServer>>,
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

    // Init logging (stderr is unbuffered — logs appear immediately even when redirected to file)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
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

    // #5: Configurable body limit (default 2 GB)
    let max_body_size: usize = std::env::var("HAVEN_MAX_BODY_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024);

    // Init database (Arc-wrapped for sharing between API + gateway connection handlers)
    let db = Arc::new(haven_db::Database::open(&PathBuf::from(&db_path))?);

    // Shared state
    let dispatcher = Dispatcher::new();
    let app_state: AppState = Arc::new(AppStateInner {
        db: db.clone(),
        jwt_secret: jwt_secret.clone(),
        dispatcher: dispatcher.clone(),
        auth_rate_limiter: AuthRateLimiter::new(),
        uploads_dir: uploads_dir.clone(),
    });

    let jwt_extension = JwtSecret(Arc::from(jwt_secret.as_str()));

    // Optional file server URL — enables store-and-forward file transfers
    let file_server_url = std::env::var("HAVEN_FILE_SERVER_URL").ok();
    if let Some(ref url) = file_server_url {
        info!("File server URL: {}", url);
    }

    let file_server_internal_url = std::env::var("HAVEN_FILE_SERVER_INTERNAL_URL").ok();
    if let Some(ref url) = file_server_internal_url {
        info!("File server internal URL (proxy target): {}", url);
    }

    // ── TURN relay ────────────────────────────────────────────────────
    let (turn_servers_for_state, turn_relay_arc) = if let (Ok(turn_user), Ok(turn_pass), Ok(public_ip)) = (
        std::env::var("HAVEN_TURN_USER"),
        std::env::var("HAVEN_TURN_PASS"),
        std::env::var("HAVEN_PUBLIC_IP"),
    ) {
        let public_ip: std::net::IpAddr = public_ip.parse()?;

        let turn_config = TurnConfig {
            udp_port: port,  // advertise gateway port for UDP too (same port, UDP vs TCP don't conflict)
            public_ip,
            realm: "haven".to_string(),
            username: turn_user.clone(),
            password: turn_pass.clone(),
        };

        let turn_relay = std::sync::Arc::new(TurnRelay::new(turn_config));

        // Spawn UDP TURN listener on the SAME port as gateway (3210)
        // UDP and TCP don't conflict on the same port number
        let turn_udp = turn_relay.clone();
        let udp_port = port;
        tokio::spawn(async move {
            if let Err(e) = turn_udp.run_udp(udp_port).await {
                tracing::error!("TURN UDP listener failed: {}", e);
            }
        });

        let ice_urls = turn_relay.ice_urls();
        info!("TURN relay listening on UDP+TCP {}", port);
        info!("TURN ICE URLs: {:?}", ice_urls);

        let servers = vec![haven_types::events::TurnServer {
            urls: ice_urls,
            username: turn_user,
            credential: turn_pass,
        }];

        (Some(servers), Some(turn_relay))
    } else {
        info!("TURN relay not configured (set HAVEN_TURN_USER, HAVEN_TURN_PASS, HAVEN_PUBLIC_IP to enable)");
        (None, None)
    };

    let http_client = Client::builder().no_proxy().build()?;

    let state = ServerState {
        app: app_state.clone(),
        dispatcher: dispatcher.clone(),
        jwt_secret: jwt_secret.clone(),
        file_server_url,
        file_server_internal_url,
        http_client: http_client.clone(),
        turn_servers: turn_servers_for_state,
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
        .route("/pending-offers", get(get_pending_offers))
        .layer(middleware::from_fn(require_auth))
        .with_state(app_state);

    let ws_route = Router::new()
        .route("/gateway", get(ws_upgrade))
        .with_state(state.clone());

    // File transfer proxy routes — forward /ft/* to internal file server (3211)
    let file_proxy_routes = Router::new()
        .route("/ft/transfers", post(ft_create_transfer))
        .route("/ft/transfers/{id}", get(ft_get_transfer))
        .route("/ft/transfers/{id}", delete(ft_delete_transfer))
        .route("/ft/transfers/{id}/data", put(ft_upload_data))
        .route("/ft/transfers/{id}/data", get(ft_download_data))
        .route("/ft/transfers/{id}/chunks/{index}", put(ft_upload_chunk))
        .route("/ft/transfers/{id}/chunks", get(ft_get_chunks))
        .route("/ft/transfers/{id}/confirm", post(ft_confirm_transfer))
        .layer(middleware::from_fn(require_auth))
        .with_state(state.clone());

    // Fast-transfer WS proxy — auth is handled by the upstream file server via ?token= param
    let ft_ws_route = Router::new()
        .route("/ft/fast-transfer", get(ft_fast_transfer_ws))
        .with_state(state.clone());

    // ── Admin dashboard ─────────────────────────────────────────────────
    let admin_secret = std::env::var("HAVEN_ADMIN_SECRET").unwrap_or_default();
    let start_time = std::time::Instant::now();

    let admin_routes = if !admin_secret.is_empty() {
        let admin_state = admin::AdminState {
            admin_secret,
            jwt_secret: jwt_secret.clone(),
            db: db.clone(),
            dispatcher: dispatcher.clone(),
            http_client: http_client.clone(),
            file_server_internal_url: state.file_server_internal_url.clone(),
            start_time,
        };

        // Login is public (no middleware)
        let admin_login = Router::new()
            .route("/admin/login", post(admin::admin_login))
            .with_state(admin_state.clone());

        // Protected admin API
        let admin_api = Router::new()
            .route("/admin/stats", get(admin::get_stats))
            .route("/admin/users", get(admin::list_users))
            .route("/admin/users/{id}", delete(admin::delete_user))
            .route("/admin/kick/{user_id}", post(admin::kick_user))
            .route("/admin/voice", get(admin::get_voice_state))
            .route("/admin/messages", get(admin::list_messages))
            .route("/admin/offers", get(admin::list_offers))
            .route("/admin/offers/{id}", delete(admin::delete_offer))
            .route("/admin/transfers", get(admin::list_transfers))
            .route("/admin/transfers/{id}", delete(admin::delete_transfer))
            .route("/admin/config", get(admin::get_config))
            .route("/admin/channels", get(admin::list_channels))
            .route("/admin/channels", post(admin::create_channel))
            .route("/admin/channels/{id}", delete(admin::delete_channel))
            .layer(middleware::from_fn(admin::require_admin))
            .with_state(admin_state.clone());

        // Admin WebSocket (auth via query param, not middleware)
        let admin_ws = Router::new()
            .route("/admin/ws", get(admin::admin_ws))
            .with_state(admin_state);

        // Static file serving — dashboard UI
        let admin_ui = Router::new()
            .nest_service("/dashboard", tower_http::services::ServeDir::new("./admin-ui"));

        info!("Admin dashboard enabled at /dashboard/");
        Some(admin_login.merge(admin_api).merge(admin_ws).merge(admin_ui))
    } else {
        info!("HAVEN_ADMIN_SECRET not set -- admin dashboard disabled");
        None
    };

    let mut app = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .merge(file_proxy_routes)
        .merge(ft_ws_route)
        .merge(ws_route);

    if let Some(admin) = admin_routes {
        app = app.merge(admin);
    }

    let app = app
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

    if let Some(turn_relay) = turn_relay_arc {
        // TCP multiplexing: peek first byte to route STUN/TURN vs HTTP.
        // STUN/TURN messages start with 0x00-0x3F (first 2 bits = 00).
        // HTTP requests start with ASCII letters (G=0x47, P=0x50, etc).
        // This lets TURN-over-TCP share port 3210 with no new port forwards.
        use hyper_util::rt::TokioIo;
        use tower::Service;

        info!("TCP multiplexing enabled: TURN + HTTP on port {}", port);

        let mut make_svc = app.into_make_service_with_connect_info::<SocketAddr>();

        let shutdown = shutdown_signal();
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, peer_addr) = match result {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!("TCP accept error: {}", e);
                            continue;
                        }
                    };

                    // Peek first byte to determine protocol
                    let mut peek_buf = [0u8; 1];
                    match stream.peek(&mut peek_buf).await {
                        Ok(1) => {}
                        _ => continue,
                    }

                    if peek_buf[0] <= STUN_FIRST_BYTE_MAX {
                        // STUN/TURN — hand to TURN TCP handler
                        let turn = turn_relay.clone();
                        tokio::spawn(async move {
                            turn.handle_tcp_connection(stream, peer_addr).await;
                        });
                    } else {
                        // HTTP/WS — hand to axum via hyper
                        let svc = make_svc.call(peer_addr).await.unwrap();
                        let hyper_svc = hyper_util::service::TowerToHyperService::new(svc);
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                                .serve_connection_with_upgrades(io, hyper_svc)
                                .await;
                        });
                    }
                }
                _ = &mut shutdown => {
                    info!("Shutting down...");
                    break;
                }
            }
        }
    } else {
        // No TURN — standard axum serve
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    }

    Ok(())
}

use haven_types::shutdown_signal;

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

    let file_server_url = state.file_server_url.clone();
    let turn_servers = state.turn_servers.clone();
    let db = state.app.db.clone();
    Ok(ws
        .max_frame_size(4 * 1024 * 1024)    // 4 MB max frame (supports larger chunk sizes)
        .max_message_size(8 * 1024 * 1024) // 8 MB max message
        .on_upgrade(move |socket| {
            connection::handle_connection_authenticated(socket, state.dispatcher, user_id, username, file_server_url, turn_servers, Some(db))
        }))
}

// ── Pending offers endpoint ──────────────────────────────────────────

/// GET /pending-offers — returns pending file/folder offers for the authenticated user.
async fn get_pending_offers(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<impl IntoResponse, StatusCode> {
    let user_id = claims.sub.to_string();

    let file_offers = state
        .db
        .get_pending_offers_for_user(&user_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let folder_offers = state
        .db
        .get_pending_folder_offers_for_user(&user_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let file_offers_json: Vec<serde_json::Value> = file_offers
        .into_iter()
        .map(|o| {
            serde_json::json!({
                "transfer_id": o.transfer_id,
                "from_user_id": o.from_user_id,
                "filename": o.filename,
                "file_size": o.file_size,
                "file_sha256": o.file_sha256,
                "chunk_hashes": o.chunk_hashes.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()),
                "file_server_url": o.file_server_url,
                "folder_id": o.folder_id,
                "status": o.status,
            })
        })
        .collect();

    let folder_offers_json: Vec<serde_json::Value> = folder_offers
        .into_iter()
        .map(|f| {
            serde_json::json!({
                "folder_id": f.folder_id,
                "from_user_id": f.from_user_id,
                "folder_name": f.folder_name,
                "total_size": f.total_size,
                "file_count": f.file_count,
                "manifest": serde_json::from_str::<serde_json::Value>(&f.manifest).unwrap_or_default(),
                "file_server_url": f.file_server_url,
                "status": f.status,
            })
        })
        .collect();

    Ok(axum::Json(serde_json::json!({
        "file_offers": file_offers_json,
        "folder_offers": folder_offers_json,
    })))
}

// ── File transfer proxy ──────────────────────────────────────────────

async fn ft_proxy(
    state: &ServerState,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    let base = state.file_server_internal_url.as_deref().unwrap_or("http://127.0.0.1:3211");
    let url = format!("{}{}", base, path);

    let mut builder = state.http_client.request(method, &url);
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }

    let body_stream = body.into_data_stream();
    let reqwest_body = reqwest::Body::wrap_stream(body_stream);
    builder = builder.body(reqwest_body);

    let upstream_resp = builder.send().await.map_err(|e| {
        tracing::error!(error = %e, upstream = %url, "file proxy: upstream request failed");
        StatusCode::BAD_GATEWAY
    })?;

    let status = StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let mut response_builder = Response::builder().status(status);
    if let Some(ct) = upstream_resp.headers().get(header::CONTENT_TYPE) {
        response_builder = response_builder.header(header::CONTENT_TYPE, ct);
    }
    if let Some(cl) = upstream_resp.headers().get(header::CONTENT_LENGTH) {
        response_builder = response_builder.header(header::CONTENT_LENGTH, cl);
    }

    let stream = upstream_resp.bytes_stream().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, e)
    });
    let body = Body::from_stream(stream);

    response_builder.body(body).map_err(|e| {
        tracing::error!(error = %e, "file proxy: failed to build response");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn ft_create_transfer(State(state): State<ServerState>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::POST, "/transfers", &headers, body).await
}

async fn ft_get_transfer(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::GET, &format!("/transfers/{id}"), &headers, body).await
}

async fn ft_delete_transfer(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::DELETE, &format!("/transfers/{id}"), &headers, body).await
}

async fn ft_upload_data(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::PUT, &format!("/transfers/{id}/data"), &headers, body).await
}

async fn ft_download_data(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::GET, &format!("/transfers/{id}/data"), &headers, body).await
}

async fn ft_upload_chunk(State(state): State<ServerState>, Path((id, index)): Path<(String, String)>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::PUT, &format!("/transfers/{id}/chunks/{index}"), &headers, body).await
}

async fn ft_get_chunks(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::GET, &format!("/transfers/{id}/chunks"), &headers, body).await
}

async fn ft_confirm_transfer(State(state): State<ServerState>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Result<Response, StatusCode> {
    ft_proxy(&state, Method::POST, &format!("/transfers/{id}/confirm"), &headers, body).await
}

/// WebSocket proxy for /ft/fast-transfer → file server /fast-transfer.
/// Bidirectionally pipes messages between the client and the upstream file server.
async fn ft_fast_transfer_ws(
    State(state): State<ServerState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    let base = state.file_server_internal_url.as_deref().unwrap_or("http://127.0.0.1:3211");
    // Convert http:// to ws://
    let ws_base = base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);

    // Forward the token query param to the upstream
    let token = params.get("token").cloned().unwrap_or_default();
    let upstream_url = format!("{}/fast-transfer?token={}", ws_base, token);

    Ok(ws.on_upgrade(move |client_socket| async move {
        // Connect to upstream file server WS
        let upstream = match tokio_tungstenite::connect_async(&upstream_url).await {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::error!(error = %e, "fast-transfer WS proxy: failed to connect upstream");
                return;
            }
        };

        let (mut client_tx, mut client_rx) = client_socket.split();
        let (mut upstream_tx, mut upstream_rx) = upstream.split();

        // Client → upstream
        let c2u = tokio::spawn(async move {
            while let Some(Ok(msg)) = client_rx.next().await {
                use tokio_tungstenite::tungstenite::Message as TMsg;
                let tung_msg = match msg {
                    axum::extract::ws::Message::Text(t) => TMsg::Text(t.to_string().into()),
                    axum::extract::ws::Message::Binary(b) => TMsg::Binary(b.to_vec().into()),
                    axum::extract::ws::Message::Ping(p) => TMsg::Ping(p.to_vec().into()),
                    axum::extract::ws::Message::Pong(p) => TMsg::Pong(p.to_vec().into()),
                    axum::extract::ws::Message::Close(_) => break,
                };
                if upstream_tx.send(tung_msg).await.is_err() {
                    break;
                }
            }
        });

        // Upstream → client
        let u2c = tokio::spawn(async move {
            while let Some(Ok(msg)) = upstream_rx.next().await {
                use tokio_tungstenite::tungstenite::Message as TMsg;
                let axum_msg = match msg {
                    TMsg::Text(t) => axum::extract::ws::Message::Text(t.to_string().into()),
                    TMsg::Binary(b) => axum::extract::ws::Message::Binary(b.to_vec().into()),
                    TMsg::Ping(p) => axum::extract::ws::Message::Ping(p.to_vec().into()),
                    TMsg::Pong(p) => axum::extract::ws::Message::Pong(p.to_vec().into()),
                    TMsg::Close(_) => break,
                    _ => continue,
                };
                if client_tx.send(axum_msg).await.is_err() {
                    break;
                }
            }
        });

        tokio::select! {
            _ = c2u => {},
            _ = u2c => {},
        }
    }))
}
