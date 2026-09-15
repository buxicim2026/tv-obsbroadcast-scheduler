//! Minimal NTP client — keeps the station clock honest.
//!
//! A broadcast clock must not depend on whatever the operator's PC thinks the
//! time is: a CMOS battery, a bad manual change or a broken domain controller
//! and the whole schedule (and the 报时器) is silently wrong. So we ask the
//! National Time Service Centre (中国科学院国家授时中心, `ntp.ntsc.ac.cn`) what
//! time it really is and remember the *offset* between that and this machine.
//!
//! Only the offset is used, never the absolute answer. That matters: if the
//! local clock is wrong, T1/T4 are wrong by the same amount, but
//! `offset = ((T2-T1) + (T3-T4)) / 2` cancels it out — so a machine whose clock
//! is off by an hour still produces a correct offset, and `now + offset` is
//! real time.
//!
//! Hand-rolled on purpose: it is ~100 lines of UDP and saves a dependency.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use tokio::net::UdpSocket;
use tracing::{info, warn};

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
const NTP_TO_UNIX_SECS: u64 = 2_208_988_800;
/// Leap indicator 0, version 4, mode 3 (client).
const HEADER_CLIENT: u8 = 0x23;
const PACKET_LEN: usize = 48;
const REPLY_TIMEOUT: Duration = Duration::from_secs(4);

/// Offset (ms) to add to this machine's clock to get true time. Process-wide so
/// `playlist::effective_now_ms` can pick it up without threading a new argument
/// through every scheduler call site.
static NTP_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

/// The offset currently in force. Positive means the local clock is behind.
pub fn ntp_offset_ms() -> i64 {
    NTP_OFFSET_MS.load(Ordering::Relaxed)
}

fn set_ntp_offset_ms(v: i64) {
    NTP_OFFSET_MS.store(v, Ordering::Relaxed);
}

/// A UDP host that may be missing its `:port`.
fn with_port(server: &str) -> String {
    if server.contains(':') {
        server.to_string()
    } else {
        format!("{server}:123")
    }
}

/// Unix ms -> NTP 64-bit timestamp (32 bits of seconds, 32 bits of fraction).
fn to_ntp_timestamp(unix_ms: i64) -> [u8; 8] {
    let secs = ((unix_ms / 1_000) as i128 + NTP_TO_UNIX_SECS as i128) as u64;
    let frac = (((unix_ms % 1_000) as i128) << 32) / 1_000;
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&(secs as u32).to_be_bytes());
    out[4..8].copy_from_slice(&(frac as u32).to_be_bytes());
    out
}

/// NTP 64-bit timestamp -> Unix ms.
fn from_ntp_timestamp(bytes: &[u8]) -> i128 {
    if bytes.len() < 8 {
        return 0;
    }
    let mut secs = [0u8; 4];
    let mut frac = [0u8; 4];
    secs.copy_from_slice(&bytes[0..4]);
    frac.copy_from_slice(&bytes[4..8]);
    let secs = u32::from_be_bytes(secs) as i128 - NTP_TO_UNIX_SECS as i128;
    let frac = u32::from_be_bytes(frac) as i128;
    secs * 1_000 + ((frac * 1_000) >> 32)
}

/// Ask one server how far off this machine is. Returns the offset in ms.
pub async fn query_offset_ms(server: &str) -> Result<i64> {
    let addr = tokio::net::lookup_host(with_port(server))
        .await
        .map_err(|e| anyhow!("resolve {server}: {e}"))?
        .next()
        .ok_or_else(|| anyhow!("resolve {server}: no address"))?;

    // Bind to the same address family as the server so we don't try to speak
    // IPv4 to an IPv6 host.
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)
        .await
        .map_err(|e| anyhow!("bind udp: {e}"))?;
    socket
        .connect(addr)
        .await
        .map_err(|e| anyhow!("connect: {e}"))?;

    // T1: our transmit timestamp, echoed back in the reply's Originate field.
    let t1 = Utc::now().timestamp_millis();
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = HEADER_CLIENT;
    packet[40..48].copy_from_slice(&to_ntp_timestamp(t1));

    socket
        .send(&packet)
        .await
        .map_err(|e| anyhow!("send: {e}"))?;

    let mut buf = [0u8; PACKET_LEN];
    let (n, _) = tokio::time::timeout(REPLY_TIMEOUT, socket.recv_from(&mut buf))
        .await
        .map_err(|_| anyhow!("timed out after {}s", REPLY_TIMEOUT.as_secs()))?
        .map_err(|e| anyhow!("recv: {e}"))?;
    if n < PACKET_LEN {
        return Err(anyhow!("short reply ({n} of {PACKET_LEN} bytes)"));
    }

    let t2 = from_ntp_timestamp(&buf[32..40]); // server received
    let t3 = from_ntp_timestamp(&buf[40..48]); // server transmitted
    if t2 == 0 && t3 == 0 {
        return Err(anyhow!("server sent a zero timestamp (not an NTP server?)"));
    }
    let t4 = Utc::now().timestamp_millis() as i128;

    let offset = ((t2 - t1 as i128) + (t3 - t4)) / 2;
    Ok(offset as i64)
}

/// Try every configured server in order and store the first answer we get.
/// Returns `(server, offset_ms)`.
pub async fn sync(servers: &[String]) -> std::result::Result<(String, i64), String> {
    let mut last = String::from("no server configured");
    for s in servers {
        let name = s.trim();
        if name.is_empty() {
            continue;
        }
        match query_offset_ms(name).await {
            Ok(off) => return Ok((name.to_string(), off)),
            Err(e) => {
                last = format!("{name}: {e:#}");
                warn!("ntp: {last}");
            }
        }
    }
    Err(last)
}

/// Store a fresh offset and surface it to the UI/overlay.
pub fn apply_offset(state: &crate::AppState, server: &str, offset_ms: i64) {
    set_ntp_offset_ms(offset_ms);
    let mut st = state.status.write();
    st.ntp_offset_ms = Some(offset_ms);
    st.ntp_server = Some(server.to_string());
    st.ntp_synced_at = Some(Utc::now());
    st.ntp_error = None;
    drop(st);
    let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
}

/// Background task: keep the offset fresh. Syncs once at startup (so the very
/// first 报时 is already right) and then every `interval_min` minutes.
pub async fn run(state: crate::AppState) {
    loop {
        let (enabled, servers, interval) = {
            let cfg = state.config.read();
            (
                cfg.time_sync.enabled,
                cfg.time_sync.servers.clone(),
                cfg.time_sync.interval_min.max(1),
            )
        };

        if !enabled {
            set_ntp_offset_ms(0);
            {
                let mut st = state.status.write();
                st.ntp_error = None;
                st.ntp_offset_ms = None;
            }
        } else {
            match sync(&servers).await {
                Ok((server, off)) => {
                    apply_offset(&state, &server, off);
                    info!("ntp: synced to {server} (offset {off}ms)");
                }
                Err(e) => {
                    // Keep the last known offset: stale time is still better
                    // than a clock that jumps back to the (wrong) local one.
                    let mut st = state.status.write();
                    st.ntp_error = Some(format!("授时失败：{e}"));
                    warn!("ntp: all servers failed ({e})");
                }
            }
            let _ = state.notify.send(crate::NotifyKind::SchedulerStateChanged);
        }

        tokio::time::sleep(Duration::from_secs(interval as u64 * 60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntp_timestamp_round_trips() {
        // A known instant: 2026-09-15T00:00:00Z
        let unix_ms: i64 = 1_787_875_200_000;
        let bytes = to_ntp_timestamp(unix_ms);
        // The fraction is only 32 bits, so we allow sub-millisecond drift.
        let back = from_ntp_timestamp(&bytes);
        assert!(
            (back - unix_ms as i128).abs() <= 1,
            "got {back} want {unix_ms}"
        );
    }

    #[test]
    fn unix_epoch_maps_to_ntp_epoch() {
        // 1970-01-01 must land exactly on the NTP era offset.
        let bytes = to_ntp_timestamp(0);
        let mut secs = [0u8; 4];
        secs.copy_from_slice(&bytes[0..4]);
        assert_eq!(u32::from_be_bytes(secs), NTP_TO_UNIX_SECS as u32);
    }

    #[test]
    fn with_port_adds_ntp_port() {
        assert_eq!(with_port("ntp.ntsc.ac.cn"), "ntp.ntsc.ac.cn:123");
        assert_eq!(with_port("10.0.0.1:1234"), "10.0.0.1:1234");
    }
}
