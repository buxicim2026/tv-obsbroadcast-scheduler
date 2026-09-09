//! WebSocket /ws — pushes the engine status + scheduler snapshot every
//! `notify` or every 250ms, whichever comes first. Clients are the admin UI
//! and the overlay widget.
//!
//! Initial implementation is the boilerplate handler; the actual payload
//! shaping and re-connect-on-config-change wiring is added by
//! `playlist-interrupt-probe` (which owns the scheduler state).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| ws_loop(socket, state))
}

async fn ws_loop(mut socket: WebSocket, state: Arc<AppState>) {
    let mut notify_rx = state.notify.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    // Never "catch up" on missed ticks: a stalled client would otherwise get
    // a burst of snapshots (wasted CPU + memory churn).
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await; // skip immediate

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if socket
                    .send(Message::Text(snapshot(&state).to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            n = notify_rx.recv() => {
                match n {
                    Ok(_) => {}
                    // `Lagged` returns immediately: without yielding, select!
                    // would spin on this branch and hammer the socket.
                    Err(RecvError::Lagged(_)) => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                }
                if socket
                    .send(Message::Text(snapshot(&state).to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() { return; }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn snapshot(state: &AppState) -> serde_json::Value {
    let st = state.status.read().clone();
    let cfg = state.config.read();
    json!({
        "kind": "snapshot",
        "scheduler": st,
        "playlist_size": cfg.playlist.items.len(),
        "bumpers_size": cfg.playlist.bumpers.len(),
        "target_input": cfg.target_input,
        // The admin panel replaces its whole snapshot with this frame on every
        // push. Without the token here the UI lost it ~2x a second and every
        // write came back 401 ("not authorised") even though the token existed.
        "bootstrap_token": cfg.bootstrap_token,
    })
}
