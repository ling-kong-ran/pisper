//! Release memory-assets 的记忆 HTTP 契约。
use crate::{ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/settings/memory",
            get(preference).patch(update_preference),
        )
        .route("/api/memory", get(dashboard))
        .route("/api/memory/candidates", get(candidates))
        .route("/api/memory/spaces", post(create_space))
        .route(
            "/api/memory/spaces/{id}",
            axum::routing::patch(update_space).delete(delete_space),
        )
        .route("/api/memory/nodes", post(create_memory))
        .route(
            "/api/memory/nodes/{id}",
            axum::routing::patch(update_memory).delete(delete_memory),
        )
        .route("/api/memory/candidates/reject-all", post(reject_all))
        .route(
            "/api/memory/candidates/{id}/{action}",
            post(resolve_candidate),
        )
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    #[serde(default)]
    space_id: String,
    #[serde(default)]
    query: String,
    limit: Option<usize>,
}
fn fail(error: anyhow::Error) -> ApiError {
    ApiError::bad_request(crate::security::redact_secret_text(&error.to_string()))
}
fn not_found(message: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", message)
}
fn body(value: &Value) -> Result<(), ApiError> {
    if value.is_object() {
        Ok(())
    } else {
        Err(ApiError::bad_request("Invalid memory input."))
    }
}
async fn dashboard(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Search>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .memory
            .lock()
            .map_err(|_| ApiError::internal("Memory store lock failed"))?
            .dashboard(&query.space_id, &query.query)
            .map_err(fail)?,
    ))
}
async fn candidates(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Search>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .memory
            .lock()
            .map_err(|_| ApiError::internal("Memory store lock failed"))?
            .candidate_inbox(query.limit.unwrap_or(5))
            .map_err(fail)?,
    ))
}
async fn create_space(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    body(&input)?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .memory
                .lock()
                .map_err(|_| ApiError::internal("Memory store lock failed"))?
                .create_space(&input)
                .map_err(fail)?,
        ),
    ))
}
async fn update_space(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    body(&input)?;
    let value = state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .update_space(&id, &input)
        .map_err(fail)?;
    value.map(Json).ok_or_else(|| not_found("星域不存在。"))
}
async fn delete_space(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .delete_space(&id)
        .map_err(fail)?
    {
        Ok(Json(json!({"deleted":true})))
    } else {
        Err(not_found("星域不存在。"))
    }
}
async fn create_memory(
    State(state): State<Arc<AppState>>,
    Json(mut input): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    body(&input)?;
    input["sourceType"] = json!("manual");
    let value = state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .remember(&input)
        .map_err(fail)?;
    state.memory_tasks.schedule_semantic();
    Ok((StatusCode::CREATED, Json(value)))
}
async fn update_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    body(&input)?;
    let value = state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .update_memory(&id, &input)
        .map_err(fail)?
        .ok_or_else(|| not_found("星辰不存在。"))?;
    state.memory_tasks.schedule_semantic();
    Ok(Json(value))
}
async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .forget(&id)
        .map_err(fail)?
    {
        Ok(Json(json!({"deleted":true})))
    } else {
        Err(not_found("星辰不存在。"))
    }
}
async fn reject_all(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .memory
            .lock()
            .map_err(|_| ApiError::internal("Memory store lock failed"))?
            .reject_all_candidates()
            .map_err(fail)?,
    ))
}
async fn resolve_candidate(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mut store = state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?;
    let result = match action.as_str() {
        "accept" => store.accept_candidate(&id),
        "reject" => store.reject_candidate(&id),
        _ => return Err(not_found("Unknown memory candidate action.")),
    }
    .map_err(fail)?;
    if action == "accept" {
        state.memory_tasks.schedule_semantic();
    }
    result
        .map(Json)
        .ok_or_else(|| not_found("候选记忆不存在或已处理。"))
}

async fn preference(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let confidence = state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .auto_approve_confidence
        * 100.0;
    Ok(Json(
        json!({"autoApproveConfidence": confidence.round() as u32,"minConfidence":0,"maxConfidence":100}),
    ))
}

async fn update_preference(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let requested = match input.get("autoApproveConfidence") {
        Some(Value::Number(value)) => value.as_f64(),
        Some(Value::String(value)) => {
            if value.trim().is_empty() {
                Some(0.0)
            } else {
                value.trim().parse().ok()
            }
        }
        Some(Value::Null) => Some(0.0),
        Some(Value::Bool(value)) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
    .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 100.0)
    .ok_or_else(|| ApiError::bad_request("记忆自动确认阈值必须在 0 到 100 之间。"))?;
    let confidence = requested.round();
    state
        .providers
        .update_app_preferences(&json!({"memoryAutoApproveConfidence": confidence as u32}))
        .await?;
    state
        .memory
        .lock()
        .map_err(|_| ApiError::internal("Memory store lock failed"))?
        .auto_approve_confidence = confidence / 100.0;
    preference(State(state)).await
}
