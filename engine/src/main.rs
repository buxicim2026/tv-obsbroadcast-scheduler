//! `tv-obsbroadcast-scheduler` —— Rust engine binary.
//!
//! Two long-running tasks:
//!   1. axum HTTP / WebSocket server on 127.0.0.1:8789
//!   2. The scheduler tick loop, which requires an OBS WebSocket connection
//!
//! Connecting to OBS is wrapped in `resilient_connector` so a transient
//! disconnect (OBS restart, server-side bounce) auto-recovers with backoff.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use parking_lot::RwLock;
use tracing::{info, warn};

use tvbs_engine::config::Config;
use tvbs_engine::embedded;
use tvbs_engine::server;
use tvbs_engine::{obs_ws, scheduler as sched, set_config_path, AppState};

/// Directory the engine binary lives in (used for portable-mode config / logs).
pub fn exe_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locate current executable")?;
    Ok(exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no parent directory"))?
        .to_path_buf())
}

pub fn resolve_config_path(cli_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli_path {
        return Ok(p);
    }
    let dir = exe_dir()?;
    let portable = dir.join("config.json");
    let probe = dir.join(".tvobs-write-probe");
    let writable = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map(|_| {
            let _ = std::fs::remove_file(&probe);
            true
        })
        .unwrap_or(false);
    if writable {
        return Ok(portable);
    }
    if let Some(mut user_dir) = dirs::config_dir() {
        user_dir.push("tv-obsbroadcast-scheduler");
        return Ok(user_dir.join("config.json"));
    }
    Ok(portable)
}

#[derive(Parser, Debug)]
#[command(
    name = "tv-obsbroadcast-scheduler",
    version,
    about = "TV-style broadcast scheduler engine for OBS Studio"
)]
struct Cli {
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    host: Option<String>,
    #[arg(long, short = 'p', global = true)]
    port: Option<u16>,
    #[arg(long)]
    open: bool,
    #[arg(long)]
    headless: bool,
    /// Audio mode override (kept for parity with stream-live-translate).
    #[arg(long, global = true)]
    audio_mode: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let cfg_path = resolve_config_path(cli.config.clone())?;
    set_config_path(cfg_path.clone());

    info!(
        "tv-obsbroadcast-scheduler engine starting; config = {}",
        cfg_path.display()
    );

    // Load (or initialize) config.
    let config = Config::load_or_init(&cfg_path).context("load config")?;
    let config = Arc::new(RwLock::new(config));

    let host = cli
        .host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = cli.port.unwrap_or(8789);

    // Shared application state — populated further by subsequent todos.
    let state = AppState::new(config.clone());

    // Start the obs-websocket connector (resilient; runs in background) and
    // the scheduler tick loop (also runs forever once an OBS client is up).
    {
        let cfg_obs = state.config.read().obs_ws.clone();
        let target_input = state.config.read().target_input.clone();
        let state_for_tasks = state.clone();
        tokio::spawn(async move {
            // The connector keeps trying with backoff; we just wait on its
            // side-channel and start the scheduler when a client is up.
            let handle = obs_ws::ClientHandle::new();
            tokio::spawn(obs_ws::resilient_connector(
                cfg_obs.clone(),
                handle.clone(),
            ));

            // Wait for the first successful connect.
            loop {
                if let Some(client) = handle.current() {
                    let scheduler = sched::Scheduler::new(client, target_input.clone());
                    scheduler.run(state_for_tasks.clone()).await;
                    // unreachable: scheduler.run is an infinite loop.
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
    }

    // Serve admin + overlay (admin/ and overlay/ are embedded at compile time).
    let dist = embedded::dist();
    let router = server::build_router(state.clone(), dist).into_make_service();
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind HTTP listener on {}", addr))?;
    info!("HTTP server listening on http://{}", addr);

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server crashed");

    if let Err(e) = result {
        warn!("server exit error: {:#}", e);
    }
    info!("engine exiting cleanly");
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tvbs_engine=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_ansi(false))
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("ctrl-c handler failed: {:#}", e);
        }
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("shutdown signal received");
}
