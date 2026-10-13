//! 扩展市场、决策路由适配与文件目录列表。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::Value;

use crate::{ApiError, AppState};

// --------------------------------------------------------------- extensions

/// GET /api/extensions/market — the upstream marketplace is a remote index;
/// without network access this serves an empty catalog (honest shape).
pub async fn extension_market(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "packages": [],
        "query": params.get("name").cloned().unwrap_or_default(),
        "page": params.get("page").cloned().unwrap_or_else(|| "1".into()),
        "total": 0,
    }))
}

// ---------------------------------------------------------------- decisions
// 完整决策领域已原生移植到 native_decisions(release decision-*.mjs 契约):
// 状态默认视图、配置白名单校验(apiKey 只写)、真实远端 typesafe-decisions 协议。

pub async fn decisions_status(
    state: axum::extract::State<std::sync::Arc<AppState>>,
) -> Json<serde_json::Value> {
    crate::native_decisions::status(state).await
}

pub async fn decisions_update_config(
    state: axum::extract::State<std::sync::Arc<AppState>>,
    body: Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::native_decisions::update_config(state, body).await
}

pub async fn decisions_test(
    state: axum::extract::State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::native_decisions::test_connection(state).await
}

pub async fn decisions_decide(
    state: axum::extract::State<std::sync::Arc<AppState>>,
    body: Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::native_decisions::decide(state, body).await
}

// ------------------------------------------------------- memory-assets / fs

fn safe_join(root: &str, requested: Option<&str>) -> std::path::PathBuf {
    match requested.filter(|s| !s.is_empty()) {
        Some(path) => {
            let candidate = std::path::Path::new(path);
            // Workspace-boundary: only allow paths inside the workspace root.
            match candidate.strip_prefix(root) {
                Ok(rel) => std::path::Path::new(root).join(rel),
                Err(_) => std::path::PathBuf::from(root),
            }
        }
        None => std::path::PathBuf::from(root),
    }
}

pub async fn list_directories(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let root = safe_join(&state.cwd, params.get("path").map(String::as_str));
    let mut dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    dirs.sort();
    Json(serde_json::json!({ "directories": dirs, "root": root.to_string_lossy() }))
}

pub async fn list_workspace_entries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let root = safe_join(&state.cwd, params.get("path").map(String::as_str));
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(&root) {
        for entry in read.flatten() {
            let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "directory"
            } else {
                "file"
            };
            entries.push(serde_json::json!({
                "name": entry.file_name().to_string_lossy(),
                "type": kind,
            }));
        }
    }
    entries.sort_by(|a, b| {
        a.get("name")
            .and_then(|n| n.as_str())
            .cmp(&b.get("name").and_then(|n| n.as_str()))
    });
    Json(serde_json::json!({ "entries": entries, "root": root.to_string_lossy() }))
}
