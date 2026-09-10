//! OBS WebSocket 5 protocol client.
//!
//! Drives the scene's *Media Source* via RPC commands. Concretely the engine
//! uses a small, focused set of requests (we never claim full coverage of the
//! 100+ op codes — see [the protocol reference](https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md)).
//!
//! Implemented requests (the only ones the scheduler needs):
//!   * `GetMediaInputStatus` — read `mediaDuration` and `mediaCursor` from
//!     the OBS *Media Source* we're driving.
//!   * `SetInputSettings` — swap the source's `local_file` to the next program.
//!   * `TriggerMediaInputAction { action: "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART" }` —
//!     seek-to-start + play.
//!   * `GetVersion` — sanity check / display.
//!
//! Note on framing: obs-websocket v5 uses small JSON messages (a few hundred
//! bytes for our use cases). tokio-tungstenite is overkill (it ships its own
//! framing state machine) but it gives us TLS for `wss://` for free, so we
//! use it consistently with the rest of the engine.

pub mod codec;
pub mod messages;

pub use codec::{ObsWsClient, ObsWsCmd};
pub use messages::{InputInfo, MediaInputAction, MediaInputStatus, ObsVersion, RequestId};

use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tracing::{info, warn};

use crate::config::ObsWsConfig;

/// Construct a connected, authenticated client. `ObsWsClient::connect` spawns
/// the background receive loop itself, so there is nothing else to wire up
/// here. Returns a handle the scheduler and admin endpoints share.
pub async fn connect(config: ObsWsConfig) -> anyhow::Result<Arc<ObsWsClient>> {
    let client = ObsWsClient::connect(&config).await?;
    info!("obs-websocket connected ({}:{})", config.host, config.port);
    Ok(client)
}

/// Shared handle that the rest of the engine uses to interact with OBS.
#[derive(Clone)]
pub struct ClientHandle {
    inner: Arc<Mutex<Option<Arc<ObsWsClient>>>>,
    last_attempt: Arc<Mutex<Option<DateTime<Utc>>>>,
}

impl ClientHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            last_attempt: Arc::new(Mutex::new(None)),
        }
    }
    pub fn set(&self, client: Arc<ObsWsClient>) {
        *self.inner.lock() = Some(client);
    }
    pub fn current(&self) -> Option<Arc<ObsWsClient>> {
        self.inner.lock().clone()
    }
    pub fn mark_attempt(&self, when: DateTime<Utc>) {
        *self.last_attempt.lock() = Some(when);
    }
    pub fn last_attempt(&self) -> Option<DateTime<Utc>> {
        *self.last_attempt.lock()
    }
}

impl Default for ClientHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Resilient connector: tries to (re)connect with exponential backoff.
/// Drives `ClientHandle::set` on success so other components can pick up the
/// new client without restarting.
pub async fn resilient_connector(cfg: ObsWsConfig, handle: ClientHandle, state: crate::AppState) {
    let mut backoff_ms = 500u64;
    let max_backoff_ms = 30_000u64;
    loop {
        handle.mark_attempt(Utc::now());
        match connect(cfg.clone()).await {
            Ok(client) => {
                handle.set(client.clone());
                backoff_ms = 500;
                // Mirror the connection state into AppStatus — the scheduler
                // gates every tick on `obs_connected`, so leaving it false
                // meant the scheduler never advanced at all.
                {
                    let mut st = state.status.write();
                    st.obs_connected = true;
                    st.obs_error = None;
                }
                info!("obs-websocket connected; holding until it drops");
                // Block until THIS client dies (OBS quit / network drop)
                // rather than reconnecting on a fixed 60s timer: the timer
                // churned a fresh TCP+WS connection every minute while the
                // previous one was still held alive by the scheduler.
                client.closed().await;
                {
                    let mut st = state.status.write();
                    st.obs_connected = false;
                    st.obs_error = Some("obs websocket disconnected".to_string());
                }
                warn!("obs-websocket disconnected; reconnecting");
            }
            Err(e) => {
                warn!(
                    "obs-websocket connect failed: {} (retry in {} ms)",
                    e, backoff_ms
                );
                {
                    let mut st = state.status.write();
                    st.obs_connected = false;
                    st.obs_error = Some(format!("{e}"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
            }
        }
    }
}
