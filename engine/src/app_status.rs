//! Global engine status snapshot — surfaced via `/api/status` and `/ws`.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct AppStatus {
    /// True if the engine has a working connection to OBS via WebSocket.
    pub obs_connected: bool,
    /// Last OBS WebSocket connection error (if any).
    pub obs_error: Option<String>,
    /// True if the user-enabled scheduler is actively driving the scene.
    pub scheduler_running: bool,
    /// Current scheduler state (mirrored from `SchedulerState` enum, kept as
    /// string for JSON friendliness).
    pub scheduler_state: String,
    /// ID of the program currently on air (or None if `Idle`).
    pub current_program_id: Option<String>,
    /// Name of the program currently on air (for quick admin display).
    pub current_program_name: Option<String>,
    /// Remaining ms of the current program (best-effort).
    pub current_remaining_ms: Option<i64>,
    /// Wall-clock timestamp of last state mutation.
    pub last_changed_at: Option<DateTime<Utc>>,
    /// Most recent failure (e.g., media file missing). Cleared by the user.
    pub last_error: Option<String>,
}
