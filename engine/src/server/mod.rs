//! axum HTTP / WebSocket server: REST endpoints for the admin UI, and a /ws
//! push channel for live status updates.

pub mod bootstrap;
pub mod health;
pub mod schedule_api;
pub mod ws_push;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::{
    cors::{Any, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

use tvbs_engine::AppState;

use crate::embedded::DistAssets;

pub fn build_router(state: AppState, _assets: DistAssets) -> Router {
    let api = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/api/status", get(schedule_api::status))
        .route("/api/bootstrap", post(bootstrap::bootstrap))
        .route("/api/playlist", get(schedule_api::get_playlist))
        .route(
            "/api/playlist/item",
            post(schedule_api::upsert_item).delete(schedule_api::delete_item),
        )
        .route(
            "/api/scheduler/enable",
            post(schedule_api::enable_scheduler),
        )
        .with_state(Arc::new(state));

    let root = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_push::ws_handler))
        .merge(api)
        .layer(
            tower::ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    "no-store",
                ))
                .layer(
                    CorsLayer::new()
                        .allow_origin(Any)
                        .allow_methods(Any)
                        .allow_headers(Any),
                ),
        );

    root
}

async fn index() -> &'static str {
    "tv-obsbroadcast-scheduler engine\n\n\
     GET  /healthz              liveness\n\
     GET  /api/status           current engine + scheduler status\n\
     GET  /api/playlist         playlist snapshot\n\
     POST /api/playlist/item    upsert a program item\n\
     DEL  /api/playlist/item    delete a program item\n\
     POST /api/scheduler/enable arm / disarm the scheduler\n\
     POST /api/bootstrap        one-time C-plugin -> engine bootstrap\n\
     WS   /ws                   live status push\n"
}
