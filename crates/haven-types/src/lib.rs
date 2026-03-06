pub mod api;
pub mod events;

/// Placeholder JWT secrets that MUST NOT be used in production.
/// Both servers validate against this list at startup and exit if matched.
pub const PLACEHOLDER_SECRETS: &[&str] = &[
    "change-me-to-a-random-string",
    "dev-secret-change-me",
];

/// Listen for Ctrl+C / SIGTERM to trigger graceful shutdown.
/// Shared by both haven-server and haven-file-server.
pub async fn shutdown_signal() {
    use tracing::info;

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
