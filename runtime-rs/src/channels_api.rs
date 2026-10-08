//! 通道的 HTTP 边界；模型目录由组合根提供，连接凭据始终留在领域私有状态。
use crate::native_channels::{ChannelError, ChannelService, Result};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;
pub(crate) type Models = Arc<dyn Fn() -> BoxFuture<'static, Result<Value>> + Send + Sync>;
struct Api {
    service: Arc<ChannelService>,
    models: Models,
}
struct Failure(ChannelError);
impl From<ChannelError> for Failure {
    fn from(error: ChannelError) -> Self {
        Self(error)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = self
            .0
            .status
            .and_then(|status| StatusCode::from_u16(status).ok())
            .filter(|status| status.is_client_error() || status.is_server_error())
            .unwrap_or(StatusCode::BAD_REQUEST);
        (
            status,
            Json(json!({"error":crate::security::redact_secret_text(&self.0.message)})),
        )
            .into_response()
    }
}
type ApiResult<T> = std::result::Result<T, Failure>;
pub(crate) fn router(service: Arc<ChannelService>, models: Models) -> Router {
    Router::new()
        .route(
            "/api/channels",
            get(catalog).head(not_found).fallback(not_found),
        )
        .route(
            "/api/channels/{platform}/onboarding",
            post(start).fallback(not_found),
        )
        .route(
            "/api/channels/{platform}/onboarding/{id}",
            get(onboarding)
                .delete(cancel)
                .head(not_found)
                .fallback(not_found),
        )
        .route(
            "/api/channels/{platform}/onboarding/{id}/verify",
            post(verify).fallback(not_found),
        )
        .route(
            "/api/channels/{platform}/reconnect",
            post(reconnect).fallback(not_found),
        )
        .route(
            "/api/channels/scopes/{id}",
            axum::routing::delete(reset).fallback(not_found),
        )
        .route(
            "/api/channels/{platform}",
            axum::routing::patch(update)
                .delete(remove)
                .fallback(not_found),
        )
        .with_state(Arc::new(Api { service, models }))
}
async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error":"接口不存在。"}))).into_response()
}
fn platform(value: &str) -> bool {
    ["feishu", "weixin", "qq", "telegram"].contains(&value)
}
async fn body(request: Request) -> ApiResult<Value> {
    let bytes = to_bytes(request.into_body(), 32_000_000)
        .await
        .map_err(|_| Failure(ChannelError::new("请求体过大。")))?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|error| Failure(ChannelError::new(error.to_string())))
}
async fn catalog(State(api): State<Arc<Api>>) -> ApiResult<Json<Value>> {
    let value = api.service.get_state()?;
    Ok(Json(
        json!({"providers":value["providers"],"connections":value["connections"],
        "scopes":value["scopes"],"models":(api.models)().await?}),
    ))
}
async fn start(
    State(api): State<Arc<Api>>,
    Path(kind): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    // release 兼容不发送正文的旧扫码客户端，解析失败同样交给服务处理空输入。
    let input = body(request).await.unwrap_or(Value::Null);
    Ok((
        StatusCode::CREATED,
        Json(api.service.start_onboarding(&kind, input).await?),
    )
        .into_response())
}
async fn onboarding(
    State(api): State<Arc<Api>>,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    Ok(match api.service.get_onboarding(&kind, &id) {
        Some(value) => Json(value).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"扫码任务不存在或已过期。"})),
        )
            .into_response(),
    })
}
async fn cancel(
    State(api): State<Arc<Api>>,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    Ok(Json(json!({"cancelled":api.service.cancel_onboarding(&kind,&id)})).into_response())
}
async fn verify(
    State(api): State<Arc<Api>>,
    Path((kind, id)): Path<(String, String)>,
    request: Request,
) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    let input = body(request).await?;
    Ok(
        match api
            .service
            .verify_onboarding(&kind, &id, input["code"].clone())?
        {
            Some(value) => Json(value).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"扫码任务不存在或已过期。"})),
            )
                .into_response(),
        },
    )
}
async fn reconnect(State(api): State<Arc<Api>>, Path(kind): Path<String>) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    api.service.reconnect(&kind).await?;
    Ok(catalog(State(api)).await?.into_response())
}
async fn update(
    State(api): State<Arc<Api>>,
    Path(kind): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    api.service.update(&kind, body(request).await?).await?;
    Ok(catalog(State(api)).await?.into_response())
}
async fn remove(State(api): State<Arc<Api>>, Path(kind): Path<String>) -> ApiResult<Response> {
    if !platform(&kind) {
        return Ok(not_found().await);
    }
    api.service.remove(&kind).await?;
    Ok(Json(json!({"deleted":true})).into_response())
}
async fn reset(State(api): State<Arc<Api>>, Path(id): Path<String>) -> ApiResult<Response> {
    Ok(if api.service.reset_scope(&id)? {
        Json(json!({"deleted":true})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"渠道会话不存在。"})),
        )
            .into_response()
    })
}
