//! REST endpoints for the playlist + scheduler state. Read-only endpoints
//! always use GET. Mutation endpoints (POST /api/playlist/item, etc.) all
//! require a valid bootstrap_token in the `X-Bootstrap-Token` header.
//!
//! The actual scheduler logic (advancing the timeline, switching OBS sources,
//! inserting bumpers) is filled in by `playlist-interrupt-probe`. This file
//! only owns the persistence shape so the admin UI can wire up its forms
//! against a stable contract.

use std::sync::Arc;

use axum::extract::Query;
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

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatPayload {
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderPayload {
    /// Program ids in the order they should air. Unknown ids keep their
    /// relative position at the end.
    pub ids: Vec<String>,
    /// Absolute start (epoch ms) for the first row. Omit to keep the current
    /// first row's start time.
    #[serde(default)]
    pub base_start_ms: Option<i64>,
    /// Re-base the whole list on "now" instead — this is what makes the
    /// schedule 顺延 / 提前 when the operator arms it earlier or later than
    /// the planned 开播时间.
    #[serde(default)]
    pub from_now: bool,
}

/// Re-sequence the playlist: reorder rows and lay them back-to-back so there
/// are no gaps or overlaps (which is what made rows air out of order).
pub async fn reorder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ReorderPayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_token(&state, &headers)?;
    let (items, ends_at) = {
        let mut cfg = state.config.write();
        let rank: std::collections::HashMap<String, usize> = payload
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let max_rank = rank.len();
        cfg.playlist.items.sort_by_key(|p| rank.get(&p.id).copied().unwrap_or(max_rank));

        let first_start = cfg
            .playlist
            .items
            .first()
            .map(|p| p.start_at_ms)
            .unwrap_or_else(chrono_now_ms);

        // Lay out back-to-back from the chosen base.
        let mut cursor = if payload.from_now {
            chrono_now_ms() + 1_000
        } else {
            payload.base_start_ms.unwrap_or(first_start)
        };
        for p in cfg.playlist.items.iter_mut() {
            p.start_at_ms = cursor;
            let dur = p
                .detected_duration_ms
                .filter(|d| *d > 0)
                .unwrap_or(p.declared_duration_ms);
            cursor = cursor.saturating_add(dur.max(1) as i64);
        }
        (cfg.playlist.items.len(), cursor)
    };
    let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    persist(state.config.clone()).await?;
    Ok(Json(json!({"ok": true, "items": items, "ends_at_ms": ends_at})))
}

/// Arm the scheduler AND re-base the list on the moment the operator actually
/// pressed the button (顺延 / 提前 vs. the planned 开播时间).
pub async fn start_scheduler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ReorderPayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_token(&state, &headers)?;
    {
        let mut cfg = state.config.write();
        let rank: std::collections::HashMap<String, usize> = payload
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let max_rank = rank.len();
        if !rank.is_empty() {
            cfg.playlist
                .items
                .sort_by_key(|p| rank.get(&p.id).copied().unwrap_or(max_rank));
        }
        let mut cursor = payload
            .base_start_ms
            .filter(|v| *v > 0)
            .unwrap_or_else(|| chrono_now_ms() + 1_000);
        for p in cfg.playlist.items.iter_mut() {
            p.start_at_ms = cursor;
            let dur = p
                .detected_duration_ms
                .filter(|d| *d > 0)
                .unwrap_or(p.declared_duration_ms);
            cursor = cursor.saturating_add(dur.max(1) as i64);
        }
        cfg.scheduler.enabled = true;
    }
    let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
    persist(state.config.clone()).await?;
    Ok(Json(json!({"ok": true, "enabled": true})))
}

/// Media / image extensions offered by the file browser.
const MEDIA_EXT: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "ts", "mxf", "mpg", "mpeg", "flv", "wmv", "webm", "m4v",
    "mp3", "wav", "aac", "flac",
];
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

/// Browse the *engine machine's* filesystem for media files.
///
/// A browser file picker only ever yields a bare file name — it never exposes
/// the absolute path — so joining it with a hand-typed folder produced paths
/// the engine could not open, and every imported row showed up as 异常. This
/// lets the operator pick the file through the engine, which knows the real
/// path.
pub async fn browse(Query(q): Query<BrowseQuery>) -> Json<Value> {
    let raw = q.path.clone().unwrap_or_default();

    // No path yet -> offer the entry points (drive letters on Windows).
    if raw.trim().is_empty() {
        let mut roots: Vec<Value> = Vec::new();
        if cfg!(windows) {
            for c in 'C'..='Z' {
                let p = format!("{c}:\\");
                if std::path::Path::new(&p).exists() {
                    roots.push(json!({ "name": p, "path": p }));
                }
            }
        } else {
            roots.push(json!({ "name": "/", "path": "/" }));
            if let Some(home) = dirs::home_dir() {
                let s = home.display().to_string();
                roots.push(json!({ "name": s.clone(), "path": s }));
            }
        }
        return Json(json!({ "path": "", "parent": Value::Null, "dirs": roots, "files": [] }));
    }

    let dir = std::path::PathBuf::from(&raw);
    if !dir.is_dir() {
        return Json(json!({
            "path": raw, "parent": Value::Null, "dirs": [], "files": [],
            "error": "不是有效目录"
        }));
    }

    let mut dirs: Vec<Value> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                dirs.push(json!({ "name": name, "path": p.display().to_string() }));
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if MEDIA_EXT.contains(&ext.as_str()) || IMAGE_EXT.contains(&ext.as_str()) {
                files.push(json!({
                    "name": name,
                    "path": p.display().to_string(),
                    "size": md.len()
                }));
            }
        }
    }
    let by_name = |a: &Value, b: &Value| {
        a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);

    Json(json!({
        "path": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "dirs": dirs,
        "files": files,
    }))
}

/// Check whether a set of paths exists on the engine machine. Used before
/// importing browser-picked files so we never create rows that are 异常.
pub async fn stat_paths(Json(payload): Json<StatPayload>) -> Json<Value> {
    let mut existing: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for p in payload.paths {
        if !p.trim().is_empty() && std::path::Path::new(&p).exists() {
            existing.push(p);
        } else {
            missing.push(p);
        }
    }
    Json(json!({ "existing": existing, "missing": missing }))
}

/// List OBS inputs so the admin can pick the real Media Source name.
pub async fn obs_inputs(State(state): State<Arc<AppState>>) -> Json<Value> {
    let handle = state.obs_client.lock().clone();
    let Some(h) = handle else {
        return Json(json!({"inputs": [], "error": "OBS 未连接"}));
    };
    let Some(client) = h.current() else {
        return Json(json!({"inputs": [], "error": "OBS 未连接"}));
    };
    match client.get_input_list().await {
        Ok(list) => Json(json!({"inputs": list})),
        Err(e) => Json(json!({"inputs": [], "error": e.to_string()})),
    }
}

/// Report rows whose media file is missing/blank so the admin can flag them
/// as 异常 before they are supposed to air.
pub async fn verify(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read();
    let missing: Vec<String> = cfg
        .playlist
        .items
        .iter()
        .filter(|p| {
            p.file_path.trim().is_empty() || !std::path::Path::new(&p.file_path).exists()
        })
        .map(|p| p.id.clone())
        .collect();
    Json(json!({"missing": missing}))
}

fn chrono_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
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
