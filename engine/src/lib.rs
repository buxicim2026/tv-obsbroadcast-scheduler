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

use std::sync::Arc;

use parking_lot::RwLock;

use crate::app_status::AppStatus;
use crate::config::Config;

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
