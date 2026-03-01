use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header, Method},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use futures_util::TryStreamExt;
use reqwest::Client;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};

// ── App state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    file_server_url: String, // e.g. "http://127.0.0.1:3211"
    jwt_secret: String,
    client: Client,
}

// ── JWT auth middleware ────────────────────────────────────────────────

async fn require_auth(
    State(state): State<AppState>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            warn!("request rejected: missing Authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
        warn!("request rejected: Authorization header not Bearer");
        StatusCode::UNAUTHORIZED
    })?;

    jsonwebtoken::decode::<haven_types::api::Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|e| {
        warn!(error = %e, "request rejected: JWT validation failed");
        StatusCode::UNAUTHORIZED
    })?;

    Ok(next.run(req).await)
}

// ── Generic proxy function ─────────────────────────────────────────────

/// Proxy an incoming request to the file server.
/// Streams both the request body and response body without buffering.
async fn proxy(
    state: &AppState,
    method: Method,
    path: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    let url = format!("{}{}", state.file_server_url, path);
    info!(method = %method, upstream = %url, "proxying request");

    // Build upstream request
    let mut builder = state.client.request(method.clone(), &url);

    // Forward Authorization header
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    // Forward Content-Type header
    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }

    // Stream request body to upstream
    let body_stream = body.into_data_stream();
    let reqwest_body = reqwest::Body::wrap_stream(body_stream);
    builder = builder.body(reqwest_body);

    let upstream_resp = builder.send().await.map_err(|e| {
        error!(error = %e, upstream = %url, "upstream request failed");
        StatusCode::BAD_GATEWAY
    })?;

    let status = StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    info!(upstream = %url, status = %status, "upstream response");

    // Build response with streamed body from upstream
    let mut response_builder = Response::builder().status(status);

    // Forward Content-Type from upstream
    if let Some(ct) = upstream_resp.headers().get(header::CONTENT_TYPE) {
        response_builder = response_builder.header(header::CONTENT_TYPE, ct);
    }
    // Forward Content-Length from upstream (important for streaming downloads)
    if let Some(cl) = upstream_resp.headers().get(header::CONTENT_LENGTH) {
        response_builder = response_builder.header(header::CONTENT_LENGTH, cl);
    }

    // Stream upstream response body back to client
    let stream = upstream_resp.bytes_stream().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, e)
    });
    let body = Body::from_stream(stream);

    response_builder.body(body).map_err(|e| {
        error!(error = %e, "failed to build response");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

// ── Route handlers ─────────────────────────────────────────────────────

async fn create_transfer(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::POST, "/transfers", &headers, body).await
}

async fn get_transfer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::GET, &format!("/transfers/{id}"), &headers, body).await
}

async fn upload_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::PUT, &format!("/transfers/{id}/data"), &headers, body).await
}

async fn upload_chunk(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::PUT, &format!("/transfers/{id}/chunks/{index}"), &headers, body).await
}

async fn download_data(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::GET, &format!("/transfers/{id}/data"), &headers, body).await
}

async fn confirm_transfer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::POST, &format!("/transfers/{id}/confirm"), &headers, body).await
}

async fn delete_transfer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, StatusCode> {
    proxy(&state, Method::DELETE, &format!("/transfers/{id}"), &headers, body).await
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "haven-file-gateway ok")
}

// ── Main ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env
    if let Err(e) = dotenvy::dotenv() {
        eprintln!("warning: .env not loaded: {e}");
    }

    // Tracing — stderr for unbuffered logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "haven_file_gateway=info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let port: u16 = std::env::var("HAVEN_FILE_GATEWAY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3212);

    let file_server_url = std::env::var("HAVEN_FILE_SERVER_INTERNAL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3211".to_string());

    let jwt_secret = std::env::var("HAVEN_JWT_SECRET").unwrap_or_default();
    if jwt_secret.is_empty() {
        eprintln!("FATAL: HAVEN_JWT_SECRET is unset. Set it in .env and restart.");
        std::process::exit(1);
    }

    info!(port, file_server_url = %file_server_url, "haven-file-gateway starting");

    let state = AppState {
        file_server_url,
        jwt_secret,
        client: Client::builder()
            .no_proxy()
            .build()?,
    };

    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/transfers", post(create_transfer))
        .route("/transfers/{id}", get(get_transfer))
        .route("/transfers/{id}", delete(delete_transfer))
        .route("/transfers/{id}/data", put(upload_data))
        .route("/transfers/{id}/data", get(download_data))
        .route("/transfers/{id}/chunks/{index}", put(upload_chunk))
        .route("/transfers/{id}/confirm", post(confirm_transfer))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .route("/health", get(health))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
