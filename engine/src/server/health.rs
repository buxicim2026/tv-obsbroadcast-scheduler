//! `GET /healthz` — simple liveness probe.

use axum::Json;
use serde_json::{json, Value};

pub async fn healthz() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "tv-obsbroadcast-scheduler",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
