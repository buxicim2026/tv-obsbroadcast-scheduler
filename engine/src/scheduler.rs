//! Scheduler — the millisecond-precision engine that drives OBS broadcasts.
//!
//! Architecture in one paragraph:
//!   * A tokio task ticks every 50ms and walks the timeline.
//!   * At `start_at - lead_in_ms` it issues `SetInputSettings` so the next
//!     decoder is ready before the cut.
//!   * At `start_at` it issues `TriggerMediaInputAction::Restart`.
//!   * Bumpers (interstitials) live in `playlist.bumpers[]`; while in
//!     `Playing` we watch for a bumper's `at_into_program_ms` window, cut to
//!     the bumper, then cut back to the primary at the same offset.
//!
//! **Single source of truth for the state machine**: `run()` owns a
//! `SchedulerState` local (`machine`) and passes `&mut` into each tick. The
//! struct itself is stateless, which is why `Scheduler` is freely shareable
//! behind an `Arc`. Every transition writes the machine *and* mirrors the
//! human-readable label into `AppStatus` for the UI.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::config::{Config, MissingFilePolicy, ProgramEntry};
use crate::obs_ws::{ClientHandle, MediaInputAction, ObsWsClient};

const TICK_MS: u64 = 50;

/// Throttle for the "program vanished from the playlist" warning: without it
/// the scheduler emitted ~20 identical warnings per second forever.
static LAST_MISSING_WARN: AtomicI64 = AtomicI64::new(0);

/// Throttle for media duration probing (at most one probe attempt per 5s).
static LAST_PROBE_MS: AtomicI64 = AtomicI64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SchedulerState {
    /// Not armed. No timeline progression.
    Idle,
    /// Will fire `SetInputSettings(target_input, next)` at `fire_at_ms`.
    Armed { target_id: String, fire_at_ms: i64 },
    /// Currently driving a program. `started_at_ms` is when this program
    /// began playing and `end_at_ms` is the absolute wall-clock time it is
    /// considered over (an explicit value, because a bumper that replays the
    /// primary from the top shifts the real end time).
    Playing {
        program_id: String,
        started_at_ms: i64,
        end_at_ms: i64,
    },
    /// A bumper is on air in the middle of `primary_id`. When it ends we cut
    /// back to the primary at `return_to_ms` (its offset inside the primary).
    InterstitialPlaying {
        primary_id: String,
        bumper_id: String,
        return_to_ms: u64,
    },
    /// Last error; user must clear before we recover.
    Error { message: String },
}

impl SchedulerState {
    pub fn label(&self) -> &'static str {
        match self {
            SchedulerState::Idle => "Idle",
            SchedulerState::Armed { .. } => "Armed",
            SchedulerState::Playing { .. } => "Playing",
            SchedulerState::InterstitialPlaying { .. } => "Interstitial",
            SchedulerState::Error { .. } => "Error",
        }
    }
    pub fn is_active(&self) -> bool {
        !matches!(self, SchedulerState::Idle | SchedulerState::Error { .. })
    }
    pub fn current_program_id(&self) -> Option<&str> {
        match self {
            SchedulerState::Playing { program_id, .. } => Some(program_id.as_str()),
            SchedulerState::Armed { target_id, .. } => Some(target_id.as_str()),
            SchedulerState::InterstitialPlaying { primary_id, .. } => Some(primary_id.as_str()),
            SchedulerState::Idle | SchedulerState::Error { .. } => None,
        }
    }
}

/// Stateless scheduler driver. Holds only the OBS handle + target input name;
/// the live `SchedulerState` lives in `run()` (see module docs).
pub struct Scheduler {
    /// A *handle*, not a client: obs-websocket reconnects produce a new
    /// client, and holding one `Arc<ObsWsClient>` forever meant the scheduler
    /// kept talking to a dead socket after OBS restarted.
    pub obs: ClientHandle,
    pub target_input: String,
}

impl Scheduler {
    pub fn new(obs: ClientHandle, target_input: String) -> Self {
        Self { obs, target_input }
    }

    /// Current live OBS client, or `None` while disconnected.
    fn client(&self) -> Option<Arc<ObsWsClient>> {
        self.obs.current()
    }

    /// Public entry — called from main.rs once we have an obs-ws handle.
    /// Keeps ticking forever; relies on caller to drop it on shutdown.
    pub async fn run(self: Arc<Self>, state: crate::AppState) {
        info!("scheduler loop starting");
        // The one and only state machine instance for this run.
        let mut machine = SchedulerState::Idle;

        let mut tick = interval(Duration::from_millis(TICK_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Rebuild the config snapshot about once per second instead of on
        // every 50ms tick. Cloning `Config` deep-copies every playlist item
        // and bumper (all their Strings); doing that 20x/s was pure
        // allocation churn with no behavioural benefit.
        let mut ticks: u64 = 0;
        let mut cfg_snapshot = Arc::new(state.config.read().clone());
        let mut prev_active = false;
        let mut error_since: Option<i64> = None;
        loop {
            tick.tick().await;
            ticks += 1;
            if ticks % 20 == 0 {
                cfg_snapshot = Arc::new(state.config.read().clone());
            }
            // Snapshot config so we don't hold the lock across awaits.
            // Take the two locks one at a time (never nested) to keep the
            // lock ordering consistent with the rest of the engine.
            let enabled = state.config.read().scheduler.enabled;
            let connected = state.status.read().obs_connected;
            let want_running = enabled && connected;
            if !want_running {
                if machine.is_active() {
                    self.become_idle(&state, &mut machine);
                }
                continue;
            }

            // Run a single tick under a deterministic state evolution.
            self.tick_once(&state, &cfg_snapshot, &mut machine).await;

            // Mirror "is the scheduler actively driving" into AppStatus — it
            // was never written before, so the UI always showed it idle.
            let active = machine.is_active();
            if active != prev_active {
                prev_active = active;
                let mut st = state.status.write();
                st.scheduler_running = active;
                st.scheduler_state = machine.label().to_string();
            }

            // Auto-recover from a sticky error. `SchedulerState::Error` had no
            // clearing endpoint, so a single transient failure (missing file,
            // one failed RPC) froze the broadcast forever with no way back
            // except restarting the engine.
            if matches!(machine, SchedulerState::Error { .. }) {
                let now = Utc::now().timestamp_millis();
                let since = *error_since.get_or_insert(now);
                if now - since > 10_000 {
                    warn!("scheduler stuck in Error for >10s; resetting to Idle");
                    machine = SchedulerState::Idle;
                    error_since = None;
                    let mut st = state.status.write();
                    st.scheduler_running = false;
                    st.scheduler_state = "Idle".to_string();
                    prev_active = false;
                }
            } else {
                error_since = None;
            }
        }
    }

    async fn tick_once(&self, state: &crate::AppState, cfg: &Config, machine: &mut SchedulerState) {
        let now_ms = crate::playlist::effective_now_ms(&cfg.scheduler);

        match machine {
            SchedulerState::Idle => {
                // Find the next program; if we're inside one already (rare
                // path: state was reset by a crash but the file is on air),
                // jump into Playing immediately.
                if let Some(p) = crate::playlist::current_program(cfg, now_ms) {
                    self.begin_playing(state, cfg, p, now_ms, machine).await;
                } else if let Some(next) = crate::playlist::next_program(cfg, now_ms) {
                    let fire_at = next.start_at_ms - cfg.scheduler.lead_in_ms as i64;
                    self.transition_to_armed(state, next, fire_at, machine);
                }
            }
            SchedulerState::Armed {
                target_id,
                fire_at_ms,
            } => {
                let target_id = target_id.clone();
                let fire_at = *fire_at_ms;
                if let Some(p) = cfg.playlist.items.iter().find(|p| p.id == target_id) {
                    if now_ms >= fire_at {
                        // Issue SetInputSettings NOW so OBS has the decoder
                        // warm by the time we RESTART.
                        if let Err(e) = self.preload(p).await {
                            self.transition_to_error(state, &format!("preload: {e}"), machine);
                            return;
                        }
                    }
                    if now_ms >= p.start_at_ms {
                        match self.cut_to(cfg, p).await {
                            Ok(true) => {
                                // Missing file + SkipToNext: don't start it,
                                // re-arm for the following program.
                                warn!("skipping missing program {} ({})", p.name, p.file_path);
                                match crate::playlist::next_program(
                                    cfg,
                                    p.start_at_ms.saturating_add(1),
                                ) {
                                    Some(next) => {
                                        let fire_at = next
                                            .start_at_ms
                                            .saturating_sub(cfg.scheduler.lead_in_ms as i64);
                                        self.transition_to_armed(state, next, fire_at, machine);
                                    }
                                    None => self.become_idle(state, machine),
                                }
                                return;
                            }
                            Ok(false) => {}
                            Err(e) => {
                                self.transition_to_error(state, &format!("cut: {e}"), machine);
                                return;
                            }
                        }
                        self.begin_playing(state, cfg, p, now_ms, machine).await;
                    }
                } else {
                    // Target disappeared (playlist mutated); revert to Idle.
                    self.become_idle(state, machine);
                }
            }
            SchedulerState::Playing {
                program_id,
                started_at_ms,
                end_at_ms,
            } => {
                let program_id = program_id.clone();
                let started = *started_at_ms;
                let end = *end_at_ms;

                let Some(p) = cfg.playlist.items.iter().find(|p| p.id == program_id) else {
                    // Program was deleted from the playlist while on air. Hold,
                    // but log at most once every 5s instead of every tick.
                    let now = Utc::now().timestamp_millis();
                    let last = LAST_MISSING_WARN.load(Ordering::Relaxed);
                    if now - last > 5_000 {
                        LAST_MISSING_WARN.store(now, Ordering::Relaxed);
                        warn!(
                            "current program id={} no longer exists; holding",
                            program_id
                        );
                    }
                    return;
                };

                let elapsed = now_ms.saturating_sub(started);
                let into_program_ms = now_ms.saturating_sub(p.start_at_ms).max(0);
                let lead_in = cfg.scheduler.lead_in_ms;

                // First time this program airs: once OBS has the file open,
                // read the real duration back so the UI can show it.
                if p.detected_duration_ms.is_none() && into_program_ms > 1500 {
                    self.probe_duration(state, p).await;
                }

                // ---- Bumper detection -------------------------------------
                if crate::interrupt::ready_to_fire(
                    &cfg.playlist.bumpers,
                    p,
                    into_program_ms,
                    lead_in,
                ) {
                    if let Some(b) = cfg
                        .playlist
                        .bumpers
                        .iter()
                        .find(|b| b.target_program_id == p.id)
                    {
                        if let Some(content) = crate::interrupt::pick_bumper_content(b) {
                            info!(
                                "firing bumper {} at +{}ms into {}",
                                content.name, into_program_ms, p.name
                            );
                            let Some(client) = self.client() else {
                                warn!("bumper skipped: no obs-websocket connection");
                                return;
                            };
                            match crate::interrupt::trigger_bumper(&client, &self.target_input, b)
                                .await
                            {
                                Ok(()) => {
                                    let return_to = crate::interrupt::return_to_ms(b);
                                    self.transition_to_interstitial(
                                        state,
                                        &p.id,
                                        &b.id,
                                        return_to,
                                        &content.name,
                                        machine,
                                    );
                                    return;
                                }
                                Err(e) => warn!("bumper fire failed: {e}"),
                            }
                        }
                    }
                }

                if elapsed < 0 {
                    // Clock went backwards (NTP step / user offset). Stay put.
                    warn!("clock skew negative on Playing; holding program {}", p.name);
                } else if now_ms >= end {
                    // Find next program.
                    match crate::playlist::next_program(cfg, now_ms) {
                        Some(next) => {
                            self.preload_if_soon(next, cfg, now_ms);
                            let fire_at = next
                                .start_at_ms
                                .saturating_sub(cfg.scheduler.lead_in_ms as i64);
                            self.transition_to_armed(state, next, fire_at, machine);
                        }
                        None => {
                            // Schedule ended; loop in Idle until user re-arms.
                            info!("schedule exhausted past {}", p.name);
                            self.become_idle(state, machine);
                        }
                    }
                } else {
                    self.refresh_remaining(state, p, end.saturating_sub(now_ms));
                }
            }
            SchedulerState::InterstitialPlaying {
                primary_id,
                bumper_id,
                return_to_ms,
            } => {
                let primary_id = primary_id.clone();
                let bumper_id = bumper_id.clone();
                let _return_to = *return_to_ms;

                let (Some(p), Some(b)) = (
                    cfg.playlist.items.iter().find(|p| p.id == primary_id),
                    cfg.playlist.bumpers.iter().find(|b| b.id == bumper_id),
                ) else {
                    // Bumper or primary vanished from the playlist; bail to Idle.
                    self.become_idle(state, machine);
                    return;
                };

                let bumper_dur = b.content.declared_duration_ms as i64;
                let bumper_start_ms = p.start_at_ms.saturating_add(b.at_into_program_ms as i64);
                let bumper_end_ms = bumper_start_ms.saturating_add(bumper_dur);

                if now_ms >= bumper_end_ms {
                    let Some(client) = self.client() else {
                        warn!("resume primary skipped: no obs-websocket connection");
                        return;
                    };
                    if let Err(e) =
                        crate::interrupt::trigger_resume_primary(&client, &self.target_input, p)
                            .await
                    {
                        warn!("resume primary failed: {e}");
                    }
                    // obs-websocket has no "seek" action for a Media Source,
                    // so resuming the primary restarts it from the top. That
                    // means the FULL declared duration is still ahead of us,
                    // and the end time moves out by the same amount — using
                    // `remaining_after_bumper` here desynced the state machine
                    // from what OBS was actually playing.
                    let primary_declared = p.declared_duration_ms as i64;
                    let end_at = now_ms.saturating_add(primary_declared);
                    self.transition_to_playing(
                        state,
                        &p.id,
                        &p.name,
                        primary_declared,
                        now_ms,
                        end_at,
                        machine,
                    );
                } else {
                    self.refresh_remaining(state, p, bumper_end_ms.saturating_sub(now_ms));
                }
            }
            SchedulerState::Error { .. } => {
                // No automatic recovery; user clears via /api/scheduler/clear-error.
            }
        }
    }

    /* ------------------------- transitions ------------------------- */

    /// Single write path: mutate the machine, then mirror the label + notify
    /// the UI. Keeping this in one place is what makes "one source of truth"
    /// actually true.
    fn apply_state(
        &self,
        state: &crate::AppState,
        machine: &mut SchedulerState,
        new_state: SchedulerState,
    ) {
        *machine = new_state;
        let mut st = state.status.write();
        st.scheduler_state = machine.label().to_string();
        st.last_changed_at = Some(Utc::now());
        drop(st);
        let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
    }

    fn transition_to_armed(
        &self,
        state: &crate::AppState,
        p: &ProgramEntry,
        fire_at_ms: i64,
        machine: &mut SchedulerState,
    ) {
        debug!(
            "transition -> Armed for {} at fire_at {}",
            p.name, fire_at_ms
        );
        self.apply_state(
            state,
            machine,
            SchedulerState::Armed {
                target_id: p.id.clone(),
                fire_at_ms,
            },
        );
    }

    fn transition_to_interstitial(
        &self,
        state: &crate::AppState,
        primary_id: &str,
        bumper_id: &str,
        return_to_ms: u64,
        bumper_name: &str,
        machine: &mut SchedulerState,
    ) {
        self.apply_state(
            state,
            machine,
            SchedulerState::InterstitialPlaying {
                primary_id: primary_id.to_string(),
                bumper_id: bumper_id.to_string(),
                return_to_ms,
            },
        );
        let mut st = state.status.write();
        st.current_program_name = Some(bumper_name.to_string());
    }

    fn transition_to_playing(
        &self,
        state: &crate::AppState,
        program_id: &str,
        program_name: &str,
        remaining_ms: i64,
        now_ms: i64,
        end_at_ms: i64,
        machine: &mut SchedulerState,
    ) {
        self.apply_state(
            state,
            machine,
            SchedulerState::Playing {
                program_id: program_id.to_string(),
                started_at_ms: now_ms,
                end_at_ms,
            },
        );
        let mut st = state.status.write();
        st.current_program_id = Some(program_id.to_string());
        st.current_program_name = Some(program_name.to_string());
        st.current_remaining_ms = Some(remaining_ms);
    }

    fn become_idle(&self, state: &crate::AppState, machine: &mut SchedulerState) {
        self.apply_state(state, machine, SchedulerState::Idle);
    }

    fn transition_to_error(
        &self,
        state: &crate::AppState,
        msg: &str,
        machine: &mut SchedulerState,
    ) {
        warn!("scheduler error: {}", msg);
        self.apply_state(
            state,
            machine,
            SchedulerState::Error {
                message: msg.to_string(),
            },
        );
        let mut st = state.status.write();
        st.last_error = Some(msg.to_string());
    }

    async fn begin_playing(
        &self,
        state: &crate::AppState,
        _cfg: &Config,
        p: &ProgramEntry,
        now_ms: i64,
        machine: &mut SchedulerState,
    ) {
        debug!("begin_playing {} now={}", p.name, now_ms);
        let end = p.start_at_ms.saturating_add(p.declared_duration_ms as i64);
        self.transition_to_playing(
            state,
            &p.id,
            &p.name,
            end.saturating_sub(now_ms),
            now_ms,
            end,
            machine,
        );
    }

    fn refresh_remaining(&self, state: &crate::AppState, p: &ProgramEntry, remaining_ms: i64) {
        let mut st = state.status.write();
        st.current_program_id = Some(p.id.clone());
        st.current_program_name = Some(p.name.clone());
        st.current_remaining_ms = Some(remaining_ms);
    }

    /// Ask OBS how long the file really is and store it on the playlist entry.
    /// `media_probe` existed but was never called, so `detected_duration_ms`
    /// stayed empty forever.
    async fn probe_duration(&self, state: &crate::AppState, p: &ProgramEntry) {
        let now = Utc::now().timestamp_millis();
        if now - LAST_PROBE_MS.load(Ordering::Relaxed) < 5_000 {
            return;
        }
        LAST_PROBE_MS.store(now, Ordering::Relaxed);

        let Some(client) = self.client() else {
            return;
        };
        let Some(dur) =
            crate::media_probe::probe_duration_ms(&client, &self.target_input).await
        else {
            return;
        };
        let mut cfg = state.config.write();
        if let Some(entry) = cfg.playlist.items.iter_mut().find(|e| e.id == p.id) {
            entry.detected_duration_ms = Some(dur);
            info!("probed duration for {}: {}ms", entry.name, dur);
        }
        drop(cfg);
        let _ = state.notify.send(crate::NotifyKind::PlaylistChanged);
    }

    /* ------------------------- obs RPC wrappers ------------------------- */

    async fn preload(&self, p: &ProgramEntry) -> anyhow::Result<()> {
        if !std::path::Path::new(&p.file_path).exists() {
            return Err(anyhow::anyhow!("file not found: {}", p.file_path));
        }
        let settings = json!({
            "local_file": p.file_path,
            "is_local_file": true,
            "looping": false,
            "restart_on_activate": false,
            "close_when_inactive": true,
            "linear_alpha": 0,
            "speed_percent": 100,
            "clear_on_media_end": false
        });
        let client = self
            .client()
            .ok_or_else(|| anyhow::anyhow!("no obs-websocket connection"))?;
        client
            .set_input_settings(&self.target_input, settings, true)
            .await
    }

    async fn preload_if_soon(&self, p: &ProgramEntry, cfg: &Config, now_ms: i64) {
        let fire_at = p.start_at_ms.saturating_sub(cfg.scheduler.lead_in_ms as i64);
        if now_ms >= fire_at && now_ms < p.start_at_ms {
            if let Err(e) = self.preload(p).await {
                warn!("preload ahead-of-fire failed for {}: {}", p.name, e);
            }
        }
    }

    /// Hard-cut to `p`.
    ///
    /// Returns `Ok(true)` when the media file is missing and the configured
    /// policy is `SkipToNext` — i.e. the caller should NOT start this program
    /// and should re-arm for the next one instead. The policy used to be
    /// hard-coded, which made HoldFrame / StopScheduler dead settings.
    async fn cut_to(&self, cfg: &Config, p: &ProgramEntry) -> anyhow::Result<bool> {
        if !std::path::Path::new(&p.file_path).exists() {
            return match cfg.scheduler.on_missing_file {
                MissingFilePolicy::SkipToNext => Ok(true),
                MissingFilePolicy::HoldFrame => Err(anyhow::anyhow!(
                    "missing file and policy=HoldFrame: {}",
                    p.file_path
                )),
                MissingFilePolicy::StopScheduler => Err(anyhow::anyhow!(
                    "missing file and policy=StopScheduler: {}",
                    p.file_path
                )),
            };
        }
        let client = self
            .client()
            .ok_or_else(|| anyhow::anyhow!("no obs-websocket connection"))?;
        client
            .trigger_media_input_action(&self.target_input, MediaInputAction::Restart)
            .await?;
        debug!("triggered RESTART on {}", self.target_input);
        Ok(false)
    }
}
