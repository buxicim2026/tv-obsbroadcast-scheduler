//! Typed wrappers around the obs-websocket 5 request/response payloads.
//!
//! We use `serde_json::Value` for most fields because the protocol is large,
//! drift-tolerant, and we only care about a handful of values — typing every
//! field would mean tracking schema changes forever.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One enumeration value for the `action` field of TriggerMediaInputAction.
/// We only ever send `Restart` from the scheduler, but the enum is here for
/// future flexibility (e.g. a manual "pause" button in the admin UI).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MediaInputAction {
    Restart,
    Pause,
    Play,
    Stop,
    Next,
    Previous,
}

impl MediaInputAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaInputAction::Restart  => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART",
            MediaInputAction::Pause    => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PAUSE",
            MediaInputAction::Play     => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PLAY",
            MediaInputAction::Stop     => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_STOP",
            MediaInputAction::Next     => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_NEXT",
            MediaInputAction::Previous => "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_PREVIOUS",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInputStatus {
    #[serde(rename = "mediaState")]
    pub media_state: String,
    /// Milliseconds.
    #[serde(rename = "mediaDuration")]
    pub media_duration: f64,
    /// Milliseconds.
    #[serde(rename = "mediaCursor")]
    pub media_cursor: f64,
    #[serde(rename = "mediaKind")]
    pub media_kind: String,
}

#[derive(Debug, Clone)]
pub struct ObsVersion {
    pub obs_version: String,
    pub rpc_version: u32,
    pub platform: String,
}

impl<'a> From<&'a Value> for ObsVersion {
    fn from(v: &Value) -> Self {
        Self {
            obs_version: v.get("obsVersion").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            rpc_version: v.get("rpcVersion").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            platform: v.get("platform").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        }
    }
}
