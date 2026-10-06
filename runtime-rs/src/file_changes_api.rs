//! HTTP 仅负责会话解析和运行/工作区锁；快照领域不依赖整个 AppState。
use crate::{native_file_changes::store, ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use pi_rust::coding_agent::session_manager::SessionManager;
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions/{id}/file-changes", get(list))
        .route("/api/sessions/{id}/change-summary", get(summary))
        .route("/api/sessions/{id}/file-changes/diff", get(diff))
        .route("/api/sessions/{id}/file-changes/revert", post(revert))
        .route("/api/sessions/{id}/file-changes/approve", post(approve))
}
fn cwd(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    let path = crate::session_api::find_session_path(state, id)?;
    let manager = SessionManager::open(&path, None, None)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(PathBuf::from(manager.get_cwd()))
}
async fn list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &id)?;
    let mut list = state.file_changes.list(&id, &cwd).await?;
    list["cwd"] = serde_json::json!(cwd.to_string_lossy());
    Ok(Json(list))
}
async fn summary(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &id)?;
    Ok(Json(state.file_changes.summary(&id, &cwd).await?))
}
async fn diff(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &id)?;
    let path = store::relative(
        &cwd,
        query.get("path").map(String::as_str).unwrap_or(""),
        true,
    )?;
    Ok(Json(state.file_changes.diff(&id, &cwd, &path).await?))
}
async fn revert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    if state.sessions.busy(&id) || state.goals.busy(&id) {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "session_busy",
            "当前会话正在运行，请完成或停止后再撤销改动。",
        ));
    }
    let _mutation = crate::session_runtime::mutation(&state, &id).await?;
    let cwd = cwd(&state, &id)?;
    let _workspace = state
        .asset_tracker
        .lock_workspace(&cwd, &CancellationToken::new())
        .await
        .map_err(|error| {
            ApiError::internal(crate::security::redact_secret_text(&error.to_string()))
        })?;
    let path = input["path"].as_str().filter(|path| !path.is_empty());
    Ok(Json(state.file_changes.revert(&id, &cwd, path).await?))
}
async fn approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &id)?;
    let path = input["path"].as_str().filter(|path| !path.is_empty());
    Ok(Json(state.file_changes.approve(&id, &cwd, path).await?))
}
