//! The host binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use api::{DEFAULT_HTTP_PORT, build, build_state, load_config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn".into()),
        )
        .init();

    let content_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let config = load_config(&content_root)?;
    let environment = config.environment().clone();
    let port = config
        .get_string("Port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_HTTP_PORT);

    let state = build_state(config, &content_root).await?;
    let app = build(state);

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;

    tracing::info!(%address, environment = environment.name(), "listening");
    if environment.exposes_operational_metadata() {
        tracing::info!("OpenAPI document at http://localhost:{port}/swagger/v1/swagger.json");
    }

    // `into_make_service_with_connect_info` is what gives the rate limiter a
    // client address to partition on.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("the server stopped unexpectedly")?;

    Ok(())
}

/// Stops on Ctrl-C, so a container stop is not a kill.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "could not listen for the shutdown signal");
    }

    tracing::info!("shutting down");
}
