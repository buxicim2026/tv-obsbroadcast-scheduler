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
///
/// Two corrections stack on top of the local clock:
///   * `scheduler.clock_offset_ms` — the operator's manual trim against an
///     external master clock;
///   * the NTP offset — measured against 国家授时中心, so a wrong PC clock
///     (flat battery, bad manual change, broken domain controller) no longer
///     moves the schedule or the on-screen 报时 off true time.
pub fn effective_now_ms(cfg: &SchedulerConfig) -> i64 {
    let raw = Utc::now().timestamp_millis();
    raw.saturating_add(cfg.clock_offset_ms)
        .saturating_add(crate::ntp::sane_offset(crate::ntp::ntp_offset_ms()))
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

/// The duration we actually schedule against: whatever OBS reported once the
/// file really played, falling back to the operator's declared estimate.
///
/// Keying everything to `declared_duration_ms` is what made programmes lose
/// their last second — the declared value is an estimate, so we cut away while
/// the real file still had time left on it.
pub fn effective_duration_ms(p: &ProgramEntry) -> u64 {
    match p.detected_duration_ms {
        Some(d) if d > 0 => d,
        _ => p.declared_duration_ms,
    }
}

/// Absolute wall-clock end of `p`, from its effective duration.
pub fn end_at_ms(p: &ProgramEntry) -> i64 {
    p.start_at_ms
        .saturating_add(effective_duration_ms(p) as i64)
}

pub fn in_window(p: &ProgramEntry, now_ms: i64) -> bool {
    now_ms >= p.start_at_ms && now_ms < end_at_ms(p)
}

/// Push every entry from `from_index` onward by `delta_ms` (negative moves them
/// earlier).
///
/// This is what keeps a manual pause from leaving dead air: everything still to
/// come slides by exactly the time the operator held the transport, instead of
/// keeping its original slot and being cut off when we catch up to it.
pub fn shift_from(items: &mut [ProgramEntry], from_index: usize, delta_ms: i64) {
    for p in items.iter_mut().skip(from_index) {
        p.start_at_ms = p.start_at_ms.saturating_add(delta_ms);
    }
}

/// Lay entries out back-to-back from `from_index` onward, the first one landing
/// on `base_ms`.
///
/// Used after a manual skip: the rest of the day closes up behind the cut so the
/// discarded programme doesn't leave a hole in the schedule (which would show as
/// black until the next advertised start time).
pub fn resequence_from(items: &mut [ProgramEntry], from_index: usize, base_ms: i64) {
    let mut cursor = base_ms;
    for p in items.iter_mut().skip(from_index) {
        p.start_at_ms = cursor;
        cursor = cursor.saturating_add(effective_duration_ms(p).max(1) as i64);
    }
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

    #[test]
    fn detected_duration_wins_over_declared() {
        // A video that is really 62s long but was declared as 60s must not be
        // cut away two seconds early.
        let mut p = program(0, 60_000);
        assert_eq!(effective_duration_ms(&p), 60_000);
        p.detected_duration_ms = Some(62_000);
        assert_eq!(effective_duration_ms(&p), 62_000);
        assert_eq!(end_at_ms(&p), 62_000);
        // A failed probe must clear back to the declared value, not to 0.
        p.detected_duration_ms = Some(0);
        assert_eq!(effective_duration_ms(&p), 60_000);
    }

    #[test]
    fn shift_from_moves_only_the_tail() {
        let mut cfg = Config::default();
        cfg.playlist.items.push(program(0, 1_000));
        cfg.playlist.items.push(program(1_000, 1_000));
        cfg.playlist.items.push(program(2_000, 1_000));
        shift_from(&mut cfg.playlist.items, 1, 5_000);
        let starts: Vec<i64> = cfg.playlist.items.iter().map(|p| p.start_at_ms).collect();
        assert_eq!(starts, vec![0, 6_000, 7_000]);
    }

    #[test]
    fn resequence_from_closes_the_gap() {
        let mut cfg = Config::default();
        cfg.playlist.items.push(program(0, 1_000));
        cfg.playlist.items.push(program(9_000, 2_000));
        cfg.playlist.items.push(program(20_000, 3_000));
        resequence_from(&mut cfg.playlist.items, 1, 5_000);
        let starts: Vec<i64> = cfg.playlist.items.iter().map(|p| p.start_at_ms).collect();
        assert_eq!(starts, vec![0, 5_000, 7_000]);
    }
}
