//! Release visual configuration HTTP boundary. The host supplies only the visual
//! service and its data directory, never the whole application/session state.
use crate::native_visual::{VisualError, VisualGenerationService, VisualKind, VisualOptions};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

const MAX_JSON_BODY: usize = 32_000_000;
struct Api {
    service: Arc<VisualGenerationService>,
    data_dir: PathBuf,
}
struct Failure(VisualError);
impl From<VisualError> for Failure {
    fn from(error: VisualError) -> Self {
        Self(error)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let error = self.0;
        let status = error
            .status
            .filter(|status| (400..600).contains(status))
            .and_then(|status| StatusCode::from_u16(status).ok())
            .unwrap_or(StatusCode::BAD_REQUEST);
        (
            status,
            Json(json!({"error":crate::security::redact_secret_text(&error.message)})),
        )
            .into_response()
    }
}
type Result<T> = std::result::Result<T, Failure>;

struct ParsedInput(Option<Value>);
impl Drop for ParsedInput {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            crate::visual_request_json::dispose(value);
        }
    }
}

/// `data_dir` is the Runtime data root. The service itself adds `visual-test`.
/// Authentication/origin policy and runtime shutdown remain host responsibilities.
pub(crate) fn router(service: Arc<VisualGenerationService>, data_dir: PathBuf) -> Router {
    Router::new()
        .route(
            "/api/visual/models",
            get(models).head(not_found).fallback(not_found),
        )
        .route(
            "/api/visual/models/{kind}",
            axum::routing::put(preference).fallback(not_found),
        )
        .route("/api/visual/test", post(test).fallback(not_found))
        .with_state(Arc::new(Api { service, data_dir }))
}
async fn not_found() -> (StatusCode, Json<Value>) {
    (StatusCode::NOT_FOUND, Json(json!({"error":"接口不存在。"})))
}
async fn models(State(api): State<Arc<Api>>) -> Result<Json<Value>> {
    Ok(Json(api.service.get_all_status().await?))
}
async fn preference(
    State(api): State<Arc<Api>>,
    Path(kind): Path<String>,
    request: Request,
) -> Result<Response> {
    // The oracle's route registry constrains the kind before reading the body.
    let kind = match kind.as_str() {
        "image" => VisualKind::Image,
        "video" => VisualKind::Video,
        _ => {
            return Ok(
                (StatusCode::NOT_FOUND, Json(json!({"error":"接口不存在。"}))).into_response(),
            )
        }
    };
    // Node bodyJson ignores Content-Type, accepts an empty body as {}, and uses
    // Buffer.toString('utf8') replacement decoding before JSON.parse.
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| Failure(VisualError::new("请求体过大。")))?;
    let input = ParsedInput(Some(if bytes.is_empty() {
        json!({})
    } else {
        crate::visual_request_json::parse(&String::from_utf8_lossy(&bytes))
            .map_err(|error| Failure(VisualError::new(error)))?
    }));
    // JS input?.model is undefined for null and JSON primitive/array bodies;
    // the service implements String(requestedModel || '').trim() coercion.
    let requested = input
        .0
        .as_ref()
        .expect("owned JSON input")
        .as_object()
        .and_then(|input| input.get("model"))
        .unwrap_or(&Value::Null);
    let requested = crate::visual_request_json::checked_string_like_model(requested)
        .map_err(|error| Failure(VisualError::new(error)))?;
    Ok(Json(
        api.service
            .set_preferred_model(kind, &Value::String(requested))
            .await?,
    )
    .into_response())
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
async fn test(State(api): State<Arc<Api>>) -> Result<Json<Value>> {
    // Release POST /visual/test intentionally ignores the request body; callers
    // cannot replace the fixed prompt/model/kind or redirect the output cwd.
    let options = VisualOptions::default();
    let _owner = CancelOnDrop(options.cancellation.clone());
    Ok(Json(api.service.test_visual(&api.data_dir, options).await?))
}

#[cfg(test)]
#[path = "visual_api_tests.rs"]
mod tests;
