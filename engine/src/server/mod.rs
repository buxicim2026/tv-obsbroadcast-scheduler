//! axum HTTP / WebSocket server: REST endpoints for the admin UI, and a /ws
//! push channel for live status updates.

pub mod bootstrap;
pub mod health;
pub mod schedule_api;
pub mod ws_push;

use std::sync::Arc;

use axum::{
    body::Body,
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response as AxumResponse},
    routing::{get, post},
    Router,
};
use tower_http::{
    cors::{Any, CorsLayer},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

use crate::embedded::{DistAssets, ADMIN_DIR, OVERLAY_DIR};
use crate::AppState;

/// Content-Type by file extension — the embedded admin/overlay assets are
/// served from memory, so axum cannot guess the MIME type from a filesystem.
fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}

/// Serve an embedded directory entry (index.html when the path is empty).
fn serve_embedded(dir: &'static include_dir::Dir<'static>, path: &str) -> AxumResponse {
    let path = path.trim_start_matches('/');
    let entry = if path.is_empty() || path.ends_with('/') {
        dir.get_file(format!("{}index.html", path).trim_start_matches('/'))
    } else {
        dir.get_file(path)
    };
    let Some(file) = entry else {
        return (StatusCode::NOT_FOUND, "404 not found").into_response();
    };
    let name = file.path().to_string_lossy().to_string();
    match AxumResponse::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_for(&name))
        .body(Body::from(file.contents().to_vec()))
    {
        Ok(resp) => resp,
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn admin_index() -> AxumResponse {
    serve_embedded(&ADMIN_DIR, "index.html")
}

async fn admin_asset(Path(path): Path<String>) -> AxumResponse {
    serve_embedded(&ADMIN_DIR, &path)
}

async fn overlay_index() -> AxumResponse {
    serve_embedded(&OVERLAY_DIR, "index.html")
}

async fn overlay_asset(Path(path): Path<String>) -> AxumResponse {
    serve_embedded(&OVERLAY_DIR, &path)
}

pub fn build_router(state: AppState, _assets: DistAssets) -> Router {
    // Both routers must carry the *same* state type to be merged, and /ws
    // needs `State<Arc<AppState>>` — so share one Arc across both.
    let shared = Arc::new(state);

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
        .route("/ws", get(ws_push::ws_handler))
        .with_state(shared.clone());

    let root = Router::new()
        .route("/", get(index))
        // Admin UI + overlay are compiled into the binary (include_dir) and
        // served from memory here. Without these routes /admin returned 404
        // and the page rendered blank.
        .route("/admin", get(admin_index))
        .route("/admin/*path", get(admin_asset))
        .route("/overlay", get(overlay_index))
        .route("/overlay/*path", get(overlay_asset))
        .with_state(shared)
        .merge(api)
        .layer(
            tower::ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
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
     GET  /admin                 admin UI (bundled)\n\
     GET  /overlay               broadcast overlay (bundled)\n\
     GET  /healthz              liveness\n\
     GET  /api/status           current engine + scheduler status\n\
     GET  /api/playlist         playlist snapshot\n\
     POST /api/playlist/item    upsert a program item\n\
     DEL  /api/playlist/item    delete a program item\n\
     POST /api/scheduler/enable arm / disarm the scheduler\n\
     POST /api/bootstrap        one-time C-plugin -> engine bootstrap\n\
     WS   /ws                   live status push\n"
}
