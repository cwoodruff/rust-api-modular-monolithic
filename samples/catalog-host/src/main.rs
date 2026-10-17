//! `cargo run -p catalog-host`: Catalog on http://localhost:5143, against the
//! compose database, accepting tokens from the monolith on port 5043.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use shared_kernel::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    // The sample's own appsettings.json sits beside its manifest.
    let content_root = std::env::var_os("CATALOG_HOST_CONTENT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    let config = AppConfig::load(&content_root).context("failed to load configuration")?;
    let port = config.get_or("Port", catalog_host::DEFAULT_HTTP_PORT);

    let host = catalog_host::build(config, &content_root)?;
    let hosted = host.start_hosted_services();

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;
    tracing::info!(%address, "Catalog.Host listening");

    axum::serve(
        listener,
        host.router()
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .context("the server stopped unexpectedly")?;

    hosted.stop().await;
    Ok(())
}
