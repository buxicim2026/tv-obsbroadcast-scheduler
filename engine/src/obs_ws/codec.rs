//! obs-websocket 5 client.
//!
//! Owns a single WebSocket connection to OBS. Each RPC is wrapped in an
//! `ObsWsCmd` and pushed onto an mpsc; a background task owns the socket,
//! allocates request ids, sends payloads, and routes responses back through
//! `oneshot` channels. The 1:1 mapping between `pending` entries and
//! in-flight commands is what guarantees a response reaches its caller.
//!
//! Only the four requests the scheduler actually needs are implemented:
//!   * GetVersion
//!   * SetInputSettings
//!   * TriggerMediaInputAction
//!   * GetMediaInputStatus

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use super::messages::{MediaInputAction, MediaInputStatus, ObsVersion};
use crate::config::ObsWsConfig;

/// Concrete stream type produced by `connect_async` (plain TCP for ws://,
/// TLS for wss:// — the enum hides the difference).
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How a resolved response is handed back to its awaiting caller.
enum Pending {
    Ack(oneshot::Sender<Result<()>>),
    Status(oneshot::Sender<Result<MediaInputStatus>>),
    Version(oneshot::Sender<Result<ObsVersion>>),
}

/// Outbound command. `resp` carries the awaited value back to the caller.
#[derive(Debug)]
pub enum ObsWsCmd {
    SetInputSettings {
        input_name: String,
        settings: Value,
        overlay: bool,
        resp: oneshot::Sender<Result<()>>,
    },
    TriggerMediaInputAction {
        input_name: String,
        action: MediaInputAction,
        resp: oneshot::Sender<Result<()>>,
    },
    GetMediaInputStatus {
        input_name: String,
        resp: oneshot::Sender<Result<MediaInputStatus>>,
    },
    GetVersion {
        resp: oneshot::Sender<Result<ObsVersion>>,
    },
}

/// Shared client handle. Cheap to clone around (the mpsc Sender is enough).
pub struct ObsWsClient {
    cfg: ObsWsConfig,
    cmd_tx: mpsc::Sender<ObsWsCmd>,
    next_id: Arc<Mutex<u64>>,
}

impl ObsWsClient {
    /// Connect + authenticate + spawn the background receive loop.
    pub async fn connect(cfg: &ObsWsConfig) -> Result<Arc<Self>> {
        let url = if cfg.tls {
            format!("wss://{}:{}/", cfg.host, cfg.port)
        } else {
            format!("ws://{}:{}/", cfg.host, cfg.port)
        };

        let (mut ws, _resp) = connect_async(url.as_str())
            .await
            .with_context(|| format!("connect to obs websocket at {url}"))?;

        // Handshake: Hello (op 0) -> Identify (op 1) -> Identified (op 2).
        let hello = recv_op(&mut ws, 0).await.context("obs hello")?;
        let auth = hello
            .get("d")
            .and_then(|d| d.get("authentication"))
            .cloned()
            .unwrap_or(Value::Null);
        send_op(&mut ws, 1, build_identify(&auth, cfg.password.as_deref())).await?;
        let identified = recv_op(&mut ws, 2).await.context("obs identified")?;
        if identified.get("op").and_then(|v| v.as_u64()) != Some(2) {
            bail!(
                "expected op 2 (Identified), got {}",
                identified.get("op").and_then(|v| v.as_u64()).unwrap_or(99)
            );
        }

        let (cmd_tx, cmd_rx) = mpsc::channel::<ObsWsCmd>(64);
        let client = Arc::new(Self {
            cfg: cfg.clone(),
            cmd_tx,
            next_id: Arc::new(Mutex::new(1)),
        });
        client.spawn(cmd_rx, ws);
        Ok(client)
    }

    fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock();
        let id = *g;
        *g = g.wrapping_add(1);
        id
    }

    async fn call(&self, cmd: ObsWsCmd) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| anyhow!("obs-ws cmd channel closed"))
    }

    /// Swap a Media Source's `local_file` to a new path.
    pub async fn set_input_settings(
        &self,
        input_name: &str,
        settings: Value,
        overlay: bool,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.call(ObsWsCmd::SetInputSettings {
            input_name: input_name.to_string(),
            settings,
            overlay,
            resp: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow!("set_input_settings response dropped"))?
    }

    /// Send an action to a Media Source (restart / pause / play / ...).
    pub async fn trigger_media_input_action(
        &self,
        input_name: &str,
        action: MediaInputAction,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.call(ObsWsCmd::TriggerMediaInputAction {
            input_name: input_name.to_string(),
            action,
            resp: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow!("trigger_media_input_action response dropped"))?
    }

    /// Read Media Source status (state, duration ms, cursor ms).
    pub async fn get_media_input_status(&self, input_name: &str) -> Result<MediaInputStatus> {
        let (tx, rx) = oneshot::channel();
        self.call(ObsWsCmd::GetMediaInputStatus {
            input_name: input_name.to_string(),
            resp: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow!("get_media_input_status response dropped"))?
    }

    /// Query OBS version (used by /healthz and the admin banner).
    pub async fn get_version(&self) -> Result<ObsVersion> {
        let (tx, rx) = oneshot::channel();
        self.call(ObsWsCmd::GetVersion { resp: tx }).await?;
        rx.await
            .map_err(|_| anyhow!("get_version response dropped"))?
    }

    /// Configuration snapshot (useful for reconnect logic).
    pub fn config(&self) -> &ObsWsConfig {
        &self.cfg
    }

    /// Background loop: owns the socket and the command receiver, and routes
    /// responses back via the `pending` map.
    fn spawn(self: &Arc<Self>, mut cmd_rx: mpsc::Receiver<ObsWsCmd>, mut ws: WsStream) {
        let pending: Arc<Mutex<HashMap<u64, Pending>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_id = self.next_id.clone();
        let pending_recv = pending.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    // ---- inbound frames -------------------------------------
                    incoming = ws.next() => {
                        let Some(frame) = incoming else { break };
                        match frame {
                            Ok(Message::Text(txt)) => {
                                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                                let op = v.get("op").and_then(|x| x.as_u64()).unwrap_or(99);
                                if op == 7 {
                                    resolve_response(&v, &pending_recv);
                                } else if op == 5 {
                                    debug!("obs event: {}", v);
                                } else {
                                    debug!("obs op={} msg={}", op, v);
                                }
                            }
                            Ok(Message::Ping(p)) => {
                                if ws.send(Message::Pong(p)).await.is_err() { break; }
                            }
                            Ok(Message::Close(_)) => break,
                            Ok(_) => {}
                            Err(e) => { warn!("ws recv error: {e}"); break; }
                        }
                    }

                    // ---- outbound commands ----------------------------------
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break };
                        let id = {
                            let mut g = next_id.lock();
                            let cur = *g;
                            *g = g.wrapping_add(1);
                            cur
                        };
                        let request_type = match &cmd {
                            ObsWsCmd::SetInputSettings { .. } => "SetInputSettings",
                            ObsWsCmd::TriggerMediaInputAction { .. } => "TriggerMediaInputAction",
                            ObsWsCmd::GetMediaInputStatus { .. } => "GetMediaInputStatus",
                            ObsWsCmd::GetVersion { .. } => "GetVersion",
                        };
                        let data = match &cmd {
                            ObsWsCmd::SetInputSettings { input_name, settings, overlay, .. } => json!({
                                "inputName": input_name,
                                "settings": settings,
                                "overlay": overlay
                            }),
                            ObsWsCmd::TriggerMediaInputAction { input_name, action, .. } => json!({
                                "inputName": input_name,
                                "action": action.as_str()
                            }),
                            ObsWsCmd::GetMediaInputStatus { input_name, .. } => json!({
                                "inputName": input_name
                            }),
                            ObsWsCmd::GetVersion { .. } => Value::Null,
                        };
                        // Register before sending so a fast reply can't race us.
                        match cmd {
                            ObsWsCmd::SetInputSettings { resp, .. }
                            | ObsWsCmd::TriggerMediaInputAction { resp, .. } => {
                                pending.lock().insert(id, Pending::Ack(resp));
                            }
                            ObsWsCmd::GetMediaInputStatus { resp, .. } => {
                                pending.lock().insert(id, Pending::Status(resp));
                            }
                            ObsWsCmd::GetVersion { resp } => {
                                pending.lock().insert(id, Pending::Version(resp));
                            }
                        }
                        let payload = json!({
                            "op": 6,
                            "d": {
                                "requestId": id.to_string(),
                                "requestType": request_type,
                                "requestData": data
                            }
                        });
                        if let Err(e) = send_msg(&mut ws, payload).await {
                            warn!("ws send failed: {e}");
                            if let Some(p) = pending.lock().remove(&id) {
                                fail_pending(p, format!("send failed: {e}"));
                            }
                            break;
                        }
                    }
                }
            }
        });
    }
}

/// Route an inbound op-7 RequestResponse to whoever is waiting on it.
fn resolve_response(v: &Value, pending: &Arc<Mutex<HashMap<u64, Pending>>>) {
    let Some(rid) = v
        .get("d")
        .and_then(|d| d.get("requestId"))
        .and_then(|r| r.as_str())
        .and_then(|r| r.parse::<u64>().ok())
    else {
        return;
    };
    let Some(entry) = pending.lock().remove(&rid) else {
        return;
    };
    let status = v.get("d").and_then(|d| d.get("requestStatus")).cloned();
    let ok = status
        .as_ref()
        .and_then(|s| s.get("result"))
        .and_then(|r| r.as_bool())
        .unwrap_or(false);
    let comment = status
        .as_ref()
        .and_then(|s| s.get("comment"))
        .and_then(|c| c.as_str())
        .unwrap_or("unknown error")
        .to_string();
    let data = v
        .get("d")
        .and_then(|d| d.get("responseData"))
        .cloned()
        .unwrap_or(Value::Null);

    match entry {
        Pending::Ack(tx) => {
            let _ = if ok {
                tx.send(Ok(()))
            } else {
                tx.send(Err(anyhow!("obs rejected request: {comment}")))
            };
        }
        Pending::Status(tx) => {
            let _ = if ok {
                match serde_json::from_value::<MediaInputStatus>(data) {
                    Ok(s) => tx.send(Ok(s)),
                    Err(e) => tx.send(Err(anyhow!("decode MediaInputStatus: {e}"))),
                }
            } else {
                tx.send(Err(anyhow!("obs rejected GetMediaInputStatus: {comment}")))
            };
        }
        Pending::Version(tx) => {
            let _ = if ok {
                tx.send(Ok(ObsVersion::from(&data)))
            } else {
                tx.send(Err(anyhow!("obs rejected GetVersion: {comment}")))
            };
        }
    }
}

fn fail_pending(entry: Pending, msg: String) {
    match entry {
        Pending::Ack(tx) => {
            let _ = tx.send(Err(anyhow!(msg)));
        }
        Pending::Status(tx) => {
            let _ = tx.send(Err(anyhow!(msg)));
        }
        Pending::Version(tx) => {
            let _ = tx.send(Err(anyhow!(msg)));
        }
    }
}

/* -------------------------------------------------------------------------- */
/* Message I/O                                                                */
/* -------------------------------------------------------------------------- */

async fn send_msg(ws: &mut WsStream, payload: Value) -> Result<()> {
    let s = serde_json::to_string(&payload)?;
    ws.send(Message::Text(s)).await?;
    Ok(())
}

async fn send_op(ws: &mut WsStream, op: u8, d: Value) -> Result<()> {
    send_msg(ws, json!({ "op": op, "d": d })).await
}

async fn recv_op(ws: &mut WsStream, expected_op: u8) -> Result<Value> {
    loop {
        let msg = ws
            .next()
            .await
            .ok_or_else(|| anyhow!("ws closed during handshake"))??;
        if let Message::Text(txt) = msg {
            let v: Value = serde_json::from_str(&txt)?;
            let op = v.get("op").and_then(|x| x.as_u64()).unwrap_or(99);
            if op == expected_op as u64 {
                return Ok(v);
            }
            if op == 8 || op == 9 {
                bail!("obs rejected handshake: {v}");
            }
        }
    }
}

/* -------------------------------------------------------------------------- */
/* obs-websocket 5 authentication                                            */
/* -------------------------------------------------------------------------- */

fn build_identify(auth: &Value, password: Option<&str>) -> Value {
    let password = password.unwrap_or("");
    if password.is_empty() {
        return json!({ "rpcVersion": 1 });
    }
    if let Value::Object(map) = auth {
        let challenge = map.get("challenge").and_then(|v| v.as_str()).unwrap_or("");
        let salt = map.get("salt").and_then(|v| v.as_str()).unwrap_or("");
        return json!({
            "rpcVersion": 1,
            "authentication": compute_auth(challenge, salt, password)
        });
    }
    json!({ "rpcVersion": 1 })
}

/// obs-websocket 5 password scheme:
///   secret_b64 = base64(sha256(password + salt))
///   auth       = base64(sha256(secret_b64 + challenge))
fn compute_auth(challenge: &str, salt: &str, password: &str) -> String {
    let mut h1 = Sha256::new();
    h1.update(password.as_bytes());
    h1.update(salt.as_bytes());
    let secret_b64 = base64::engine::general_purpose::STANDARD.encode(h1.finalize());

    let mut h2 = Sha256::new();
    h2.update(secret_b64.as_bytes());
    h2.update(challenge.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h2.finalize())
}

/// Used only to keep the `Duration` import meaningful if the timeout helpers
/// are reintroduced; harmless otherwise.
#[allow(dead_code)]
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
