//! obs-websocket 5 client. Owns a single WebSocket connection. Each RPC
//! command is wrapped in `ObsWsCmd` and pushed onto a `mpsc`; a background
//! task owns the `WebSocketStream`, allocates request ids, sends payloads,
//! and routes responses back through `oneshot` channels. The 1-by-1 mapping
//! between `pending` entries and in-flight commands is what guarantees that
//! the response gets back to the right caller.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, MaybeTlsStream};
use tracing::{debug, warn};

use crate::config::ObsWsConfig;

use super::messages::{MediaInputAction, MediaInputStatus, ObsVersion};

/// Outbound command types. The `resp` channel carries the awaited response
/// back to the caller; each RPC variant declares its own response shape.
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

/// Shared client. Cheap to clone — internal cmd_tx is enough for most callers.
pub struct ObsWsClient {
    cfg: ObsWsConfig,
    cmd_tx: mpsc::Sender<ObsWsCmd>,
    next_request_id: Arc<Mutex<u64>>,
}

impl ObsWsClient {
    /// Connect + authenticate + spawn the background receive loop.
    pub async fn connect(cfg: &ObsWsConfig) -> Result<Arc<Self>> {
        let url = if cfg.tls {
            format!("wss://{}:{}/", cfg.host, cfg.port)
        } else {
            format!("ws://{}:{}/", cfg.host, cfg.port)
        };

        let request = Request::builder()
            .method("GET")
            .uri(&url)
            .header("Host", format!("{}:{}", cfg.host, cfg.port))
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header(
                "Sec-WebSocket-Key",
                base64::engine::general_purpose::STANDARD.encode(&[0u8; 16]),
            )
            .header("Sec-WebSocket-Version", "13")
            .body(())
            .map_err(|e| anyhow!("build WS handshake: {e}"))?;

        let tcp = tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port))
            .await
            .with_context(|| format!("tcp connect to {}:{}", cfg.host, cfg.port))?;
        tcp.set_nodelay(true).ok();

        let (stream, _response) = client_async(request, tcp)
            .await
            .context("ws handshake to obs")?;
        let mut ws = stream;

        // Authentication: receive `op 0 Hello`, send `op 1 Identify`.
        let hello = recv_op(&mut ws, 0).await.context("obs hello")?;
        let auth = hello
            .get("d")
            .and_then(|d| d.get("authentication"))
            .cloned()
            .unwrap_or(Value::Null);
        let identify = build_identify(&auth, cfg.password.as_deref());
        send_op(&mut ws, 1, identify).await?;
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
            next_request_id: Arc::new(Mutex::new(1)),
        });

        client.spawn_loop(cmd_rx, ws).await;
        Ok(client)
    }

    fn alloc_id(&self) -> u64 {
        let mut g = self.next_request_id.lock();
        let id = *g;
        *g = g.wrapping_add(1);
        id
    }

    /// Swap a Media Source's `local_file` to a new path.
    pub async fn set_input_settings(
        &self,
        input_name: &str,
        settings: Value,
        overlay: bool,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(ObsWsCmd::SetInputSettings {
                input_name: input_name.to_string(),
                settings,
                overlay,
                resp: tx,
            })
            .await
            .map_err(|_| anyhow!("obs-ws cmd channel closed"))?;
        rx.await
            .map_err(|_| anyhow!("set_input_settings response dropped"))?
    }

    /// Send an action to a Media Source.
    pub async fn trigger_media_input_action(
        &self,
        input_name: &str,
        action: MediaInputAction,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(ObsWsCmd::TriggerMediaInputAction {
                input_name: input_name.to_string(),
                action,
                resp: tx,
            })
            .await
            .map_err(|_| anyhow!("obs-ws cmd channel closed"))?;
        rx.await
            .map_err(|_| anyhow!("trigger_media_input_action response dropped"))?
    }

    /// Read Media Source status (`mediaState`, `mediaDuration` ms, `mediaCursor` ms).
    pub async fn get_media_input_status(&self, input_name: &str) -> Result<MediaInputStatus> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(ObsWsCmd::GetMediaInputStatus {
                input_name: input_name.to_string(),
                resp: tx,
            })
            .await
            .map_err(|_| anyhow!("obs-ws cmd channel closed"))?;
        rx.await
            .map_err(|_| anyhow!("get_media_input_status response dropped"))?
    }

    /// Query OBS version (used by /healthz, admin UI banner).
    pub async fn get_version(&self) -> Result<ObsVersion> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(ObsWsCmd::GetVersion { resp: tx })
            .await
            .map_err(|_| anyhow!("obs-ws cmd channel closed"))?;
        rx.await
            .map_err(|_| anyhow!("get_version response dropped"))?
    }

    /// Background loop: owns the WebSocketStream and the command receiver.
    /// Routes response frames back via the `pending` map.
    async fn spawn_loop(
        self: &Arc<Self>,
        mut cmd_rx: mpsc::Receiver<ObsWsCmd>,
        mut ws: tokio_tungstenite::WebSocketStream<
            MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) {
        // `pending` maps the obs-websocket requestId -> the oneshot awaiting
        // the response payload. We register before send and remove on receive.
        let pending: Arc<Mutex<HashMap<u64, PendingTask>>> =
            Arc::new(Mutex::new(HashMap::new()));

        enum PendingTask {
            Status(oneshot::Sender<Result<MediaInputStatus>>),
            Version(oneshot::Sender<Result<ObsVersion>>),
            Ack(oneshot::Sender<Result<()>>),
        }

        let pending_for_recv = pending.clone();
        let id_alloc = self.next_request_id.clone();

        // Helper to issue a request with an entry in `pending`.
        async fn issue<F>(
            ws: &mut tokio_tungstenite::WebSocketStream<
                MaybeTlsStream<tokio::net::TcpStream>,
            >,
            id: u64,
            request_type: &str,
            request_data: Value,
            pending: &Arc<Mutex<HashMap<u64, PendingTask>>>,
            store: F,
        ) -> Result<()>
        where
            F: FnOnce(oneshot::Sender<()>) -> PendingTask,
        {
            let payload = json!({
                "op": 6,
                "d": {
                    "requestId": id.to_string(),
                    "requestType": request_type,
                    "requestData": request_data
                }
            });
            // The wrapper Future<Output = ()> is just used to give us a Sender
            // we can store in the enum; we don't actually await it.
            let (tx, _future) = oneshot::channel::<()>();
            pending.lock().insert(id, store(tx));
            send_msg(ws, payload).await
        }

        loop {
            tokio::select! {
                biased;

                incoming = ws.next() => {
                    let Some(frame) = incoming else { break };
                    match frame {
                        Ok(Message::Text(txt)) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                                let op = v.get("op").and_then(|x| x.as_u64()).unwrap_or(99);
                                if op == 7 {
                                    // RequestResponse — server returns op 7 with
                                    // requestData; the payload is in `d.requestData`.
                                    let rid = v.get("d").and_then(|x| x.get("requestId"))
                                        .and_then(|x| x.as_u64());
                                    let status = v.get("d").and_then(|x| x.get("requestStatus"))
                                        .cloned().unwrap_or(Value::Null);
                                    let data = v.get("d").and_then(|x| x.get("responseData"))
                                        .cloned().unwrap_or(Value::Null);
                                    if let Some(rid) = rid {
                                        let task = pending_for_recv.lock().remove(&rid);
                                        match task {
                                            Some(PendingTask::Status(t)) => {
                                                if status.get("result").and_then(|x| x.as_bool()).unwrap_or(false) {
                                                    match serde_json::from_value::<MediaInputStatus>(data) {
                                                        Ok(s) => { let _ = t.send(Ok(s)); }
                                                        Err(e) => { let _ = t.send(Err(anyhow!("decode MediaInputStatus: {e}"))); }
                                                    }
                                                } else {
                                                    let msg = status.get("comment").and_then(|x| x.as_str()).unwrap_or("unknown");
                                                    let _ = t.send(Err(anyhow!("obs status: {msg}")));
                                                }
                                            }
                                            Some(PendingTask::Version(t)) => {
                                                if status.get("result").and_then(|x| x.as_bool()).unwrap_or(false) {
                                                    let _ = t.send(Ok(ObsVersion::from(&data)));
                                                } else {
                                                    let msg = status.get("comment").and_then(|x| x.as_str()).unwrap_or("unknown");
                                                    let _ = t.send(Err(anyhow!("obs status: {msg}")));
                                                }
                                            }
                                            Some(PendingTask::Ack(t)) => {
                                                if status.get("result").and_then(|x| x.as_bool()).unwrap_or(false) {
                                                    let _ = t.send(Ok(()));
                                                } else {
                                                    let msg = status.get("comment").and_then(|x| x.as_str()).unwrap_or("unknown");
                                                    let _ = t.send(Err(anyhow!("obs ack: {msg}")));
                                                }
                                            }
                                            None => { /* unknown id */ }
                                        }
                                    }
                                } else if op == 5 {
                                    // Event; ignore for now (could route
                                    // `MediaInputPlaybackEnded` etc. here).
                                    debug!("obs event: {}", v);
                                } else {
                                    debug!("obs op={} msg={}", op, v);
                                }
                            }
                        }
                        Ok(Message::Ping(p)) => { let _ = ws.send(Message::Pong(p)).await; }
                        Ok(Message::Close(_)) => { break; }
                        Ok(_) => {}
                        Err(e) => { warn!("ws recv err: {e}"); break; }
                    }
                }

                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    let id = {
                        let mut g = id_alloc.lock();
                        let cur = *g;
                        *g = g.wrapping_add(1);
                        cur
                    };
                    let outcome: Result<()> = match cmd {
                        ObsWsCmd::SetInputSettings { input_name, settings, overlay, resp } => {
                            let data = json!({
                                "inputName": input_name,
                                "settings": settings,
                                "overlay": overlay
                            });
                            let r = issue(&mut ws, id, "SetInputSettings", data, &pending, PendingTask::Ack).await;
                            match r {
                                Ok(_) => {
                                    let pending = pending.clone();
                                    let (tx, rx) = oneshot::channel();
                                    pending.lock().insert(id, PendingTask::Ack(tx));
                                    match tokio::time::timeout(Duration::from_secs(5), rx).await {
                                        Ok(Ok(_)) => resp.send(Ok(())),
                                        Ok(Err(_)) => resp.send(Err(anyhow!("ack dropped"))),
                                        Err(_) => resp.send(Err(anyhow!("ack timeout"))),
                                    }.ok();
                                    Ok(())
                                }
                                Err(e) => {
                                    let _ = resp.send(Err(anyhow!("send SetInputSettings: {e}")));
                                    Err(e)
                                }
                            }
                        }
                        ObsWsCmd::TriggerMediaInputAction { input_name, action, resp } => {
                            let data = json!({
                                "inputName": input_name,
                                "action": action.as_str()
                            });
                            let (tx, rx) = oneshot::channel();
                            pending.lock().insert(id, PendingTask::Ack(tx));
                            let r = send_msg(&mut ws, json!({
                                "op": 6,
                                "d": {
                                    "requestId": id.to_string(),
                                    "requestType": "TriggerMediaInputAction",
                                    "requestData": data
                                }
                            })).await;
                            if let Err(e) = r {
                                let _ = resp.send(Err(anyhow!("send TriggerMediaInputAction: {e}")));
                                Err(e)
                            } else {
                                match tokio::time::timeout(Duration::from_secs(5), rx).await {
                                    Ok(Ok(_)) => { let _ = resp.send(Ok(())); Ok(()) }
                                    Ok(Err(_)) => { let _ = resp.send(Err(anyhow!("ack dropped"))); Ok(()) }
                                    Err(_) => { let _ = resp.send(Err(anyhow!("ack timeout"))); Ok(()) }
                                }
                            }
                        }
                        ObsWsCmd::GetMediaInputStatus { input_name, resp } => {
                            let data = json!({ "inputName": input_name });
                            let (tx, rx) = oneshot::channel();
                            pending.lock().insert(id, PendingTask::Status(tx));
                            let r = send_msg(&mut ws, json!({
                                "op": 6,
                                "d": {
                                    "requestId": id.to_string(),
                                    "requestType": "GetMediaInputStatus",
                                    "requestData": data
                                }
                            })).await;
                            if let Err(e) = r {
                                let _ = resp.send(Err(anyhow!("send GetMediaInputStatus: {e}")));
                                Err(e)
                            } else {
                                match tokio::time::timeout(Duration::from_secs(5), rx).await {
                                    Ok(Ok(Ok(s))) => { let _ = resp.send(Ok(s)); Ok(()) }
                                    Ok(Ok(Err(e))) => { let _ = resp.send(Err(e)); Ok(()) }
                                    Ok(Err(_)) => { let _ = resp.send(Err(anyhow!("status dropped"))); Ok(()) }
                                    Err(_) => { let _ = resp.send(Err(anyhow!("status timeout"))); Ok(()) }
                                }
                            }
                        }
                        ObsWsCmd::GetVersion { resp } => {
                            let (tx, rx) = oneshot::channel();
                            pending.lock().insert(id, PendingTask::Version(tx));
                            let r = send_msg(&mut ws, json!({
                                "op": 6,
                                "d": {
                                    "requestId": id.to_string(),
                                    "requestType": "GetVersion"
                                }
                            })).await;
                            if let Err(e) = r {
                                let _ = resp.send(Err(anyhow!("send GetVersion: {e}")));
                                Err(e)
                            } else {
                                match tokio::time::timeout(Duration::from_secs(5), rx).await {
                                    Ok(Ok(Ok(v))) => { let _ = resp.send(Ok(v)); Ok(()) }
                                    Ok(Ok(Err(e))) => { let _ = resp.send(Err(e)); Ok(()) }
                                    Ok(Err(_)) => { let _ = resp.send(Err(anyhow!("version dropped"))); Ok(()) }
                                    Err(_) => { let _ = resp.send(Err(anyhow!("version timeout"))); Ok(()) }
                                }
                            }
                        }
                    };
                    let _ = outcome; // errors already reported via resp
                }
            }
        }
    }
}

impl ObsWsClient {
    /// Configuration snapshot (useful for reconnect logic).
    pub fn config(&self) -> &ObsWsConfig {
        &self.cfg
    }
}

/* -------------------------------------------------------------------------- */
/* Message I/O                                                                */
/* -------------------------------------------------------------------------- */

async fn send_msg(
    ws: &mut tokio_tungstenite::WebSocketStream<
        MaybeTlsStream<tokio::net::TcpStream>,
    >,
    payload: Value,
) -> Result<()> {
    let s = serde_json::to_string(&payload)?;
    ws.send(Message::Text(s)).await?;
    Ok(())
}

async fn send_op(
    ws: &mut tokio_tungstenite::WebSocketStream<
        MaybeTlsStream<tokio::net::TcpStream>,
    >,
    op: u8,
    d: Value,
) -> Result<()> {
    send_msg(ws, json!({ "op": op, "d": d })).await
}

async fn recv_op(
    ws: &mut tokio_tungstenite::WebSocketStream<
        MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_op: u8,
) -> Result<Value> {
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
    match auth {
        Value::Null | Value::Object(_) if password.is_empty() => {
            json!({ "rpcVersion": 1 })
        }
        Value::Object(map) => {
            let challenge = map
                .get("challenge")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let salt = map
                .get("salt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let auth_str = compute_auth(challenge, salt, password);
            json!({
                "rpcVersion": 1,
                "authentication": auth_str
            })
        }
        _ => json!({ "rpcVersion": 1 }),
    }
}

/// obs-websocket 5 password hashing scheme:
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
