use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::Parser;
use edgeagent_rs::{api, client, config::Config, identity::Identity, logging, state::AppState};
use tokio::net::TcpListener;
use tracing::{error, info};

#[derive(Debug, Parser)]
#[command(name = "edgeagent-rs", version, about = "Rust target-device agent")]
struct Cli {
    #[arg(
        long,
        env = "EDGEAGENT_CONFIG",
        default_value = "/etc/edgeagent/config.toml"
    )]
    config: PathBuf,

    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::init_console();
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    let identity = Identity::load(&config).context("load device identity")?;

    if cli.check {
        println!(
            "configuration valid: device={} generation={} farm={}",
            identity.device_id, config.device.generation, config.farm.base_url
        );
        return Ok(());
    }

    let state = Arc::new(AppState::new(config, identity)?);
    state.info(
        "edgeagent",
        "agent starting",
        serde_json::json!({
            "deviceId": state.identity.device_id,
            "generation": state.config.device.generation,
            "version": env!("CARGO_PKG_VERSION")
        }),
    );

    let bind: SocketAddr = state
        .config
        .server
        .bind
        .parse()
        .context("parse server.bind")?;
    let listener = TcpListener::bind(bind)
        .await
        .context("bind HTTP listener")?;
    info!(%bind, "edge agent HTTP listener started");

    let client_state = Arc::clone(&state);
    let client_task = tokio::spawn(async move { client::run(client_state).await });

    let server = axum::serve(
        listener,
        api::router(Arc::clone(&state)).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal());

    if let Err(server_error) = server.await {
        error!(%server_error, "HTTP server stopped unexpectedly");
    }
    state.shutdown.cancel();
    if let Err(join_error) = client_task.await {
        error!(%join_error, "FarmController client task failed");
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
