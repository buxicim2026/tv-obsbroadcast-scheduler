//! Library form of the engine so unit tests / integration tests can poke
//! individual modules without spawning the binary.

pub mod app_status;
pub mod bridge;
pub mod config;
pub mod embedded;
pub mod interrupt;
pub mod media_probe;
pub mod ntp;
pub mod obs_ws;
pub mod playlist;
pub mod scheduler;
pub mod server;

use std::collections::VecDeque;
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
    /// Handle to the live obs-websocket client (set once the connector is up).
    /// Lets HTTP handlers query OBS itself — e.g. list the inputs so the admin
    /// can pick the target Media Source by name instead of typing it.
    pub obs_client: Arc<parking_lot::Mutex<Option<crate::obs_ws::ClientHandle>>>,
    /// Transport-panel commands (pause / resume / next / reload) handed to the
    /// scheduler loop. The HTTP layer only *enqueues*; the scheduler owns the
    /// state machine and is the only place that may mutate it, so buttons can
    /// never race with a tick.
    pub control: Arc<parking_lot::Mutex<VecDeque<ControlCommand>>>,
}

/// What the transport buttons in the admin console ask the scheduler to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    /// Freeze the timeline and pause the media source in OBS.
    Pause,
    /// Undo `Pause`: resume OBS playback and re-anchor the end time.
    Resume,
    /// Cut to the next program immediately, discarding the rest of this one.
    Next,
    /// Re-read `config.json` from disk and re-anchor the timeline to now.
    Reload,
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
            obs_client: Arc::new(parking_lot::Mutex::new(None)),
            control: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
        }
    }
}
