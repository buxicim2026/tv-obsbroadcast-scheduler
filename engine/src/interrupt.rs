//! Interstitial (bumper) scheduling — *decisions* and *RPC*, not state.
//!
//! This module answers three questions and nothing else:
//!   1. Is a bumper due right now?      -> `ready_to_fire`
//!   2. Which file should it play?      -> `pick_bumper_content`
//!   3. Which OBS commands do we send?  -> `trigger_bumper` / `trigger_resume_primary`
//!
//! The state machine transition itself (`Playing` -> `InterstitialPlaying`
//! -> `Playing`) is owned by `scheduler.rs`, which is the single source of
//! truth. Keeping them separate avoids two writers for one state.

use serde_json::json;
use tracing::{info, warn};

use crate::config::{BumperEntry, Config, ProgramEntry, ProgramKind};
use crate::obs_ws::{MediaInputAction, ObsWsClient};

/// Returns true if a bumper is due to fire for `p` at `now_ms_into_program`,
/// i.e. inside the lead-in window of `b.at_into_program_ms`.
pub fn ready_to_fire(
    bumpers: &[BumperEntry],
    p: &ProgramEntry,
    now_ms_into_program: i64,
    lead_in_ms: u64,
) -> bool {
    bumpers
        .iter()
        .filter(|b| b.target_program_id == p.id)
        .any(|b| {
            let offset = b.at_into_program_ms as i64;
            // Same window as the inter-cut: anywhere in [offset - lead_in, offset).
            now_ms_into_program >= offset - lead_in_ms as i64 && now_ms_into_program < offset
        })
}

/// The offset (inside the primary) to resume at once the bumper ends.
/// MVP: the bumper plays on top of the window that starts at its trigger
/// point, so we simply resume at that same trigger point.
pub fn return_to_ms(b: &BumperEntry) -> u64 {
    b.at_into_program_ms
}

/// How long the primary still has to run after the bumper finished.
pub fn remaining_after_bumper(p: &ProgramEntry, b: &BumperEntry) -> i64 {
    (p.declared_duration_ms as i64 - b.at_into_program_ms as i64).max(0)
}

/// Resolve the bumper's content file (or skip if missing / wrong kind).
/// We log loudly when skipping; the caller decides whether to advance or hold.
pub fn pick_bumper_content(b: &BumperEntry) -> Option<ProgramEntry> {
    if !std::path::Path::new(&b.content.file_path).exists() {
        warn!("bumper content missing: {}", b.content.file_path);
        return None;
    }
    if matches!(b.content.kind, ProgramKind::Primary) {
        warn!(
            "bumper {} has kind=Primary; expected Interstitial/Standalone",
            b.content.name
        );
        return None;
    }
    Some(b.content.clone())
}

/// The `SetInputSettings` payload pointing the target input at `file_path`.
pub fn settings_payload(file_path: &str) -> serde_json::Value {
    json!({
        "local_file": file_path,
        "is_local_file": true,
        "looping": false,
        "restart_on_activate": false,
        "close_when_inactive": true,
        "linear_alpha": 0,
        "speed_percent": 100,
        "clear_on_media_end": true,
    })
}

/// Point the target input at the bumper file and start it (hard cut).
pub async fn trigger_bumper(
    obs: &ObsWsClient,
    target_input: &str,
    b: &BumperEntry,
) -> anyhow::Result<()> {
    obs.set_input_settings(target_input, settings_payload(&b.content.file_path), true)
        .await?;
    obs.trigger_media_input_action(target_input, MediaInputAction::Restart)
        .await?;
    info!("bumper fired: id={} on input={}", b.id, target_input);
    Ok(())
}

/// Point the target input back at the primary and start it (hard cut).
pub async fn trigger_resume_primary(
    obs: &ObsWsClient,
    target_input: &str,
    primary: &ProgramEntry,
) -> anyhow::Result<()> {
    obs.set_input_settings(target_input, settings_payload(&primary.file_path), true)
        .await?;
    obs.trigger_media_input_action(target_input, MediaInputAction::Restart)
        .await?;
    info!(
        "resumed primary program: id={} ({})",
        primary.id, primary.name
    );
    Ok(())
}

/// Crash recovery: scan the playlist against the wall clock and figure out
/// where the engine should resume on restart.
pub enum Resume<'a> {
    /// We're inside this program's window; seek to `offset_ms` within it.
    LiveAt(&'a ProgramEntry, u64),
    /// The current slot already passed; jump to this one next.
    SkipTo(&'a ProgramEntry),
    /// A very long gap elapsed; don't sit around waiting, fast-forward.
    FastForward(&'a ProgramEntry),
}

pub fn recover<'a>(cfg: &'a Config, now_ms: i64) -> Option<Resume<'a>> {
    if !cfg.scheduler.enabled {
        return None;
    }
    // Case 1 — we're currently inside a program's window.
    if let Some(p) = crate::playlist::current_program(cfg, now_ms) {
        let elapsed = (now_ms - p.start_at_ms).max(0) as u64;
        return Some(Resume::LiveAt(p, elapsed));
    }
    // Case 2 — we missed the last program already.
    let next = crate::playlist::next_program(cfg, now_ms)?;
    let gap = next.start_at_ms - now_ms;
    if gap > 24 * 60 * 60 * 1000 {
        Some(Resume::FastForward(next))
    } else {
        Some(Resume::SkipTo(next))
    }
}
