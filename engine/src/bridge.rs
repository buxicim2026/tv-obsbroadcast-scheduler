//! File bridge between the OBS Lua script and the engine.
//!
//! Why this exists: on Windows, every `os.execute` / `io.popen` from Lua runs
//! through `cmd.exe`, and OBS is a GUI application with no console of its own,
//! so each call flashed a black console window. OBS startup used to pop two or
//! three of them (the readiness probe alone fired a `curl` every 250ms), and so
//! did shutdown.
//!
//! The script now never spawns a process for the routine work: it writes its
//! settings to `bridge.json` with plain file IO and reads `status.json` back to
//! see that we're alive. The engine watches the file and answers. The only
//! remaining subprocess is launching the engine itself, and only when it isn't
//! already running.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::AppState;

/// How often we look for a new `bridge.json`. Fast enough that a settings
/// change feels instant, slow enough to be free.
const POLL: Duration = Duration::from_millis(500);
/// How often we refresh `status.json`. The script only needs to know we're
/// alive within a few seconds, so once a second is plenty and keeps us off the
/// disk.
const STATUS_EVERY: Duration = Duration::from_millis(1_000);

/// What the OBS script writes for us. Every field is optional so a partially
/// filled file (or one written by an older script) still applies cleanly.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BridgePayload {
    #[serde(default)]
    pub bootstrap_token: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub tls: Option<bool>,
    #[serde(default)]
    pub target_input: Option<String>,
    /// Arm / disarm the scheduler (mirrors the script's checkbox).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Ask the engine to open the admin page in the default browser — otherwise
    /// the script would have to shell out just to launch a URL.
    #[serde(default)]
    pub open_admin: Option<bool>,
    /// Set when the operator pressed Test Connection: re-take the credentials
    /// from the script even though the panel has been in charge since the
    /// bootstrap.
    #[serde(default)]
    pub force: Option<bool>,
}

/// What we write back, so the script can prove we're alive and report the OBS
/// link without ever making an HTTP request.
#[derive(Debug, Clone, Serialize)]
pub struct BridgeStatus {
    /// Epoch ms. The script compares this against its own clock to decide
    /// whether an engine instance is already running.
    pub ts: i64,
    pub engine_version: String,
    pub obs_connected: bool,
    pub obs_error: Option<String>,
    pub scheduler_running: bool,
    pub scheduler_state: String,
    pub current_program_name: Option<String>,
    pub http_port: u16,
}

/// Candidate directories for the bridge files.
///
/// Two, because the engine may be in portable mode (config beside the exe) or
/// user mode (config under %APPDATA% / ~/.config), and Lua cannot tell which.
/// The script writes to both and we read whichever is newest.
pub fn bridge_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(p) = crate::config_path().parent() {
        dirs.push(p.to_path_buf());
    }
    // exe is <script dir>/engine/tv-obsbroadcast-scheduler[.exe]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(script_dir) = dir.parent() {
                dirs.push(script_dir.to_path_buf());
            }
        }
    }

    dirs.sort();
    dirs.dedup();
    dirs
}

fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Newest readable `bridge.json` across the candidate directories.
pub fn read_bridge() -> Option<BridgePayload> {
    let mut best: Option<(std::time::SystemTime, BridgePayload)> = None;
    for dir in bridge_dirs() {
        let path = dir.join("bridge.json");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<BridgePayload>(&raw) else {
            warn!("ignoring unreadable bridge file {}", path.display());
            continue;
        };
        let mtime = file_mtime(&path).unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, parsed));
        }
    }
    best.map(|(_, p)| p)
}

/// Write `status.json` (atomically, so a reader never sees a half file).
pub fn write_status(state: &AppState, http_port: u16) -> Result<()> {
    let st = state.status.read().clone();
    let payload = BridgeStatus {
        ts: chrono::Utc::now().timestamp_millis(),
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        obs_connected: st.obs_connected,
        obs_error: st.obs_error,
        scheduler_running: st.scheduler_running,
        scheduler_state: st.scheduler_state,
        current_program_name: st.current_program_name,
        http_port,
    };
    let json = serde_json::to_vec_pretty(&payload)?;
    for dir in bridge_dirs() {
        std::fs::create_dir_all(&dir).ok();
        let final_path = dir.join("status.json");
        let tmp_path = dir.join("status.json.tmp");
        if std::fs::write(&tmp_path, &json).is_err() {
            continue;
        }
        if std::fs::rename(&tmp_path, &final_path).is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
    }
    Ok(())
}

/// Background task: apply settings the script left for us, and keep
/// `status.json` fresh so the script knows not to launch a second engine.
pub async fn run(state: AppState, http_port: u16) {
    let mut last_seen: Option<std::time::SystemTime> = None;
    let mut since_status = std::time::Instant::now();
    loop {
        // Fresh bridge file? Apply it.
        let mut current: Option<std::time::SystemTime> = None;
        for dir in bridge_dirs() {
            let path = dir.join("bridge.json");
            if let Some(m) = file_mtime(&path) {
                if current.map(|c| m > c).unwrap_or(true) {
                    current = Some(m);
                }
            }
        }
        if let Some(m) = current {
            if last_seen.map(|l| m > l).unwrap_or(true) {
                last_seen = Some(m);
                if let Some(payload) = read_bridge() {
                    apply(&state, &payload, http_port);
                }
            }
        }

        if since_status.elapsed() >= STATUS_EVERY {
            since_status = std::time::Instant::now();
            if let Err(e) = write_status(&state, http_port) {
                debug!("bridge status write failed: {e:#}");
            }
        }

        tokio::time::sleep(POLL).await;
    }
}

fn apply(state: &AppState, p: &BridgePayload, http_port: u16) {
    let force = p.force == Some(true);
    let changed = {
        let mut cfg = state.config.write();
        let mut changed = false;

        // Credentials come from the script while the engine has none yet — or
        // when the operator explicitly pressed Test Connection (`force`).
        if !cfg.bootstrapped || force {
            if let Some(h) = p.host.as_ref() {
                cfg.obs_ws.host = h.clone();
            }
            if let Some(v) = p.port {
                cfg.obs_ws.port = v;
            }
            if let Some(v) = p.password.as_ref() {
                cfg.obs_ws.password = Some(v.clone());
            }
            if let Some(v) = p.tls {
                cfg.obs_ws.tls = v;
            }
            if let Some(v) = p.bootstrap_token.as_ref() {
                if !v.is_empty() {
                    cfg.bootstrap_token = Some(v.clone());
                }
            }
            cfg.bootstrapped = true;
            changed = true;
        }

        // Target source is handled separately, and much more conservatively: the
        // script's own default is a placeholder (`main_media`) that usually does
        // not exist in the operator's OBS. Copying that over a real choice is
        // exactly what produced "No source was found by the name of `main_media`".
        // So: only adopt the script's value when the panel has nothing usable
        // yet, or when the operator forced it from the script panel.
        if let Some(v) = p.target_input.as_ref() {
            let candidate = v.trim().to_string();
            let current = cfg.target_input.trim().to_string();
            let panel_has_none = current.is_empty() || current == "main_media";
            if !candidate.is_empty() && (panel_has_none || force) && candidate != current {
                info!(
                    "bridge: target media source '{}' -> '{}' (from the OBS script)",
                    current, candidate
                );
                cfg.target_input = candidate;
                changed = true;
            }
        }

        // The arm flag is only taken on an explicit request: the script's
        // checkbox defaults to off and would keep switching the channel off
        // right after the operator started it from the panel.
        if force {
            if let Some(v) = p.enabled {
                cfg.scheduler.enabled = v;
                changed = true;
            }
        }

        changed
    };

    if !changed {
        debug!("bridge: nothing to adopt from the OBS script (panel is the source of truth)");
    }

    if p.open_admin == Some(true) {
        open_browser(&format!("http://127.0.0.1:{http_port}/admin"));
        // Clear the flag so we don't re-open on every poll.
        for dir in bridge_dirs() {
            let path = dir.join("bridge.json");
            if let Ok(raw) = std::fs::read_to_string(&path) {
                if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("open_admin".into(), serde_json::Value::Bool(false));
                        let _ =
                            std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or(raw));
                    }
                }
            }
        }
    }

    // Nothing of ours changed: don't rewrite config.json (it would also clobber
    // settings the operator just saved in the panel).
    if changed {
        let cfg = state.config.read().clone();
        let path = crate::config_path();
        if let Err(e) = cfg.save_atomic(&path) {
            warn!("bridge: persist config failed: {e:#}");
        } else {
            info!("bridge: applied settings from the OBS script");
        }
    }
    let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
}

/// Open a URL in the user's browser without a console window (the engine is a
/// windowless binary, so a plain `cmd /c start` would flash one).
fn open_browser(url: &str) {
    let result = if cfg!(target_os = "windows") {
        // rundll32 has no console, so nothing flashes.
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map(|_| ())
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
    } else {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
    };
    if let Err(e) = result {
        warn!("open admin in browser failed: {e}");
    }
}
