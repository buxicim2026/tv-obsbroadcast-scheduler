//! REST endpoints for the playlist + scheduler state. Read-only endpoints
//! always use GET. Mutation endpoints (POST /api/playlist/item, etc.) all
//! require a valid bootstrap_token in the `X-Bootstrap-Token` header.
//!
//! The actual scheduler logic (advancing the timeline, switching OBS sources,
//! inserting bumpers) is filled in by `playlist-interrupt-probe`. This file
//! only owns the persistence shape so the admin UI can wire up its forms
//! against a stable contract.

use std::sync::Arc;

use axum::{extract::State, http::{HeaderMap, StatusCode}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{app_status::AppStatus, AppState};

/// Mutation guard. The engine listens on loopback, but a browser page from
/// ANY origin could still drive it (the CORS layer used to allow every
/// origin), so writes require the bootstrap token the UI already knows.
///
/// If no token has been issued yet (fresh install, before the first
/// bootstrap) there is nothing to protect and the UI must be able to
/// initialise, so we let it through.
fn require_token(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let expected = match state.config.read().bootstrap_token.clone() {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()),
    };
    let provided = headers
        .get("X-Bootstrap-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing or invalid X-Bootstrap-Token".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct UpsertItem {
    pub id: String,
    pub name: String,
    pub file_path: String,
    pub start_at_ms: i64,
    pub declared_duration_ms: u64,
    #[serde(default)]
    pub detected_duration_ms: Option<u64>,
    pub kind: crate::config::ProgramKind,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteItem {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct EnableScheduler {
    pub enabled: bool,
}

pub async fn status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read().clone();
    let st = state.status.read().clone();
    Json(snapshot_value(&cfg, &st))
}

/// One canonical status shape shared by `GET /api/status` and the `/ws`
/// snapshot. The admin UI reads `scheduler` as the **runtime** scheduler
/// state (AppStatus) — a plain `merge_into_value` used to put the scheduler
/// *config* there instead, so every dashboard value came back empty/undefined.
fn snapshot_value(cfg: &crate::config::Config, st: &AppStatus) -> Value {
    json!({
        "kind": "snapshot",
        // Runtime scheduler state (what the UI renders).
        "scheduler": st,
        // Scheduler settings (lead-in / clock offset / missing-file policy).
        "scheduler_cfg": cfg.scheduler,
        "obs_ws": cfg.obs_ws,
        "target_input": cfg.target_input,
        "bootstrap_token": cfg.bootstrap_token,
        "playlist_size": cfg.playlist.items.len(),
        "bumpers_size": cfg.playlist.bumpers.len(),
    })
}

pub async fn get_playlist(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read();
    Json(json!({
        "items": cfg.playlist.items,
        "bumpers": cfg.playlist.bumpers,
    }))
}

pub async fn upsert_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(item): Json<UpsertItem>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_token(&state, &headers)?;
    let entry = crate::config::ProgramEntry {
        id: item.id.clone(),
        name: item.name,
        file_path: item.file_path,
        start_at_ms: item.start_at_ms,
        declared_duration_ms: item.declared_duration_ms,
        detected_duration_ms: item.detected_duration_ms,
        kind: item.kind,
        notes: item.notes,
    };
    {
        let mut cfg = state.config.write();
        if let Some(existing) = cfg.playlist.items.iter_mut().find(|p| p.id == entry.id) {
            *existing = entry.clone();
        } else {
            cfg.playlist.items.push(entry.clone());
        }
        cfg.playlist.items.sort_by_key(|p| p.start_at_ms);
    }
    let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    persist(state.config.clone()).await?;
    Ok(Json(json!({"ok": true, "id": entry.id})))
}

pub async fn delete_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<DeleteItem>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_token(&state, &headers)?;
    {
        let mut cfg = state.config.write();
        cfg.playlist.items.retain(|p| p.id != payload.id);
        cfg.playlist
            .bumpers
            .retain(|b| b.target_program_id != payload.id && b.content.id != payload.id);
    }
    let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    persist(state.config.clone()).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn enable_scheduler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<EnableScheduler>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_token(&state, &headers)?;
    {
        let mut cfg = state.config.write();
        cfg.scheduler.enabled = payload.enabled;
    }
    let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
    persist(state.config.clone()).await?;
    Ok(Json(json!({"ok": true, "enabled": payload.enabled})))
}

async fn persist(
    config: Arc<parking_lot::RwLock<crate::config::Config>>,
) -> Result<(), (StatusCode, String)> {
    let cfg_clone = config.read().clone();
    let path = crate::config_path();
    tokio::task::spawn_blocking(move || cfg_clone.save_atomic(&path))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join: {e}")))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("save: {e}")))?;
    Ok(())
}
