//! `POST /api/bootstrap` — one-time handshake between the OBS C plugin and
//! the engine. Caches the OBS WebSocket credentials and the
//! bootstrap_token used to gate subsequent calls.

use std::sync::Arc;

use anyhow::anyhow;
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct BootstrapPayload {
    /// Shared secret matching `Config.bootstrap_token` (issued by the C
    /// plugin on first launch). If the engine has no token yet, any
    /// non-empty value is accepted (subsequent boots REQUIRE a match).
    pub bootstrap_token: String,
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub tls: bool,
    pub target_input: String,
    /// Optional scheduler settings the admin panel can save in one call.
    #[serde(default)]
    pub scheduler: Option<SchedulerPatch>,
    /// Optional station-clock settings (整点 / 半点报时).
    #[serde(default)]
    pub clock: Option<ClockPatch>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ClockPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub font_size_px: Option<u32>,
    #[serde(default)]
    pub bg_opacity_percent: Option<u8>,
    #[serde(default)]
    pub duration_s: Option<u32>,
    #[serde(default)]
    pub lead_s: Option<u32>,
    #[serde(default)]
    pub plate_style: Option<String>,
    #[serde(default)]
    pub position: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SchedulerPatch {
    #[serde(default)]
    pub lead_in_ms: Option<u64>,
    #[serde(default)]
    pub clock_offset_ms: Option<i64>,
    #[serde(default)]
    pub on_missing_file: Option<crate::config::MissingFilePolicy>,
}

pub async fn bootstrap(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BootstrapPayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Validate the bootstrap token if the engine already has one. An *empty*
    // token means "not issued yet" (e.g. the admin saved settings before the
    // OBS script ever bootstrapped) — accept and adopt the new one instead of
    // rejecting forever, which would lock the plugin out of its own engine.
    {
        let cfg = state.config.read();
        if let Some(expected) = cfg.bootstrap_token.as_ref() {
            if !expected.is_empty() && expected != &payload.bootstrap_token {
                return Err((StatusCode::UNAUTHORIZED, "bad bootstrap token".into()));
            }
        }
    }

    {
        let mut cfg = state.config.write();
        if cfg.bootstrap_token.is_none() {
            cfg.bootstrap_token = Some(payload.bootstrap_token.clone());
        }
        cfg.obs_ws.host = payload.host.clone();
        cfg.obs_ws.port = payload.port;
        cfg.obs_ws.password = payload.password.clone();
        cfg.obs_ws.tls = payload.tls;
        cfg.target_input = payload.target_input.clone();
        if let Some(s) = payload.scheduler {
            if let Some(v) = s.lead_in_ms {
                cfg.scheduler.lead_in_ms = v;
            }
            if let Some(v) = s.clock_offset_ms {
                cfg.scheduler.clock_offset_ms = v;
            }
            if let Some(v) = s.on_missing_file {
                cfg.scheduler.on_missing_file = v;
            }
        }
        if let Some(c) = payload.clock {
            if let Some(v) = c.enabled {
                cfg.clock.enabled = v;
            }
            if let Some(v) = c.font_family {
                cfg.clock.font_family = v;
            }
            if let Some(v) = c.font_size_px {
                cfg.clock.font_size_px = v.clamp(12, 400);
            }
            if let Some(v) = c.bg_opacity_percent {
                cfg.clock.bg_opacity_percent = v.min(100);
            }
            if let Some(v) = c.duration_s {
                cfg.clock.duration_s = v.clamp(5, 900);
            }
            if let Some(v) = c.lead_s {
                cfg.clock.lead_s = v.min(600);
            }
            if let Some(v) = c.plate_style {
                cfg.clock.plate_style = v;
            }
            if let Some(v) = c.position {
                cfg.clock.position = v;
            }
        }
    }

    // Best-effort: persist to disk now so a crash right after bootstrap still
    // preserves the credentials.
    let path = crate::config_path();
    let cfg = state.config.read().clone();
    if let Err(e) = cfg.save_atomic(&path) {
        let msg = format!("persist bootstrap config: {:#}", e);
        let _ = anyhow!(msg.clone());
        return Err((StatusCode::INTERNAL_SERVER_ERROR, msg));
    }

    Ok(Json(json!({"ok": true})))
}
