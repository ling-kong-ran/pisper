//! Bing 配置可用性测试；宿主负责 cookie/来源鉴权，领域状态保持独立。
use crate::{native_web_search::WebSearchService, ApiError};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub fn router<S: Clone + Send + Sync + 'static>(service: Arc<WebSearchService>) -> Router<S> {
    Router::new()
        .route("/api/plugins/web-search/test", post(test))
        .with_state(service)
}

async fn test(
    State(service): State<Arc<WebSearchService>>,
    request: Request,
) -> Result<Response, ApiError> {
    // Release bodyJson accepts an empty body as {}, reads JSON regardless of
    // Content-Type and applies the shared 32,000,000-byte body limit.
    let bytes = to_bytes(request.into_body(), 32_000_000)
        .await
        .map_err(|_| ApiError::bad_request("请求体过大。"))?;
    let config = if bytes.is_empty() {
        json!({})
    } else {
        crate::native_web_search::parse_config_json(&String::from_utf8_lossy(&bytes))
            .map_err(|error| ApiError::bad_request(format!("无效 JSON 请求：{error}")))?
    };
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let result = service
        .test(&config, cancellation)
        .await
        .map_err(|error| ApiError::bad_request(error.message))?;
    Ok((
        [
            ("Cache-Control", "no-store"),
            ("Content-Type", "application/json; charset=utf-8"),
        ],
        Json(json!({"count":result.results.len(),"provider":result.provider})),
    )
        .into_response())
}
