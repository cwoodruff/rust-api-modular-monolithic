//! The host binary.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use api::{DEFAULT_HTTP_PORT, HostOptions};
use shared_kernel::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn,sqlx=warn".into()),
        )
        .init();

    let content_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = AppConfig::load(&content_root).context("failed to load configuration")?;
    let environment = config.environment().clone();
    let port = config.get_or("Port", DEFAULT_HTTP_PORT);

    let host = api::build(config, HostOptions::new(&content_root)).await?;
    let hosted = host.start_hosted_services();

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;

    tracing::info!(%address, environment = environment.name(), "listening");
    if environment.exposes_operational_metadata() {
        tracing::info!("Swagger UI at http://localhost:{port}/swagger");
    }

    // `into_make_service_with_connect_info` is what gives the rate limiters a
    // client address to partition on.
    axum::serve(
        listener,
        host.router()
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("the server stopped unexpectedly")?;

    hosted.stop().await;
    Ok(())
}

/// Stops on Ctrl-C, so a container stop is not a kill.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "could not listen for the shutdown signal");
    }

    tracing::info!("shutting down");
}
