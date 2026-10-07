//! 桌面界面启动协议与本机界面偏好。
//!
//! 已实现的读写必须落盘；尚未接入的领域返回稳定 unsupported 错误，
//! 不能把简化的内部存储形状当成 React 页面可以使用的完整服务。

use std::{io::Write, sync::Mutex};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{any, get},
    Json, Router,
};
use serde_json::{json, Map, Value};

use crate::{ApiError, AppState};

const PREFERENCE_KEYS: [&str; 17] = [
    "pisper-ui",
    "pisper-session-context-layout",
    "pisper-composer-toolbar",
    "pisper-zcode-composer-toolbar",
    "pisper-workspace-order",
    "pisper-floating-widgets",
    "pisper-floating-placement",
    "pisper-language",
    "pisper-shortcuts",
    "pisper-active-session",
    "pisper-mobile-session-tabs",
    "pisper-terminal-panel",
    "pisper-sponsor-dismissals",
    "pisper-model-onboarding-v1-dismissed",
    "pisper.config.manageConnectionsOpen",
    "pisper.config.visualConnectionsOpen",
    "pisper-web-desktop-pet-position",
];
const MAX_VALUE_BYTES: usize = 512 * 1024;
const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub(crate) struct UiState {
    mutation: Mutex<()>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            mutation: Mutex::new(()),
        }
    }
}

fn unsupported(feature: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "unsupported",
        format!("Rust 后端尚未提供完整的{feature}功能，当前操作不可用。"),
    )
}

pub(crate) fn capabilities() -> Value {
    let terminal = terminal_available(
        std::env::var("PISPER_RUNTIME_PROFILE").ok().as_deref(),
        std::env::var("PISPER_RUNTIME_PLATFORM").ok().as_deref(),
    );
    json!({
        "version": 1, "profile": "desktop", "engine": "pi-rs", "degraded": true,
        "modules": {"childProcess": true, "workerThreads": false, "sqlite": true, "wasm": false},
        "features": {
            "chat": true, "sessions": true, "providers": true, "filesystem": true,
            "assets": true, "skills": true, "webSearch": true, "visualGeneration": true,
            "imageProcessing": true, "imageAssets": true, "processes": false, "shell": true,
            "terminal": terminal, "vcs": true, "fileChanges": true, "memory": true, "workers": false,
            "plugins": false, "mcp": true, "goals": true, "plans": true,
            "multiAgent": true, "channels": true, "workflows": true, "schedules": true,
            "computerUse": false, "browserAutomation": false, "remoteAccess": false,
            "desktopPet": false,
        },
        "tools": ["read", "bash", "edit", "write"],
    })
}

// The PTY lives in the Tauri host. A plain Web client still requires the real
// desktop bridge before displaying it; mobile hosts must never advertise it.
fn terminal_available(profile: Option<&str>, platform: Option<&str>) -> bool {
    let desktop_profile = !matches!(
        profile.map(str::trim),
        Some("mobile-embedded" | "mobile-store")
    );
    cfg!(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "macos"
    )) && desktop_profile
        && platform != Some("ios")
}

/// root 合并前需移除旧 product/product2 的同路径简化接口。
pub(crate) fn router() -> Router<std::sync::Arc<AppState>> {
    Router::new()
        .route(
            "/api/client-info",
            get(|| async { Json(json!({"client":"web"})) }),
        )
        .route(
            "/api/runtime/capabilities",
            get(|| async { Json(capabilities()) }),
        )
        .route(
            "/api/local/browser-preferences",
            get(browser_preferences)
                .put(update_browser_preferences)
                .post(update_browser_preferences),
        )
        .route(
            "/api/settings/chat-dock-layout",
            get(chat_dock_layout).put(save_chat_dock_layout),
        )
        .route(
            "/api/settings/compaction",
            get(compaction_preference).patch(update_compaction_preference),
        )
        // JSON 中控制字符可能转义成六字节；值本身仍执行共享协议的真实字节上限。
        .layer(DefaultBodyLimit::max(MAX_TOTAL_BYTES * 6 + 16 * 1024))
}

fn read_json(directory: &str, name: &str, fallback: Value) -> Result<Value, ApiError> {
    match std::fs::read(std::path::Path::new(directory).join(name)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| ApiError::internal("界面偏好文件无效。"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback),
        Err(error) => Err(ApiError::internal(error.to_string())),
    }
}

fn write_json(directory: &str, name: &str, value: &Value) -> Result<(), ApiError> {
    std::fs::create_dir_all(directory).map_err(|e| ApiError::internal(e.to_string()))?;
    let path = std::path::Path::new(directory).join(name);
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut file =
        std::fs::File::create(&temporary).map_err(|e| ApiError::internal(e.to_string()))?;
    file.write_all(&bytes)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    file.sync_all()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|e| ApiError::internal(e.to_string()))
}

fn local_preferences_request(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.get("authorization").is_some() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "browser_preferences_unavailable",
            "本机偏好不向远程客户端开放。",
        ));
    }
    let Some(host) = headers.get("host").and_then(|value| value.to_str().ok()) else {
        return Ok(());
    };
    let host = host.to_ascii_lowercase();
    let local = host == "localhost"
        || host.starts_with("localhost:")
        || host == "127.0.0.1"
        || host.starts_with("127.0.0.1:")
        || host == "[::1]"
        || host.starts_with("[::1]:");
    if !local {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "browser_preferences_unavailable",
            "本机偏好不向远程客户端开放。",
        ));
    }
    if let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) {
        let localhost = host.replacen("127.0.0.1", "localhost", 1);
        let loopback = host.replacen("localhost", "127.0.0.1", 1);
        if ![
            format!("http://{host}"),
            format!("http://{localhost}"),
            format!("http://{loopback}"),
        ]
        .contains(&origin.to_string())
        {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "browser_preferences_origin",
                "本机偏好仅允许同源请求。",
            ));
        }
    }
    Ok(())
}

fn invalid_preferences() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "browser_preferences_invalid",
        "界面偏好字段、大小或修订号无效。",
    )
}

fn validate_values(value: &Value) -> Result<Map<String, Value>, ApiError> {
    let values = value.as_object().ok_or_else(invalid_preferences)?;
    if values.len() > PREFERENCE_KEYS.len() {
        return Err(invalid_preferences());
    }
    let mut total = 0;
    for (key, value) in values {
        if !PREFERENCE_KEYS.contains(&key.as_str()) || (!value.is_null() && !value.is_string()) {
            return Err(invalid_preferences());
        }
        if let Some(value) = value.as_str() {
            total += value.len();
            if value.len() > MAX_VALUE_BYTES || total > MAX_TOTAL_BYTES {
                return Err(invalid_preferences());
            }
        }
    }
    Ok(values.clone())
}

fn validate_snapshot(mut value: Value) -> Result<Value, ApiError> {
    let object = value.as_object_mut().ok_or_else(invalid_preferences)?;
    if object
        .keys()
        .any(|key| !["version", "values", "revisions"].contains(&key.as_str()))
        || object.get("version") != Some(&json!(1))
    {
        return Err(invalid_preferences());
    }
    // 退役布局不得阻断其余设置恢复，与 shared/browser-preferences.mjs 一致。
    if let Some(values) = object.get_mut("values").and_then(Value::as_object_mut) {
        values.remove("pisper-chat-layout");
    }
    if let Some(revisions) = object.get_mut("revisions").and_then(Value::as_object_mut) {
        revisions.remove("pisper-chat-layout");
    }
    let values = validate_values(&value["values"])?;
    let revisions = value.get("revisions").cloned().unwrap_or_else(|| json!({}));
    let revisions = revisions.as_object().ok_or_else(invalid_preferences)?;
    for (key, revision) in revisions {
        if !values.contains_key(key)
            || !revision
                .as_u64()
                .is_some_and(|number| number > 0 && number <= MAX_SAFE_INTEGER)
        {
            return Err(invalid_preferences());
        }
    }
    value["revisions"] = json!(revisions);
    Ok(value)
}

fn merge_preferences(current: Value, input: &Value) -> Result<Value, ApiError> {
    let object = input.as_object().ok_or_else(invalid_preferences)?;
    if object
        .keys()
        .any(|key| !["updates", "revisions"].contains(&key.as_str()))
    {
        return Err(invalid_preferences());
    }
    let updates = validate_values(&input["updates"])?;
    let revisions = match object.get("revisions") {
        Some(value) => {
            let revisions = value.as_object().ok_or_else(invalid_preferences)?;
            if revisions.len() != updates.len() {
                return Err(invalid_preferences());
            }
            for (key, revision) in revisions {
                if !updates.contains_key(key)
                    || !revision
                        .as_u64()
                        .is_some_and(|number| number > 0 && number <= MAX_SAFE_INTEGER)
                {
                    return Err(invalid_preferences());
                }
            }
            revisions.clone()
        }
        None => Map::new(),
    };
    let mut next = validate_snapshot(current)?;
    for (key, value) in updates {
        let previous = next["revisions"][&key].as_u64().unwrap_or(0);
        let revision = revisions
            .get(&key)
            .and_then(Value::as_u64)
            .unwrap_or_else(|| (crate::product::now_ms() * 1000).max(previous + 1));
        if revision <= previous {
            continue;
        }
        next["values"][&key] = value;
        next["revisions"][&key] = json!(revision);
    }
    validate_snapshot(next)
}

async fn browser_preferences(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    local_preferences_request(&headers)?;
    let _mutation = state
        .ui
        .mutation
        .lock()
        .map_err(|_| ApiError::internal("界面偏好锁无效。"))?;
    Ok(Json(validate_snapshot(read_json(
        &state.agent_dir,
        "pisper-browser-preferences.json",
        json!({"version":1,"values":{},"revisions":{}}),
    )?)?))
}

async fn update_browser_preferences(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<StatusCode, ApiError> {
    local_preferences_request(&headers)?;
    let _mutation = state
        .ui
        .mutation
        .lock()
        .map_err(|_| ApiError::internal("界面偏好锁无效。"))?;
    let current = read_json(
        &state.agent_dir,
        "pisper-browser-preferences.json",
        json!({"version":1,"values":{},"revisions":{}}),
    )?;
    let next = merge_preferences(current, &input)?;
    write_json(&state.agent_dir, "pisper-browser-preferences.json", &next)?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_dock_layout(value: &Value) -> Result<(), ApiError> {
    // release saveChatDockLayout：只要求是 JSON 对象（不能是数组）。
    if !value.is_object() {
        return Err(ApiError::bad_request("Dock 布局必须是 JSON 对象。"));
    }
    Ok(())
}

async fn chat_dock_layout(
    State(state): State<std::sync::Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let _mutation = state
        .ui
        .mutation
        .lock()
        .map_err(|_| ApiError::internal("界面偏好锁无效。"))?;
    Ok(Json(read_json(
        &state.agent_dir,
        "pisper-chat-dock-layout.json",
        Value::Null,
    )?))
}

async fn save_chat_dock_layout(
    State(state): State<std::sync::Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    validate_dock_layout(&input)?;
    let _mutation = state
        .ui
        .mutation
        .lock()
        .map_err(|_| ApiError::internal("界面偏好锁无效。"))?;
    write_json(&state.agent_dir, "pisper-chat-dock-layout.json", &input)?;
    Ok(Json(input))
}

// release compaction-policy.mjs: 10..95 区间，非法回退 80。
const MIN_COMPACTION_THRESHOLD_PERCENT: f64 = 10.0;
const MAX_COMPACTION_THRESHOLD_PERCENT: f64 = 95.0;
const DEFAULT_COMPACTION_THRESHOLD_PERCENT: f64 = 80.0;

fn normalize_compaction_threshold_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return DEFAULT_COMPACTION_THRESHOLD_PERCENT;
    }
    value
        .round()
        .clamp(MIN_COMPACTION_THRESHOLD_PERCENT, MAX_COMPACTION_THRESHOLD_PERCENT)
}

async fn compaction_preference(
    State(state): State<std::sync::Arc<crate::AppState>>,
) -> Result<Json<serde_json::Value>, crate::ApiError> {
    let threshold = state
        .providers
        .app_preferences()
        .ok()
        .and_then(|app| app["compactionThresholdPercent"].as_f64())
        .map(normalize_compaction_threshold_percent)
        .unwrap_or(DEFAULT_COMPACTION_THRESHOLD_PERCENT);
    Ok(Json(json!({
        "thresholdPercent": threshold as u32,
        "minPercent": MIN_COMPACTION_THRESHOLD_PERCENT as u32,
        "maxPercent": MAX_COMPACTION_THRESHOLD_PERCENT as u32,
    })))
}

async fn update_compaction_preference(
    State(state): State<std::sync::Arc<crate::AppState>>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, crate::ApiError> {
    let requested = input
        .get("thresholdPercent")
        .and_then(|value| value.as_f64())
        .ok_or_else(|| {
            crate::ApiError::bad_request("自动压缩阈值必须在 10% 到 95% 之间。")
        })?;
    if !requested.is_finite()
        || requested < MIN_COMPACTION_THRESHOLD_PERCENT
        || requested > MAX_COMPACTION_THRESHOLD_PERCENT
    {
        return Err(crate::ApiError::bad_request(
            "自动压缩阈值必须在 10% 到 95% 之间。",
        ));
    }
    let threshold = normalize_compaction_threshold_percent(requested);
    state
        .providers
        .update_app_preferences(&json!({"compactionThresholdPercent": threshold as u32}))
        .await?;
    compaction_preference(State(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_preference_revision_prevents_late_write_and_keeps_tombstones() {
        let current = json!({"version":1,"values":{"pisper-language":"zh-CN"},"revisions":{"pisper-language":10}});
        let stale = merge_preferences(
            current.clone(),
            &json!({"updates":{"pisper-language":"en-US"},"revisions":{"pisper-language":9}}),
        )
        .unwrap();
        assert_eq!(stale, current);
        let next = merge_preferences(
            current,
            &json!({"updates":{"pisper-language":null},"revisions":{"pisper-language":11}}),
        )
        .unwrap();
        assert_eq!(next["values"]["pisper-language"], Value::Null);
        assert_eq!(next["revisions"]["pisper-language"], 11);
    }

    #[test]
    fn browser_preferences_reject_unregistered_secrets_large_values_and_invalid_revision() {
        assert!(validate_values(&json!({"apiKey":"secret"})).is_err());
        assert!(validate_values(&json!({"pisper-ui":"x".repeat(MAX_VALUE_BYTES + 1)})).is_err());
        let current = json!({"version":1,"values":{},"revisions":{}});
        assert!(merge_preferences(
            current.clone(),
            &json!({"updates":{"pisper-language":"en"},"revisions":{"pisper-language":0}})
        )
        .is_err());
        assert!(merge_preferences(
            current,
            &json!({"updates":{"pisper-language":"en"},"revisions":{}})
        )
        .is_err());
    }

    #[test]
    fn capabilities_are_nested_and_unfinished_domains_are_explicitly_unavailable() {
        let value = capabilities();
        for feature in [
            "chat",
            "sessions",
            "providers",
            "mcp",
            "skills",
            "memory",
            "assets",
            "imageProcessing",
            "imageAssets",
            "webSearch",
            "visualGeneration",
            "goals",
            "plans",
            "multiAgent",
            "workflows",
            "schedules",
            "channels",
        ] {
            assert_eq!(value["features"][feature], true);
        }
        for feature in ["desktopPet", "plugins", "remoteAccess"] {
            assert_eq!(value["features"][feature], false);
        }
    }

    #[test]
    fn native_desktop_terminal_is_available_but_mobile_host_profiles_are_not() {
        let desktop = cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "macos"
        ));
        assert_eq!(terminal_available(None, None), desktop);
        assert_eq!(terminal_available(Some("desktop"), None), desktop);
        assert_eq!(terminal_available(Some("unknown-profile"), None), desktop);
        assert!(!terminal_available(Some(" mobile-embedded "), None));
        assert!(!terminal_available(Some("mobile-store"), None));
        assert!(!terminal_available(Some("desktop"), Some("ios")));
    }

    #[test]
    fn same_origin_is_required_for_local_preferences() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:12345".parse().unwrap());
        headers.insert("origin", "http://evil.invalid".parse().unwrap());
        assert!(local_preferences_request(&headers).is_err());
        headers.insert("origin", "http://localhost:12345".parse().unwrap());
        assert!(local_preferences_request(&headers).is_ok());
        headers.insert("authorization", "Bearer remote-token".parse().unwrap());
        assert!(local_preferences_request(&headers).is_err());
    }

    #[test]
    fn dock_layout_requires_an_actual_dockview_envelope() {
        // release saveChatDockLayout 只要求 JSON 对象（任意结构，不能是数组）。
        assert!(validate_dock_layout(&json!({"version":1,"engine":"dockview","activePanelId":"session:a","layout":{"grid":{},"panels":{}}})).is_ok());
        assert!(validate_dock_layout(&json!({"panels":{}})).is_ok());
        assert!(validate_dock_layout(&json!(["not","an","object"])).is_err());
        assert!(validate_dock_layout(&json!("string")).is_err());
        assert!(validate_dock_layout(&json!(null)).is_err());
    }
}
