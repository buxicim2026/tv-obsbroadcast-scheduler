//! Library form of the engine so unit tests / integration tests can poke
//! individual modules without spawning the binary.

pub mod app_status;
pub mod config;
pub mod embedded;
pub mod interrupt;
pub mod media_probe;
pub mod obs_ws;
pub mod playlist;
pub mod scheduler;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::OnceCell;
use parking_lot::RwLock;

use crate::app_status::AppStatus;
use crate::config::Config;

/// Absolute path of the `config.json` in use, resolved once at startup by the
/// binary and then read by library code (e.g. the bootstrap handler, which
/// persists credentials after the C plugin hands them over).
static CONFIG_PATH: OnceCell<PathBuf> = OnceCell::new();

pub fn set_config_path(path: PathBuf) {
    let _ = CONFIG_PATH.set(path);
}

/// Returns the config path resolved at startup (falls back to a relative
/// `config.json` if `set_config_path` was never called).
pub fn config_path() -> PathBuf {
    CONFIG_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

/// Shared application state passed to every HTTP / WS handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    /// Live snapshot of engine health, surfaced via /api/status and /ws.
    pub status: Arc<RwLock<AppStatus>>,
    /// Notifies connected overlays / admin pages that the playlist or OBS
    /// state changed. Receivers should re-fetch whatever they need.
    pub notify: tokio::sync::broadcast::Sender<NotifyKind>,
}

#[derive(Debug, Clone, Copy)]
pub enum NotifyKind {
    PlaylistChanged,
    SchedulerStateChanged,
    ObsStatusChanged,
}

impl AppState {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        let (notify, _rx) = tokio::sync::broadcast::channel(64);
        Self {
            config,
            status: Arc::new(RwLock::new(AppStatus::default())),
            notify,
        }
    }
}
