//! Time-aware playlist queries over `Config::playlist`. The HTTP layer reads
//! / writes through `Config`, so this module does NOT own persistence —
//! it just answers "what's on air right now?", "what's next?", and validates
//! the schedule for sane values.
//!
//! All time inputs are Unix epoch *milliseconds* (UTC). Caller is responsible
//! for converting `OffsetDateTime`/`chrono::Utc` to ms first.

use chrono::{DateTime, TimeZone, Utc};

use crate::config::{BumperEntry, Config, ProgramEntry, ProgramKind, SchedulerConfig};

/// Returns the effective wall clock the scheduler should use.
/// Applies `scheduler.clock_offset_ms` so the user can align with an external
/// time base (e.g. broadcast clock).
pub fn effective_now_ms(cfg: &SchedulerConfig) -> i64 {
    let raw = Utc::now().timestamp_millis();
    raw + cfg.clock_offset_ms
}

/// `now_ms` falls in `[start_at_ms, start_at_ms + declared_duration_ms)`.
/// Returns the program that should currently be driving the scene, or `None`.
pub fn current_program<'a>(cfg: &'a Config, now_ms: i64) -> Option<&'a ProgramEntry> {
    cfg.playlist.items.iter().find(|p| in_window(p, now_ms))
}

/// Next program to switch to (strictly after `now_ms`). Returns `None` if
/// there are no more items past the current moment.
pub fn next_program<'a>(cfg: &'a Config, now_ms: i64) -> Option<&'a ProgramEntry> {
    cfg.playlist.items.iter().find(|p| p.start_at_ms > now_ms)
}

/// Returns the program whose [start_at, end_at) window ends closest to `now_ms`
/// from the future — i.e. the next "primary" anchor for switch timing.
pub fn next_within<'a>(cfg: &'a Config, now_ms: i64) -> Option<&'a ProgramEntry> {
    next_program(cfg, now_ms)
}

pub fn in_window(p: &ProgramEntry, now_ms: i64) -> bool {
    let end = p.start_at_ms.saturating_add(p.declared_duration_ms as i64);
    now_ms >= p.start_at_ms && now_ms < end
}

/// Returns the bumper that should fire inside `p` at or before `now_ms`,
/// based on `at_into_program_ms` relative offset. Caller passes `program_offset_ms`
/// = `now_ms - p.start_at_ms`.
pub fn bumper_for<'a>(
    bumpers: &'a [BumperEntry],
    p: &ProgramEntry,
    program_offset_ms: i64,
) -> Option<&'a BumperEntry> {
    bumpers
        .iter()
        .find(|b| b.target_program_id == p.id && b.at_into_program_ms as i64 == program_offset_ms)
}

/// Validate a schedule end-to-end. Used by admin UI / CLI on save.
pub fn validate(cfg: &Config) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    if cfg.playlist.items.is_empty() {
        errs.push("节目表为空，请至少添加一条节目".into());
        return errs;
    }
    let mut prev: Option<i64> = None;
    for (idx, p) in cfg.playlist.items.iter().enumerate() {
        if p.name.trim().is_empty() {
            errs.push(format!("第 {} 条: 名称不能为空", idx + 1));
        }
        if p.file_path.trim().is_empty() {
            errs.push(format!("第 {} 条: 文件路径不能为空", idx + 1));
        }
        if p.declared_duration_ms == 0 {
            errs.push(format!("第 {} 条: 时长必须 > 0", idx + 1));
        }
        if let Some(prev_start) = prev {
            if p.start_at_ms < prev_start {
                errs.push(format!(
                    "第 {} 条: 起始时间早于前一条，请按时间顺序编排",
                    idx + 1
                ));
            }
        }
        prev = Some(p.start_at_ms);

        // Cross-reference bumpers.
        for b in cfg
            .playlist
            .bumpers
            .iter()
            .filter(|b| b.target_program_id == p.id)
        {
            if b.at_into_program_ms >= p.declared_duration_ms {
                errs.push(format!(
                    "节目 '{}' 上的插播触发点 '{}' 超出节目时长",
                    p.name, b.content.name
                ));
            }
            if matches!(b.content.kind, ProgramKind::Primary) {
                errs.push(format!(
                    "插播 '{}' 类型不能是 Primary（只能是 Interstitial / Standalone）",
                    b.content.name
                ));
            }
        }
    }
    errs
}

/// Display helper: ms -> `HH:MM:SS.mmm`.
pub fn fmt_ms(ms: i64) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let abs = ms.unsigned_abs();
    let h = abs / 3_600_000;
    let m = (abs / 60_000) % 60;
    let s = (abs / 1000) % 60;
    let milli = abs % 1000;
    format!("{}{:02}:{:02}:{:02}.{:03}", sign, h, m, s, milli)
}

/// Display helper: epoch ms -> ISO 8601 local-zone string.
pub fn fmt_iso_local(ms: i64) -> String {
    let dt: DateTime<Utc> = Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| Utc.timestamp_millis_opt(0).unwrap());
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProgramKind;

    fn program(start: i64, dur: u64) -> ProgramEntry {
        ProgramEntry {
            id: uuid::Uuid::new_v4().to_string(),
            name: "p".into(),
            file_path: "/tmp/x.mp4".into(),
            start_at_ms: start,
            declared_duration_ms: dur,
            detected_duration_ms: None,
            kind: ProgramKind::Primary,
            notes: None,
        }
    }

    #[test]
    fn current_program_within_window() {
        let mut cfg = Config::default();
        cfg.playlist.items.push(program(0, 1000));
        cfg.playlist.items.push(program(1000, 1000));
        assert_eq!(current_program(&cfg, 500).map(|p| p.start_at_ms), Some(0));
        assert_eq!(
            current_program(&cfg, 1500).map(|p| p.start_at_ms),
            Some(1000)
        );
        // ProgramEntry has no PartialEq (and doesn't need one), so assert on
        // the Option directly rather than comparing against `None`.
        assert!(current_program(&cfg, 999_999).is_none());
    }

    #[test]
    fn next_program_does_not_include_now() {
        let mut cfg = Config::default();
        cfg.playlist.items.push(program(0, 1000));
        cfg.playlist.items.push(program(2000, 1000));
        assert_eq!(next_program(&cfg, 1500).map(|p| p.start_at_ms), Some(2000));
    }

    #[test]
    fn fmt_ms_handles_zero_and_negative() {
        assert_eq!(fmt_ms(0), "00:00:00.000");
        assert_eq!(fmt_ms(3_661_001), "01:01:01.001");
        assert_eq!(fmt_ms(-500), "-00:00:00.500");
    }
}
