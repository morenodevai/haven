use std::sync::Arc;

use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{DecodingKey, Validation, decode};

pub use haven_types::api::Claims;

/// Shared JWT secret injected via `axum::Extension` from the server entrypoint.
/// This avoids every middleware call reading from the environment independently.
#[derive(Clone)]
pub struct JwtSecret(pub Arc<str>);

/// Extract and validate JWT from Authorization header.
/// The signing secret is obtained from the `JwtSecret` extension layer,
/// NOT from environment variables — the server sets this once at startup.
pub async fn require_auth(mut req: Request, next: Next) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let secret = req
        .extensions()
        .get::<JwtSecret>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.0.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;

    req.extensions_mut().insert(token_data.claims);
    Ok(next.run(req).await)
}
