//! The JSON API (`/v0/...`), `/health` and `/metrics`.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use super::{AppState, Q};
use crate::error::ApiResult;
use crate::service::{BookAt, Coverage, InstrumentPage, ManifestInfo, Page, RangeQuery};

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/v0/instruments", get(instruments))
        .route("/v0/trades", get(trades))
        .route("/v0/bars", get(bars))
        .route("/v0/book", get(book))
        .route("/v0/book-events", get(book_events))
        .route("/v0/footprint", get(footprint))
        .route("/v0/volume-profile", get(volume_profile))
        .route("/v0/coverage", get(coverage))
        .route("/v0/manifests", get(manifests))
        .route("/v0/limits", get(limits))
        .route("/v0/openapi.json", get(openapi))
}

async fn health(State(s): State<AppState>) -> Response {
    let probe = s
        .svc
        .engine()
        .query("SELECT 1".into(), vec![], |r| r.get::<_, i32>(0))
        .await;
    let (status, body) = match probe {
        Ok(_) => (
            StatusCode::OK,
            serde_json::json!({
                "status": "ok",
                "archive": s.svc.config.root.display(),
                "archives": s.svc.config.archives,
                "duckdb": s.svc.engine().version(),
                "version": env!("CARGO_PKG_VERSION"),
            }),
        ),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"status": "unavailable", "error": e.body()["error"]}),
        ),
    };
    (status, Json(body)).into_response()
}

async fn metrics(State(s): State<AppState>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        crate::metrics::global().render(s.svc.engine().busy_slots()),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct InstrumentsQuery {
    q: Option<String>,
    venue: Option<String>,
    class: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

async fn instruments(
    State(s): State<AppState>,
    Q(q): Q<InstrumentsQuery>,
) -> ApiResult<Json<InstrumentPage>> {
    s.svc
        .instruments(
            q.q.as_deref(),
            q.venue.as_deref(),
            q.class.as_deref(),
            q.limit,
            q.cursor.as_deref(),
        )
        .await
        .map(Json)
}

async fn trades(State(s): State<AppState>, Q(q): Q<RangeQuery>) -> ApiResult<Json<Page>> {
    s.svc.trades(&q).await.map(Json)
}

async fn book_events(State(s): State<AppState>, Q(q): Q<RangeQuery>) -> ApiResult<Json<Page>> {
    s.svc.book_events(&q).await.map(Json)
}

async fn bars(State(s): State<AppState>, Q(q): Q<RangeQuery>) -> ApiResult<Json<Page>> {
    s.svc.bars(&q).await.map(Json)
}

async fn footprint(State(s): State<AppState>, Q(q): Q<RangeQuery>) -> ApiResult<Json<Page>> {
    s.svc.footprints(&q).await.map(Json)
}

async fn volume_profile(State(s): State<AppState>, Q(q): Q<RangeQuery>) -> ApiResult<Json<Page>> {
    s.svc.volume_profiles(&q).await.map(Json)
}

#[derive(Debug, Deserialize)]
struct BookQuery {
    instrument: String,
    at: Option<String>,
    depth: Option<String>,
    levels: Option<usize>,
}

async fn book(State(s): State<AppState>, Q(q): Q<BookQuery>) -> ApiResult<Json<BookAt>> {
    s.svc
        .book_at(&q.instrument, q.at.as_deref(), q.depth.as_deref(), q.levels)
        .await
        .map(Json)
}

#[derive(Debug, Deserialize)]
struct CoverageQuery {
    instrument: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

async fn coverage(State(s): State<AppState>, Q(q): Q<CoverageQuery>) -> ApiResult<Json<Coverage>> {
    s.svc
        .coverage(
            q.instrument.as_deref(),
            q.start.as_deref(),
            q.end.as_deref(),
        )
        .await
        .map(Json)
}

#[derive(Debug, Deserialize)]
struct ManifestQuery {
    start: Option<String>,
    end: Option<String>,
    archive: Option<String>,
    #[serde(default)]
    files: bool,
}

async fn manifests(
    State(s): State<AppState>,
    Q(q): Q<ManifestQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let list: Vec<ManifestInfo> = s
        .svc
        .manifests(
            q.start.as_deref(),
            q.end.as_deref(),
            q.archive.as_deref(),
            q.files,
        )
        .await?;
    Ok(Json(serde_json::json!({ "manifests": list })))
}

async fn limits(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "limits": s.svc.config.limits }))
}

async fn openapi() -> Json<serde_json::Value> {
    Json(super::openapi::document())
}
