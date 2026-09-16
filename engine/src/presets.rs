//! Playlist presets — save the schedule under a name and bring it back later.
//!
//! Two jobs in one file format:
//!   * a local library (`presets/*.json` next to config.json) so a channel can
//!     flip between, say, 平日版 and 周末版；
//!   * a portable file the operator can copy to a new machine — that is why the
//!     preset carries the scheduler/clock settings too, and why loading it
//!     *replaces* the current playlist instead of merging into it.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{BumperEntry, ClockConfig, ProgramEntry, SchedulerConfig};

/// Marker so a random JSON file can't be loaded as a playlist by accident.
pub const KIND: &str = "tvbs-playlist-preset";

fn default_kind() -> String {
    KIND.to_string()
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub saved_at: Option<String>,
    #[serde(default)]
    pub items: Vec<ProgramEntry>,
    #[serde(default)]
    pub bumpers: Vec<BumperEntry>,
    /// Optional, so a preset can also restore the "how" (lead-in, clock, …).
    #[serde(default)]
    pub scheduler: Option<SchedulerConfig>,
    #[serde(default)]
    pub clock: Option<ClockConfig>,
}

/// What the list endpoint returns: enough for the picker, no playlist bodies.
#[derive(Debug, Clone, Serialize)]
pub struct PresetSummary {
    pub name: String,
    pub items: usize,
    pub total_ms: u64,
    pub saved_at: Option<String>,
}

pub fn presets_dir() -> PathBuf {
    crate::config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("presets")
}

/// Turn an operator-supplied name into something safe to use as a file name.
/// Chinese is kept as-is (filesystems handle it fine); only path separators,
/// wildcards and control characters are replaced.
pub fn safe_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    // Only a name that is *entirely* dots is a problem; trimming dots from a
    // normal name would quietly rewrite what the operator typed.
    let stem = match trimmed {
        "" | "." | ".." => "preset".to_string(),
        other => other.to_string(),
    };
    stem.chars().take(80).collect()
}

pub fn preset_path(name: &str) -> PathBuf {
    presets_dir().join(format!("{}.json", safe_stem(name)))
}

pub fn save(preset: &Preset) -> Result<PathBuf> {
    let dir = presets_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create preset directory {}", dir.display()))?;
    let path = preset_path(&preset.name);
    let body = serde_json::to_string_pretty(preset).context("serialize preset")?;
    // Write-then-rename so a crash mid-save can't leave a truncated preset.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Preset> {
    let path = preset_path(name);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read preset {}", path.display()))?;
    let preset: Preset =
        serde_json::from_str(&raw).with_context(|| format!("parse preset {}", path.display()))?;
    if preset.kind != KIND {
        return Err(anyhow!(
            "这不是本插件的预设文件（kind={}，应为 {}）",
            preset.kind,
            KIND
        ));
    }
    Ok(preset)
}

pub fn delete(name: &str) -> Result<()> {
    let path = preset_path(name);
    std::fs::remove_file(&path).with_context(|| format!("delete {}", path.display()))
}

/// Every preset on disk, newest first, skipping anything unreadable.
pub fn list() -> Vec<PresetSummary> {
    let dir = presets_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PresetSummary> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(preset) = serde_json::from_str::<Preset>(&raw) else {
            continue;
        };
        let total_ms = preset
            .items
            .iter()
            .map(crate::playlist::effective_duration_ms)
            .sum();
        out.push(PresetSummary {
            name: preset.name.clone(),
            items: preset.items.len(),
            total_ms,
            saved_at: preset.saved_at.clone(),
        });
    }
    out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    out
}

/// Build a preset out of the live configuration.
pub fn from_config(cfg: &crate::config::Config, name: &str, with_settings: bool) -> Preset {
    Preset {
        kind: KIND.to_string(),
        version: 1,
        name: name.to_string(),
        saved_at: Some(chrono::Utc::now().to_rfc3339()),
        items: cfg.playlist.items.clone(),
        bumpers: cfg.playlist.bumpers.clone(),
        scheduler: if with_settings {
            Some(cfg.scheduler.clone())
        } else {
            None
        },
        clock: if with_settings {
            Some(cfg.clock.clone())
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_stem_strips_path_separators() {
        assert_eq!(safe_stem("晚间/新闻"), "晚间_新闻");
        assert_eq!(safe_stem("..\\..\\etc\\passwd"), ".._.._etc_passwd");
        assert_eq!(safe_stem("   "), "preset");
        assert_eq!(safe_stem(""), "preset");
    }

    #[test]
    fn safe_stem_keeps_chinese_and_trims() {
        assert_eq!(safe_stem(" 平日版 "), "平日版");
    }

    #[test]
    fn safe_stem_is_bounded() {
        let long = "节".repeat(500);
        assert!(safe_stem(&long).chars().count() <= 80);
    }

    #[test]
    fn preset_path_uses_the_preset_directory() {
        let p = preset_path("每周节目表");
        assert!(p.to_string_lossy().contains("presets"));
        assert!(p.to_string_lossy().ends_with("每周节目表.json"));
    }
}
