//! Media duration probe — fills `detected_duration_ms` for a program entry
//! the first time it plays, by reading `mediaDuration` from the OBS Media
//! Source via `GetMediaInputStatus`.
//!
//! Probing is cooperative: the scheduler calls `probe_and_store` once per
//! program the first time it transitions into `Playing`. The detected value
//! is stored on the in-memory `ProgramEntry` (and on disk by the next todo's
//! persistence step).

use std::sync::Arc;

use chrono::Utc;

use crate::obs_ws::ObsWsClient;

#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub detected_duration_ms: Option<f64>,
}

/// Ask OBS how long the file is, via the WebSocket. Returning `None` means
/// "OBS hasn't started the file yet, or it failed"; caller can retry.
pub async fn probe(obs: &Arc<ObsWsClient>, target_input: &str) -> anyhow::Result<ProbeOutcome> {
    let status = obs.get_media_input_status(target_input).await?;
    Ok(ProbeOutcome {
        detected_duration_ms: Some(status.media_duration),
    })
}

/// Convenience: probe now and return the rounded milliseconds ready to be
/// persisted on `ProgramEntry::detected_duration_ms`.
pub async fn probe_duration_ms(obs: &Arc<ObsWsClient>, target_input: &str) -> Option<u64> {
    match probe(obs, target_input).await {
        Ok(out) => out.detected_duration_ms.and_then(|f| {
            if f.is_finite() && f > 0.0 {
                Some(f.round() as u64)
            } else {
                None
            }
        }),
        Err(e) => {
            tracing::warn!("probe failed: {:#}", e);
            None
        }
    }
}

/// Helper for tests / callers that need a deterministic clock anchor.
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
