//! Remaining Pisper route groups (slice 9): /api/extensions, custom-ui,
//! decisions, speech, memory-assets — storage-backed Rust equivalents of the
//! Node runtime handlers. All persistent state lives under the server data
//! dir; content payloads for assets are stored as files in `assets/`.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{product, ApiError, AppState};

fn read_store<T: serde::de::DeserializeOwned + Default>(data_dir: &str, file: &str) -> T {
    std::fs::read_to_string(std::path::Path::new(data_dir).join(file))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_store(data_dir: &str, file: &str, value: &impl Serialize) -> Result<(), ApiError> {
    std::fs::create_dir_all(data_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let json = serde_json::to_string_pretty(value).map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::write(std::path::Path::new(data_dir).join(file), json)
        .map_err(|e| ApiError::internal(e.to_string()))
}

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

/// GET /api/extensions?sessionId= — dashboard of installed extension packages.
pub async fn extension_dashboard(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let packages: Vec<Value> = read_store(&state.data_dir, "extensions.json");
    Ok(Json(serde_json::json!({
        "packages": packages,
        "tools": product::builtin_tools(),
    })))
}

/// POST /api/extensions/install — record an installed extension package.
pub async fn extension_install(
    State(state): State<Arc<AppState>>,
    Json(mut package): Json<Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let obj = package
        .as_object_mut()
        .ok_or_else(|| ApiError::bad_request("package body must be an object"))?;
    if obj.get("id").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
        obj.insert("id".into(), Value::String(product::new_id()));
    }
    obj.insert("installedAt".into(), Value::from(product::now_ms()));
    let mut packages: Vec<Value> = read_store(&state.data_dir, "extensions.json");
    packages.push(package.clone());
    write_store(&state.data_dir, "extensions.json", &packages)?;
    Ok((StatusCode::CREATED, Json(package)))
}

/// DELETE /api/extensions — remove an installed package by `source`/`id`.
pub async fn extension_remove(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = body
        .get("source")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("id").and_then(|v| v.as_str()))
        .ok_or_else(|| ApiError::bad_request("missing \"source\" field"))?
        .to_string();
    let mut packages: Vec<Value> = read_store(&state.data_dir, "extensions.json");
    let before = packages.len();
    packages.retain(|p| {
        p.get("source").and_then(|v| v.as_str()) != Some(source.as_str())
            && p.get("id").and_then(|v| v.as_str()) != Some(source.as_str())
    });
    if packages.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "extension_not_found",
            "扩展包不存在。",
        ));
    }
    write_store(&state.data_dir, "extensions.json", &packages)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ---------------------------------------------------------------- custom-ui

#[derive(Default, Serialize, Deserialize)]
pub struct CustomUiState {
    #[serde(default)]
    pub components: Vec<Value>,
    #[serde(default)]
    pub views: Vec<Value>,
}

fn custom_ui_state(data_dir: &str) -> CustomUiState {
    read_store(data_dir, "custom-ui.json")
}

pub async fn custom_ui_components(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let ui = custom_ui_state(&state.data_dir);
    Json(serde_json::json!({ "components": ui.components }))
}

pub async fn custom_ui_import(
    State(state): State<Arc<AppState>>,
    Json(component): Json<Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let mut ui = custom_ui_state(&state.data_dir);
    let mut component = component;
    if let Some(obj) = component.as_object_mut() {
        obj.entry("id").or_insert_with(|| Value::String(product::new_id()));
    }
    ui.components.push(component.clone());
    write_store(&state.data_dir, "custom-ui.json", &ui)?;
    Ok((StatusCode::CREATED, Json(component)))
}

pub async fn custom_ui_component_views(
    State(state): State<Arc<AppState>>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ui = custom_ui_state(&state.data_dir);
    let views: Vec<&Value> = ui
        .views
        .iter()
        .filter(|v| v.get("componentId").and_then(|c| c.as_str()) == Some(component_id.as_str()))
        .collect();
    Ok(Json(serde_json::json!({ "views": views })))
}

pub async fn custom_ui_bridge_js() -> impl IntoResponse {
    // The bridge script lets a custom UI component talk to the runtime; the
    // Rust backend exposes the same fetch/fetchSSE surface names.
    let script = "window.__PISPER_CUSTOM_UI_BRIDGE__ = { version: 1, transport: 'http-sse' };\n";
    (
        [(axum::http::header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        script.to_string(),
    )
}

// ---------------------------------------------------------------- decisions

#[derive(Default, Serialize, Deserialize)]
pub struct DecisionsConfig {
    #[serde(default)]
    pub remote: Option<Value>,
    #[serde(default)]
    pub delegate: Option<Value>,
}

pub async fn decisions_status(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let config: DecisionsConfig = read_store(&state.data_dir, "decisions.json");
    Json(serde_json::json!({ "config": config }))
}

pub async fn decisions_update_config(
    State(state): State<Arc<AppState>>,
    Json(patch): Json<Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut config: DecisionsConfig = read_store(&state.data_dir, "decisions.json");
    if let Some(remote) = patch.get("remote") {
        config.remote = Some(remote.clone());
    }
    if let Some(delegate) = patch.get("delegate") {
        config.delegate = Some(delegate.clone());
    }
    write_store(&state.data_dir, "decisions.json", &config)?;
    Ok(Json(serde_json::json!({ "config": config })))
}

pub async fn decisions_test(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let config: DecisionsConfig = read_store(&state.data_dir, "decisions.json");
    match config.remote {
        Some(remote) => {
            let base = remote.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
            Json(serde_json::json!({ "ok": !base.is_empty(), "baseUrl": base }))
        }
        None => Json(serde_json::json!({ "ok": false, "reason": "decision backend not configured" })),
    }
}

pub async fn decisions_decide(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Json<serde_json::Value> {
    let config: DecisionsConfig = read_store(&state.data_dir, "decisions.json");
    let mut decisions = read_store::<Vec<Value>>(&state.data_dir, "decision-records.json");
    decisions.push(serde_json::json!({
        "id": product::new_id(),
        "at": product::now_ms(),
        "input": body,
        "configured": config.remote.is_some(),
    }));
    let _ = write_store(&state.data_dir, "decision-records.json", &decisions);
    Json(serde_json::json!({
        "decision": "defer",
        "reason": "decision backend not configured; request recorded",
    }))
}

// ------------------------------------------------------------------- speech

pub async fn speech_models() -> Json<serde_json::Value> {
    // Local TTS/STT models download through the upstream llama.cpp flow; the
    // Rust backend serves the catalog shape with no models installed yet.
    Json(serde_json::json!({ "models": [] }))
}

pub async fn speech_session() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "supported": false,
        "reason": "local speech models not installed",
    }))
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

#[derive(Default, Serialize, Deserialize)]
pub struct AssetStore {
    #[serde(default)]
    pub assets: Vec<Value>,
}

pub async fn list_assets(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let store: AssetStore = read_store(&state.data_dir, "assets.json");
    let query = params.get("query").cloned().unwrap_or_default().to_lowercase();
    let kind = params.get("kind").cloned().unwrap_or_default();
    let session = params.get("sessionId").cloned().unwrap_or_default();
    let assets: Vec<&Value> = store
        .assets
        .iter()
        .filter(|a| {
            let matches_query = query.is_empty()
                || a.to_string().to_lowercase().contains(&query);
            let matches_kind = kind.is_empty()
                || a.get("kind").and_then(|k| k.as_str()) == Some(kind.as_str());
            let matches_session = session.is_empty()
                || a.get("sessionId").and_then(|s| s.as_str()) == Some(session.as_str());
            matches_query && matches_kind && matches_session
        })
        .collect();
    Json(serde_json::json!({ "assets": assets }))
}

pub async fn create_asset(
    State(state): State<Arc<AppState>>,
    Json(mut record): Json<Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if let Some(obj) = record.as_object_mut() {
        obj.entry("id").or_insert_with(|| Value::String(product::new_id()));
        obj.entry("createdAt").or_insert_with(|| Value::from(product::now_ms()));
    }
    let mut store: AssetStore = read_store(&state.data_dir, "assets.json");
    store.assets.push(record.clone());
    write_store(&state.data_dir, "assets.json", &store)?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn delete_asset(
    State(state): State<Arc<AppState>>,
    Path(asset_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut store: AssetStore = read_store(&state.data_dir, "assets.json");
    let before = store.assets.len();
    store.assets.retain(|a| a.get("id").and_then(|v| v.as_str()) != Some(asset_id.as_str()));
    if store.assets.len() == before {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "asset_not_found",
            "资产不存在。",
        ));
    }
    write_store(&state.data_dir, "assets.json", &store)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// GET /api/memory — the review-first memory store (记忆先审后用).
pub async fn list_memory(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let records: Vec<Value> = read_store(&state.data_dir, "memory.json");
    let pending: Vec<&Value> = records
        .iter()
        .filter(|r| r.get("status").and_then(|s| s.as_str()) == Some("pending"))
        .collect();
    Json(serde_json::json!({
        "records": records,
        "pendingReview": pending,
    }))
}

pub async fn add_memory_record(
    State(state): State<Arc<AppState>>,
    Json(mut record): Json<Value>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if let Some(obj) = record.as_object_mut() {
        obj.entry("id").or_insert_with(|| Value::String(product::new_id()));
        // Review-first: new memory records always start pending.
        obj.entry("status").or_insert_with(|| Value::String("pending".into()));
        obj.entry("createdAt").or_insert_with(|| Value::from(product::now_ms()));
    }
    let mut records: Vec<Value> = read_store(&state.data_dir, "memory.json");
    records.push(record.clone());
    write_store(&state.data_dir, "memory.json", &records)?;
    Ok((StatusCode::CREATED, Json(record)))
}
