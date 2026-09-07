//! Persistent config (JSON): obs-websocket credentials, scheduler parameters,
//! and the playlist state. Atomic load/save/merge to a single file.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub obs_ws: ObsWsConfig,
    /// Name of the OBS *Media Source* that the engine drives.
    #[serde(default = "default_target_input")]
    pub target_input: String,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub playlist: PlaylistState,
    /// Shared secret issued by the C plugin on first launch; protects the
    /// /api/bootstrap endpoint (which carries the WS password in plaintext).
    #[serde(default)]
    pub bootstrap_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsWsConfig {
    #[serde(default = "default_obs_host")]
    pub host: String,
    #[serde(default = "default_obs_port")]
    pub port: u16,
    #[serde(default)]
    pub password: Option<String>,
    /// TLS (obs-websocket 5 supports `wss://`). Local installs rarely need it.
    #[serde(default)]
    pub tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// How early (ms) before a program's start_at to send SetInputSettings so
    /// OBS has the new decoder ready by the time we trigger RESTART.
    #[serde(default = "default_lead_in_ms")]
    pub lead_in_ms: u64,
    /// Manual clock offset (ms) to align the scheduler with an external time
    /// base (e.g. a broadcast facility's central clock). Applied as
    /// `effective_now() = Utc::now() + clock_offset_ms`.
    #[serde(default)]
    pub clock_offset_ms: i64,
    /// Whether the user has armed the scheduler.
    #[serde(default)]
    pub enabled: bool,
    /// What to do if a program's file is missing / undecodable.
    #[serde(default)]
    pub on_missing_file: MissingFilePolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingFilePolicy {
    /// Skip to the next valid program (default).
    SkipToNext,
    /// Freeze on last frame / black and surface an error to the admin UI.
    HoldFrame,
    /// Pause the scheduler entirely until the user intervenes.
    StopScheduler,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaylistState {
    #[serde(default)]
    pub items: Vec<ProgramEntry>,
    #[serde(default)]
    pub bumpers: Vec<BumperEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramEntry {
    pub id: String,
    pub name: String,
    pub file_path: String,
    /// Unix epoch ms; absolute wall clock start time.
    pub start_at_ms: i64,
    pub declared_duration_ms: u64,
    #[serde(default)]
    pub detected_duration_ms: Option<u64>,
    pub kind: ProgramKind,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramKind {
    Primary,
    Interstitial,
    Standalone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BumperEntry {
    pub id: String,
    pub target_program_id: String,
    /// Offset (ms) into the target program at which the bumper plays.
    pub at_into_program_ms: u64,
    pub content: ProgramEntry,
}

fn default_obs_host() -> String {
    "127.0.0.1".to_string()
}
fn default_obs_port() -> u16 {
    4455
}
fn default_target_input() -> String {
    "main_media".to_string()
}
fn default_lead_in_ms() -> u64 {
    200
}

impl Default for MissingFilePolicy {
    fn default() -> Self {
        MissingFilePolicy::SkipToNext
    }
}

/// `#[serde(default)]` on `Config`'s fields requires every field type to
/// implement `Default`, and we want the *sensible* defaults here (not
/// `host: ""`, `port: 0`), so implement it by hand instead of deriving.
impl Default for ObsWsConfig {
    fn default() -> Self {
        Self {
            host: default_obs_host(),
            port: default_obs_port(),
            password: None,
            tls: false,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            lead_in_ms: default_lead_in_ms(),
            clock_offset_ms: 0,
            enabled: false,
            on_missing_file: MissingFilePolicy::default(),
        }
    }
}

impl Config {
    pub fn load_or_init(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw =
                fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
            let cfg: Config =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
            Ok(cfg)
        } else {
            let cfg = Config::default();
            cfg.save_atomic(path)?;
            Ok(cfg)
        }
    }

    pub fn save_atomic(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_string_pretty(self).context("serialize config.json")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let tmp = with_extension(path, "json.tmp");
        fs::write(&tmp, &data).with_context(|| format!("write tmp {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("rename tmp {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// In-memory patch: deserialize a JSON value and replace the existing
    /// struct. Caller is expected to hold a write lock on `AppState.config`.
    pub fn apply_patch(&mut self, patch: serde_json::Value) -> Result<()> {
        let cur = serde_json::to_value(&self)?;
        let merged = merge_json(cur, patch);
        *self = serde_json::from_value(merged)?;
        Ok(())
    }
}

fn merge_json(mut base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (&mut base, &patch) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                let cur = a.remove(k).unwrap_or(Value::Null);
                a.insert(k.clone(), merge_json(cur, v.clone()));
            }
            Value::Object(a.clone())
        }
        _ => patch,
    }
}

fn with_extension(path: &Path, ext: &str) -> PathBuf {
    let mut p = path.to_path_buf();
    p.set_extension(ext);
    p
}

impl Default for Config {
    fn default() -> Self {
        Self {
            obs_ws: ObsWsConfig {
                host: default_obs_host(),
                port: default_obs_port(),
                password: None,
                tls: false,
            },
            target_input: default_target_input(),
            scheduler: SchedulerConfig {
                lead_in_ms: default_lead_in_ms(),
                clock_offset_ms: 0,
                enabled: false,
                on_missing_file: MissingFilePolicy::default(),
            },
            playlist: PlaylistState::default(),
            bootstrap_token: None,
        }
    }
}
