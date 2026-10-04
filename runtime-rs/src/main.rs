//! Pisper Rust backend — vertical slice 1.
//!
//! This server replaces the Node.js `runtime/` layer: the same HTTP contract
//! (`/api/*`) backed by the pi-rs engine (crates.io `pi-rs`) instead of the
//! `@earendil-works/pi-coding-agent` npm package.
//!
//! Slice 1 (this file): process bootstrap, `/api/health` contract, JSON 404
//! for unknown API routes, and graceful config surfaces. Slice 2 wires the
//! pi-rs session host (create/list sessions, `/input`, `/live` SSE).

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

/// API version handshake. Clients compare this against their minimum
/// supported version (see `runtime/http/routes/sessions-runtime.mjs`).
const API_VERSION: u32 = 1;
const MIN_CLIENT_VERSION: u32 = 1;
const ENGINE: &str = "pi-rs";

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "engine": ENGINE,
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": API_VERSION,
        "minClientVersion": MIN_CLIENT_VERSION,
        "capabilities": capabilities(),
    }))
}

/// Feature flags the frontends gate UI on. The Node runtime computed this from
/// installed components; the Rust backend grows it as slices land.
fn capabilities() -> serde_json::Value {
    json!({
        "sessions": true,
        "mcp": false,
        "skills": false,
        "workflows": false,
        "schedules": false,
        "remote": false,
    })
}

async fn unknown_api_fallback() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": { "code": "not_found", "message": "unknown API route" } })),
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pisper_server=info,tower_http=info".into()),
        )
        .init();

    let app = router();
    let addr = std::env::var("PISPER_RS_ADDR").unwrap_or_else(|_| "127.0.0.1:5174".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("pisper-server (Rust runtime) listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn router() -> Router {
    Router::new()
        .route("/api/health", get(health))
        .fallback(unknown_api_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_matches_client_handshake_contract() {
        let res = router()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["engine"], json!(ENGINE));
        assert_eq!(body["apiVersion"], json!(API_VERSION));
        assert_eq!(body["minClientVersion"], json!(MIN_CLIENT_VERSION));
        assert!(body["capabilities"].is_object());
    }

    #[tokio::test]
    async fn unknown_api_routes_return_structured_404() {
        let res = router()
            .oneshot(
                Request::get("/api/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
