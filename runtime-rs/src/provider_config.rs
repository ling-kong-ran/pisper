//! React Provider 配置契约与 Pi 模型运行时之间的适配。
//!
//! 保留原 Pisper 的四份 JSON 文档及未知字段；凭据只进入 auth.json，
//! 配置响应只返回认证状态。文件读取错误必须阻止写入，避免损坏文档被空对象覆盖。

mod discovery;

use crate::{ApiError, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use pi_rust::{
    ai::{models::ModelsRefreshOptions, types::Model},
    coding_agent::core::{
        agent_session_runtime::AgentSessionRuntime,
        provider_composer::{ExtensionModelDefinition, ProviderConfigInput},
    },
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

const KNOWN_PROVIDERS: &[(&str, &str)] = &[
    ("openai", "OpenAI"),
    ("openai-codex", "OpenAI Codex"),
    ("anthropic", "Anthropic"),
    ("google", "Google"),
    ("deepseek", "DeepSeek"),
    ("xai", "xAI"),
    ("openrouter", "OpenRouter"),
    ("kimi-coding", "Kimi Code"),
    ("zai-coding-cn", "GLM"),
];
const PROTOCOLS: &[&str] = &[
    "openai-responses",
    "openai-completions",
    "anthropic-messages",
    "google-generative-ai",
];
const LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub(crate) struct ProviderConfigStore {
    agent_dir: PathBuf,
    write: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct Documents {
    models: Value,
    app: Value,
    settings: Value,
    auth: Value,
}

impl ProviderConfigStore {
    /// 所有 pisper.json 领域变更共用写入锁，并只持久化真正改变的文档。
    pub(crate) async fn mutate_app_preferences<F>(&self, mutation: F) -> Result<Value, ApiError>
    where
        F: FnOnce(&mut Value) -> Result<(), ApiError> + Send,
    {
        let _write = self.write.lock().await;
        let old = self.read()?;
        let mut documents = old.clone();
        mutation(&mut documents.app)?;
        if !documents.app.is_object() {
            return Err(ApiError::bad_request("Invalid application preferences"));
        }
        self.persist(&old, &documents)?;
        Ok(documents.app)
    }
    /// Notification settings share pisper.json with provider and application
    /// preferences. Keep the same writer so concurrent updates cannot replace
    /// each other's unrelated fields or credentials.
    pub(crate) async fn update_browser_notifications_enabled(
        &self,
        enabled: bool,
    ) -> Result<Value, ApiError> {
        let _write = self.write.lock().await;
        let old = self.read()?;
        let mut documents = old.clone();
        let app = documents
            .app
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("Invalid application preferences"))?;
        let notifications = app.entry("notifications").or_insert_with(|| json!({}));
        if !notifications.is_object() {
            *notifications = json!({});
        }
        let browser = notifications
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("Invalid notification preferences"))?
            .entry("browser")
            .or_insert_with(|| json!({}));
        if !browser.is_object() {
            *browser = json!({});
        }
        browser["enabled"] = json!(enabled);
        self.persist(&old, &documents)?;
        Ok(documents.app)
    }

    pub(crate) async fn update_app_preferences(&self, patch: &Value) -> Result<Value, ApiError> {
        let patch = patch
            .as_object()
            .ok_or_else(|| ApiError::bad_request("Invalid application preferences"))?;
        let _write = self.write.lock().await;
        let old = self.read()?;
        let mut documents = old.clone();
        let app = documents
            .app
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("Invalid application preferences"))?;
        app.extend(
            patch
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        self.persist(&old, &documents)?;
        Ok(documents.app)
    }

    pub(crate) fn app_preferences(&self) -> Result<Value, ApiError> {
        Ok(self.read()?.app)
    }

    /// 视觉目录沿用 release 的三个原始 JSON 输入；与规范配置写入共用锁，
    /// 不把无关 settings.json 的解析失败变成图像或视频目录错误。
    pub(crate) async fn visual_documents(&self) -> Result<(Value, Value, Value), ApiError> {
        let _write = self.write.lock().await;
        Ok((
            read_visual_document(&self.agent_dir.join("models.json"), json!({"providers":{}}))?,
            read_visual_document(&self.agent_dir.join("auth.json"), json!({}))?,
            read_visual_document(
                &self.agent_dir.join("pisper.json"),
                json!({
                    "disabledProviders":[], "providerTypes":{}
                }),
            )?,
        ))
    }

    /// 与其它配置领域共用规范写锁，但视觉偏好只依赖、只替换 pisper.json。
    /// 清空偏好不得先读取无关模型、凭据或 settings 文档。
    pub(crate) async fn mutate_visual_preferences<F>(&self, mutation: F) -> Result<Value, ApiError>
    where
        F: FnOnce(Value) -> Result<Value, ApiError> + Send,
    {
        let _write = self.write.lock().await;
        let path = self.agent_dir.join("pisper.json");
        let app = read_visual_document(&path, json!({"visualDefaultModels":{}}))?;
        let app = mutation(app)?;
        if !app.is_object() {
            return Err(ApiError::bad_request("Invalid application preferences"));
        }
        write_object(&path, &app)?;
        Ok(app)
    }

    pub(crate) fn new(agent_dir: &str) -> Self {
        Self {
            agent_dir: agent_dir.into(),
            write: tokio::sync::Mutex::new(()),
        }
    }

    fn read(&self) -> Result<Documents, ApiError> {
        Ok(Documents {
            models: read_object(&self.agent_dir.join("models.json"))?,
            app: read_object(&self.agent_dir.join("pisper.json"))?,
            settings: read_object(&self.agent_dir.join("settings.json"))?,
            auth: read_object(&self.agent_dir.join("auth.json"))?,
        })
    }

    fn persist(&self, old: &Documents, new: &Documents) -> Result<(), ApiError> {
        for (name, before, after) in [
            ("models.json", &old.models, &new.models),
            ("pisper.json", &old.app, &new.app),
            ("settings.json", &old.settings, &new.settings),
            ("auth.json", &old.auth, &new.auth),
        ] {
            if before != after {
                write_object(&self.agent_dir.join(name), after)?;
            }
        }
        Ok(())
    }

    /// 启动时使用同一套应用配置恢复默认模型，兼容 Node 保存的连接。
    pub(crate) async fn restore(&self, runtime: &AgentSessionRuntime) -> Result<(), String> {
        let docs = self.read().map_err(|error| error.message)?;
        let session = runtime.session();
        let context = session
            .session_manager
            .lock()
            .map_err(|_| "Session lock failed")?
            .build_session_context();
        reload_engine(runtime, &docs)
            .await
            .map_err(|error| error.message)?;
        if let (Ok(provider), Ok(key)) = (
            std::env::var("PISPER_RS_PROVIDER"),
            std::env::var("PISPER_RS_API_KEY"),
        ) {
            if !provider.is_empty() && !key.is_empty() {
                session
                    .model_runtime()
                    .set_runtime_api_key(&provider, &key, None)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        let (provider, model) = context
            .model
            .as_ref()
            .map(|selected| (selected.provider.as_str(), selected.model_id.as_str()))
            .unwrap_or_else(|| {
                (
                    string(&docs.settings, "defaultProvider"),
                    string(&docs.settings, "defaultModel"),
                )
            });
        let explicit_override = ["PISPER_RS_PROVIDER", "PISPER_RS_MODEL"]
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()));
        if !explicit_override
            && !provider.is_empty()
            && !model.is_empty()
            && model_allowed(&docs, provider, model)
        {
            if let Some(model) = runtime
                .session()
                .model_runtime()
                .get_model(provider, model)
                .await
            {
                runtime
                    .session()
                    .set_model(model, None)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

fn read_object(path: &FsPath) -> Result<Value, ApiError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = pi_rust::coding_agent::utils::json::strip_json_comments(
                text.trim_start_matches('\u{feff}'),
            );
            let value: Value = serde_json::from_str(&text).map_err(|_| {
                ApiError::internal(format!(
                    "{} contains invalid JSON",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ))
            })?;
            if !value.is_object() {
                return Err(ApiError::internal("configuration must be a JSON object"));
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(ApiError::internal(format!(
            "cannot read configuration: {error}"
        ))),
    }
}

fn read_visual_document(path: &FsPath, fallback: Value) -> Result<Value, ApiError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|_| {
            ApiError::internal(format!(
                "{} contains invalid JSON",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        }),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(fallback)
        }
        Err(error) => Err(ApiError::internal(format!(
            "cannot read configuration: {error}"
        ))),
    }
}

fn write_object(path: &FsPath, value: &Value) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("configuration parent missing"))?;
    std::fs::create_dir_all(parent).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut entropy = [0_u8; 8];
    getrandom::getrandom(&mut entropy).map_err(|e| ApiError::internal(e.to_string()))?;
    let suffix: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = parent.join(format!(".pisper-config-{suffix}.tmp"));
    let result = (|| -> Result<(), ApiError> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let bytes =
            serde_json::to_vec_pretty(value).map_err(|e| ApiError::internal(e.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| ApiError::internal(e.to_string()))?;
        drop(file);
        // Windows 的 rename 同样支持替换文件，写入失败时旧文件仍完整保留。
        std::fs::rename(&temporary, path).map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
fn known(id: &str) -> bool {
    KNOWN_PROVIDERS.iter().any(|(provider, _)| *provider == id)
}
fn overlay<'a>(docs: &'a Documents, id: &str) -> &'a Value {
    docs.models
        .get("providers")
        .and_then(|v| v.get(id))
        .unwrap_or(&Value::Null)
}
fn definition<'a>(docs: &'a Documents, provider: &str, model: &str) -> Option<&'a Value> {
    overlay(docs, provider)
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.iter().find(|item| string(item, "id") == model))
}
fn kind(value: &Value) -> &str {
    match string(value, "kind") {
        "image" => "image",
        "video" => "video",
        _ => "chat",
    }
}
fn model_allowed(docs: &Documents, provider: &str, model: &str) -> bool {
    !array(&docs.app, "disabledProviders")
        .iter()
        .any(|v| v.as_str() == Some(provider))
        && docs
            .app
            .get("providerTypes")
            .and_then(|v| v.get(provider))
            .and_then(Value::as_str)
            != Some("visual")
        && !array(overlay(docs, provider), "excludedModels")
            .iter()
            .any(|v| v.as_str() == Some(model))
        && definition(docs, provider, model)
            .map(kind)
            .unwrap_or("chat")
            == "chat"
}

pub(crate) async fn available_models(state: &AppState) -> Result<Vec<Model>, ApiError> {
    let docs = state.providers.read()?;
    Ok(state
        .runtime
        .session()
        .model_runtime()
        .get_available_snapshot()
        .into_iter()
        .filter(|model| model_allowed(&docs, &model.provider, &model.id))
        .collect())
}

fn engine_definition(model: Model) -> ExtensionModelDefinition {
    ExtensionModelDefinition {
        id: model.id,
        name: model.name,
        api: Some(model.api),
        base_url: Some(model.base_url),
        reasoning: model.reasoning,
        thinking_level_map: model.thinking_level_map,
        input: model.input,
        cost: model.cost,
        context_window: model.context_window,
        max_tokens: model.max_tokens,
        sampling_params: model.sampling_params,
        sampling_params_by_thinking_level: model.sampling_params_by_thinking_level,
        headers: model.headers.map(|headers| {
            headers
                .into_iter()
                .filter_map(|(name, value)| value.map(|value| (name, value)))
                .collect()
        }),
        compat: model.compat,
    }
}

async fn reload_engine(runtime: &AgentSessionRuntime, docs: &Documents) -> Result<(), ApiError> {
    let session = runtime.session();
    let engine = session.model_runtime();
    // 先移除上次用于过滤的注册，避免被旧注册覆盖刚保存的模型参数。
    for id in engine.get_registered_provider_ids() {
        engine.unregister_provider(&id).await;
    }
    engine
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            ..Default::default()
        })
        .await
        .map_err(ApiError::internal)?;
    if let Some(error) = engine.get_error() {
        return Err(ApiError::bad_request(error));
    }
    for provider in engine.get_providers().await {
        let models = engine.get_models(Some(provider.id())).await;
        let filtered: Vec<_> = models
            .iter()
            .filter(|model| model_allowed(docs, &model.provider, &model.id))
            .cloned()
            .collect();
        if filtered.len() != models.len() {
            // Pisper 的 kind/excludedModels 是产品字段，Pi 不会自动应用这些过滤。
            engine
                .register_provider_sync(
                    provider.id(),
                    ProviderConfigInput {
                        models: Some(filtered.into_iter().map(engine_definition).collect()),
                        ..Default::default()
                    },
                )
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
    }
    session.settings_manager.reload();
    Ok(())
}

fn model_view(model: &Model, metadata: Option<&Value>) -> Value {
    let metadata = metadata.unwrap_or(&Value::Null);
    let resolved_kind = kind(metadata);
    let levels: Vec<&str> = if !model.reasoning || resolved_kind != "chat" {
        vec!["off"]
    } else {
        LEVELS
            .iter()
            .copied()
            .filter(|level| {
                match model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|map| map.get(*level))
                {
                    Some(None) => false,
                    Some(Some(_)) => true,
                    None => !matches!(*level, "xhigh" | "max"),
                }
            })
            .collect()
    };
    json!({ "id": model.id, "name": model.name, "kind": resolved_kind,
        "capabilities": metadata.get("capabilities").cloned().unwrap_or_else(|| json!([resolved_kind])),
        "input": model.input, "maxTokens": model.max_tokens, "api": model.api, "reasoning": model.reasoning,
        "thinkingLevels": levels, "contextWindow": model.context_window, "baseUrl": model.base_url,
        "baseUrlOverride": string(metadata, "baseUrl"), "added": metadata.get("userConfigured").and_then(Value::as_bool).unwrap_or(false) })
}

async fn snapshot(state: &AppState, docs: &Documents) -> Result<Value, ApiError> {
    let session = state.runtime.session();
    let engine = session.model_runtime();
    let mut ids: Vec<String> = KNOWN_PROVIDERS
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    if let Some(providers) = docs.models.get("providers").and_then(Value::as_object) {
        for id in providers.keys() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    for model in engine.get_available_snapshot() {
        if !ids.contains(&model.provider) {
            ids.push(model.provider);
        }
    }
    let mut providers = Vec::new();
    for id in ids {
        let connection = overlay(docs, &id);
        let explicit_type = docs
            .app
            .get("providerTypes")
            .and_then(|v| v.get(&id))
            .and_then(Value::as_str);
        let provider_type = explicit_type.unwrap_or("chat");
        let excluded = array(connection, "excludedModels");
        let mut models: Vec<Value> = engine
            .get_models(Some(&id))
            .await
            .into_iter()
            .filter(|model| {
                !excluded
                    .iter()
                    .any(|v| v.as_str() == Some(model.id.as_str()))
            })
            .map(|model| model_view(&model, definition(docs, &id, &model.id)))
            .collect();
        // 已禁用的供应商和视觉定义仍需在配置页可见，才能再次启用、编辑或删除。
        for item in array(connection, "models") {
            if models
                .iter()
                .any(|model| string(model, "id") == string(&item, "id"))
                || excluded
                    .iter()
                    .any(|v| v.as_str() == Some(string(&item, "id")))
            {
                continue;
            }
            let mut view = json!({});
            for field in [
                "id",
                "name",
                "input",
                "maxTokens",
                "api",
                "reasoning",
                "contextWindow",
                "added",
            ] {
                if let Some(value) = item.get(field) {
                    view[field] = value.clone();
                }
            }
            view["kind"] = json!(kind(&item));
            view["capabilities"] = item
                .get("capabilities")
                .cloned()
                .unwrap_or_else(|| json!([kind(&item)]));
            view["thinkingLevels"] = item
                .get("thinkingLevels")
                .cloned()
                .unwrap_or_else(|| json!(["off"]));
            view["baseUrlOverride"] = json!(string(&item, "baseUrl"));
            models.push(view);
        }
        if provider_type == "visual" {
            models.retain(|model| kind(model) != "chat");
        }
        models.sort_by(|left, right| string(left, "id").cmp(string(right, "id")));
        let preferred = docs
            .app
            .get("providerDefaultModels")
            .and_then(|v| v.get(&id))
            .and_then(Value::as_str)
            .or_else(|| {
                (string(&docs.settings, "defaultProvider") == id)
                    .then(|| string(&docs.settings, "defaultModel"))
            })
            .unwrap_or("");
        let default_model = models
            .iter()
            .find(|model| kind(model) == "chat" && string(model, "id") == preferred)
            .or_else(|| models.iter().find(|model| kind(model) == "chat"))
            .map(|model| string(model, "id"))
            .unwrap_or("");
        let runtime_provider = engine.get_provider(&id).await;
        let label = if !string(connection, "name").is_empty() {
            string(connection, "name").to_string()
        } else {
            KNOWN_PROVIDERS
                .iter()
                .find(|(provider, _)| *provider == id)
                .map(|(_, name)| name.to_string())
                .or_else(|| {
                    runtime_provider
                        .as_ref()
                        .map(|provider| provider.name().to_string())
                })
                .unwrap_or_else(|| id.clone())
        };
        let api = if string(connection, "api").is_empty() {
            models
                .first()
                .map(|v| string(v, "api"))
                .filter(|s| !s.is_empty())
                .unwrap_or("openai-responses")
        } else {
            string(connection, "api")
        };
        let base_url = if string(connection, "baseUrl").is_empty() {
            models.first().map(|v| string(v, "baseUrl")).unwrap_or("")
        } else {
            string(connection, "baseUrl")
        };
        providers.push(json!({ "id": id, "name": label, "type": provider_type, "api": api, "models": models,
            "defaultModel": default_model, "baseUrl": base_url,
            "organization": connection.get("headers").and_then(|v| v.get("OpenAI-Organization")).and_then(Value::as_str).unwrap_or(""),
            "enabled": !array(&docs.app, "disabledProviders").iter().any(|v| v.as_str() == Some(id.as_str())),
            "configured": docs.auth.get(&id).is_some() || engine.has_configured_auth(&id), "custom": !known(&id) }));
    }
    let default_provider = string(&docs.settings, "defaultProvider");
    let default_model = string(&docs.settings, "defaultModel");
    let selected = providers
        .iter()
        .find(|v| {
            string(v, "id") == default_provider
                && v["enabled"] == true
                && v["configured"] == true
                && !string(v, "defaultModel").is_empty()
        })
        .or_else(|| {
            providers.iter().find(|v| {
                v["enabled"] == true
                    && v["configured"] == true
                    && !string(v, "defaultModel").is_empty()
            })
        })
        .or_else(|| providers.first());
    Ok(
        json!({ "providers": providers, "provider": selected.map(|v| string(v, "id")).unwrap_or(""),
        "model": selected.map(|v| string(v, "defaultModel")).unwrap_or(""), "defaultProvider": default_provider, "defaultModel": default_model,
        "thinkingLevel": docs.settings.get("defaultThinkingLevel").and_then(Value::as_str).unwrap_or("medium"),
        "toolMode": docs.app.get("toolMode").and_then(Value::as_str).unwrap_or("full"),
        "configuredToolMode": docs.app.get("toolMode").and_then(Value::as_str).unwrap_or("full"),
        "toolModeMessage": "" }),
    )
}

pub(crate) async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let docs = state.providers.read()?;
    Ok(Json(snapshot(&state, &docs).await?))
}

fn check_provider(docs: &Documents, id: &str) -> Result<(), ApiError> {
    if !known(id) && overlay(docs, id).is_null() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "provider_not_found",
            "Provider 不存在。",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 60
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(ApiError::bad_request("Provider ID 无效。"));
    }
    Ok(())
}

fn validate_connection(body: &Value, connection: &mut Value) -> Result<(), ApiError> {
    for field in ["name", "api", "baseUrl"] {
        if let Some(value) = body.get(field) {
            let value = value
                .as_str()
                .ok_or_else(|| ApiError::bad_request(format!("{field} 必须是文本。")))?
                .trim();
            if value.is_empty() {
                return Err(ApiError::bad_request(format!("{field} 不能为空。")));
            }
            connection[field] = json!(value);
        }
    }
    let api = string(connection, "api");
    if !api.is_empty() && !PROTOCOLS.contains(&api) {
        return Err(ApiError::bad_request("当前 API 协议不支持此连接。"));
    }
    let base = string(connection, "baseUrl");
    if !base.is_empty() {
        let mut url = discovery::validated_url(base)?;
        if api == "anthropic-messages" {
            let path = url
                .path()
                .trim_end_matches('/')
                .strip_suffix("/v1")
                .unwrap_or(url.path().trim_end_matches('/'))
                .to_string();
            url.set_path(&path);
        }
        connection["baseUrl"] = json!(url.as_str().trim_end_matches('/'));
    }
    if let Some(value) = body.get("organization") {
        let organization = value
            .as_str()
            .ok_or_else(|| ApiError::bad_request("organization 必须是文本。"))?
            .trim();
        if !connection.get("headers").is_some_and(Value::is_object) {
            connection["headers"] = json!({});
        }
        if organization.is_empty() {
            connection["headers"]
                .as_object_mut()
                .unwrap()
                .remove("OpenAI-Organization");
        } else {
            connection["headers"]["OpenAI-Organization"] = json!(organization);
        }
    }
    Ok(())
}

fn apply_api_key(docs: &mut Documents, id: &str, body: &Value) -> Result<bool, ApiError> {
    let Some(value) = body.get("apiKey") else {
        return Ok(false);
    };
    let key = value
        .as_str()
        .ok_or_else(|| ApiError::bad_request("API Key 必须是文本。"))?
        .trim();
    // 空输入表示保留密钥，和 React 凭据编辑器的留空行为一致。
    if key.is_empty() {
        return Ok(false);
    }
    if key.len() > 16_384 {
        return Err(ApiError::bad_request("API Key 过长。"));
    }
    let key = key.strip_prefix("Bearer ").unwrap_or(key).trim();
    let mut credential = docs
        .auth
        .get(id)
        .cloned()
        .filter(|v| string(v, "type") == "api_key")
        .unwrap_or_else(|| json!({}));
    credential["type"] = json!("api_key");
    credential["key"] = json!(key);
    docs.auth[id] = credential;
    Ok(true)
}

fn model_draft(body: &Value, previous: Option<Value>) -> Result<Value, ApiError> {
    let id = string(body, "id").trim();
    let id = if id.is_empty() {
        string(body, "modelId").trim()
    } else {
        id
    };
    if id.is_empty() || id.chars().count() > 240 || id.chars().any(char::is_control) {
        return Err(ApiError::bad_request("模型 ID 无效。"));
    }
    let mut model = previous.unwrap_or_else(|| json!({ "id": id, "name": id, "reasoning": false, "input": ["text"], "contextWindow": 128000, "maxTokens": 8192, "kind": "chat", "userConfigured": true }));
    for field in [
        "name",
        "api",
        "baseUrl",
        "reasoning",
        "input",
        "capabilities",
    ] {
        if let Some(value) = body.get(field) {
            model[field] = value.clone();
        }
    }
    for field in ["contextWindow", "maxTokens"] {
        if let Some(value) = body.get(field) {
            let number = value
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| ApiError::bad_request(format!("{field} 必须是正整数。")))?;
            model[field] = json!(number);
        }
    }
    if model["maxTokens"].as_u64().unwrap_or(0) > model["contextWindow"].as_u64().unwrap_or(0) {
        return Err(ApiError::bad_request("输出上限不能超过上下文窗口。"));
    }
    let model_kind = body
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("capabilities")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str)
        })
        .unwrap_or(kind(&model))
        .to_string();
    if !["chat", "image", "video"].contains(&model_kind.as_str()) {
        return Err(ApiError::bad_request("模型能力无效。"));
    }
    model["kind"] = json!(model_kind);
    if !model.get("capabilities").is_some_and(Value::is_array) {
        model["capabilities"] = json!([model_kind]);
    }
    let inputs = model
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("模型输入类型无效。"))?;
    if inputs.is_empty()
        || inputs
            .iter()
            .any(|v| !matches!(v.as_str(), Some("text" | "image")))
    {
        return Err(ApiError::bad_request("模型输入类型无效。"));
    }
    if !model["reasoning"].is_boolean() || !model["name"].is_string() {
        return Err(ApiError::bad_request("模型参数类型无效。"));
    }
    if let Some(levels) = body.get("thinkingLevels") {
        let levels = levels
            .as_array()
            .ok_or_else(|| ApiError::bad_request("思考等级无效。"))?;
        if levels
            .iter()
            .any(|v| !v.as_str().is_some_and(|s| LEVELS.contains(&s)))
        {
            return Err(ApiError::bad_request("思考等级无效。"));
        }
        let mut mapping = BTreeMap::<String, Option<String>>::new();
        for level in LEVELS {
            mapping.insert(
                level.to_string(),
                if *level == "off" || levels.iter().any(|v| v.as_str() == Some(*level)) {
                    Some(level.to_string())
                } else {
                    None
                },
            );
        }
        model["thinkingLevelMap"] = json!(mapping);
    }
    Ok(model)
}

fn connection_mut<'a>(docs: &'a mut Documents, id: &str) -> &'a mut Value {
    if !docs.models.get("providers").is_some_and(Value::is_object) {
        docs.models["providers"] = json!({});
    }
    if !docs.models["providers"]
        .get(id)
        .is_some_and(Value::is_object)
    {
        docs.models["providers"][id] = json!({ "headers": {} });
    }
    &mut docs.models["providers"][id]
}

fn set_enabled(docs: &mut Documents, id: &str, enabled: bool) {
    let mut disabled = array(&docs.app, "disabledProviders");
    disabled.retain(|v| v.as_str() != Some(id));
    if !enabled {
        disabled.push(json!(id));
    }
    docs.app["disabledProviders"] = json!(disabled);
}

fn set_provider_model(docs: &mut Documents, id: &str, model: &str) {
    if !docs
        .app
        .get("providerDefaultModels")
        .is_some_and(Value::is_object)
    {
        docs.app["providerDefaultModels"] = json!({});
    }
    docs.app["providerDefaultModels"][id] = json!(model);
}

async fn finish(
    state: &AppState,
    old: &Documents,
    docs: &mut Documents,
    key_provider: Option<&str>,
) -> Result<Value, ApiError> {
    state.providers.persist(old, docs)?;
    if old.app.get("toolMode") != docs.app.get("toolMode")
        || old.app.get("enabledTools") != docs.app.get("enabledTools")
    {
        state.sessions.refresh_tools();
        state
            .plugin_integration
            .apply_loadout(state.runtime.session())
            .map_err(|error| ApiError::internal(error.message))?;
    }
    reload_engine(&state.runtime, docs).await?;
    if let Some(id) = key_provider {
        if let Some(key) = docs
            .auth
            .get(id)
            .and_then(|v| v.get("key"))
            .and_then(Value::as_str)
        {
            state
                .runtime
                .session()
                .model_runtime()
                .set_runtime_api_key(id, key, None)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
    }
    let mut view = snapshot(state, docs).await?;
    // 首次可用连接自动成为新会话默认；已存在的有效默认保持原选择。
    let default_exists = view["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|provider| {
            string(provider, "id") == string(&docs.settings, "defaultProvider")
                && provider["configured"] == true
                && provider["enabled"] == true
                && provider["models"].as_array().unwrap().iter().any(|model| {
                    kind(model) == "chat"
                        && string(model, "id") == string(&docs.settings, "defaultModel")
                })
        });
    if !default_exists {
        if let Some(provider) = view["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| {
                provider["configured"] == true
                    && provider["enabled"] == true
                    && !string(provider, "defaultModel").is_empty()
            })
        {
            let id = string(provider, "id").to_string();
            let model = string(provider, "defaultModel").to_string();
            docs.settings["defaultProvider"] = json!(id);
            docs.settings["defaultModel"] = json!(model);
            write_object(
                &state.providers.agent_dir.join("settings.json"),
                &docs.settings,
            )?;
            state.runtime.session().settings_manager.reload();
            view = snapshot(state, docs).await?;
        } else if !string(&docs.settings, "defaultProvider").is_empty()
            || !string(&docs.settings, "defaultModel").is_empty()
        {
            docs.settings["defaultProvider"] = json!("");
            docs.settings["defaultModel"] = json!("");
            write_object(
                &state.providers.agent_dir.join("settings.json"),
                &docs.settings,
            )?;
            state.runtime.session().settings_manager.reload();
            view = snapshot(state, docs).await?;
        }
    }
    let provider = string(&docs.settings, "defaultProvider");
    let model_id = string(&docs.settings, "defaultModel");
    let previous = state.runtime.session().model();
    let default_changed = old.settings.get("defaultProvider")
        != docs.settings.get("defaultProvider")
        || old.settings.get("defaultModel") != docs.settings.get("defaultModel");
    let (provider, model_id) = match previous.as_ref() {
        Some(previous) if !default_changed => (previous.provider.as_str(), previous.id.as_str()),
        _ => (provider, model_id),
    };
    if model_allowed(docs, provider, model_id)
        && state
            .runtime
            .session()
            .model_runtime()
            .has_configured_auth(provider)
    {
        if let Some(model) = state
            .runtime
            .session()
            .model_runtime()
            .get_model(provider, model_id)
            .await
        {
            if previous.as_ref() != Some(&model) {
                state
                    .runtime
                    .session()
                    .set_model(model, None)
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))?;
            }
        }
    }
    for hosted in state.sessions.all() {
        let session = hosted.session();
        let previous = session.model();
        reload_engine(&hosted.runtime, docs).await?;
        if let Some(previous) = previous {
            if let Some(model) = session
                .model_runtime()
                .get_model(&previous.provider, &previous.id)
                .await
            {
                if model_allowed(docs, &model.provider, &model.id) && model != previous {
                    session
                        .set_model(model, None)
                        .await
                        .map_err(|error| ApiError::internal(error.to_string()))?;
                }
            }
        }
    }
    state
        .memory_tasks
        .set_semantic_model(state.runtime.session().model().map(|model| {
            Arc::new(crate::memory_store::runtime::PiMemoryModel::new(
                state.runtime.session().model_runtime().clone(),
                model,
            )) as Arc<dyn crate::memory_store::runtime::MemoryModel>
        }));
    Ok(view)
}

fn ensure_idle(state: &AppState) -> Result<(), ApiError> {
    if state.sessions.any_busy() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "session_busy",
            "请等待当前回答结束后修改模型配置。",
        ));
    }
    Ok(())
}

fn engine_busy() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "session_busy",
        "请等待当前回答结束后修改模型配置。",
    )
}

pub(crate) async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    ensure_idle(&state)?;
    let old = state.providers.read()?;
    let mut docs = old.clone();
    let id = string(&body, "provider").trim();
    if !id.is_empty() {
        check_provider(&docs, id)?;
    } else if ["model", "api", "baseUrl", "apiKey", "enabled"]
        .iter()
        .any(|field| body.get(*field).is_some())
    {
        return Err(ApiError::bad_request("缺少 Provider。"));
    }
    let mut key_updated = false;
    if !id.is_empty() {
        validate_connection(&body, connection_mut(&mut docs, id))?;
        key_updated = apply_api_key(&mut docs, id, &body)?;
        if let Some(value) = body.get("enabled") {
            set_enabled(
                &mut docs,
                id,
                value
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request("enabled 必须是布尔值。"))?,
            );
        }
        if let Some(provider_type) = body.get("providerType").and_then(Value::as_str) {
            if !["chat", "visual"].contains(&provider_type) {
                return Err(ApiError::bad_request("Provider 类型无效。"));
            }
            if !docs.app.get("providerTypes").is_some_and(Value::is_object) {
                docs.app["providerTypes"] = json!({});
            }
            docs.app["providerTypes"][id] = json!(provider_type);
        }
        let model_id = string(&body, "model").trim();
        if !model_id.is_empty() {
            let available = state
                .runtime
                .session()
                .model_runtime()
                .get_model(id, model_id)
                .await;
            if definition(&docs, id, model_id).is_none() && available.is_none() {
                let draft = json!({ "id": model_id, "kind": body.get("modelKind").and_then(Value::as_str).unwrap_or("chat") });
                let model = model_draft(&draft, None)?;
                let connection = connection_mut(&mut docs, id);
                if !connection.get("models").is_some_and(Value::is_array) {
                    connection["models"] = json!([]);
                }
                connection["models"].as_array_mut().unwrap().push(model);
            }
            if definition(&docs, id, model_id).map(kind).unwrap_or("chat") == "chat" {
                set_provider_model(&mut docs, id, model_id);
            }
        }
        if body.get("setAsDefault").and_then(Value::as_bool) == Some(true) {
            let model = if model_id.is_empty() {
                let view = snapshot(&state, &docs).await?;
                view["providers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| string(v, "id") == id)
                    .map(|v| string(v, "defaultModel").to_string())
                    .unwrap_or_default()
            } else {
                model_id.to_string()
            };
            if model.is_empty() || !model_allowed(&docs, id, &model) {
                return Err(ApiError::bad_request("请选择启用的对话模型。"));
            }
            if !key_updated
                && docs.auth.get(id).is_none()
                && !state
                    .runtime
                    .session()
                    .model_runtime()
                    .has_configured_auth(id)
            {
                return Err(ApiError::bad_request("请先配置此连接的 API Key。"));
            }
            docs.settings["defaultProvider"] = json!(id);
            docs.settings["defaultModel"] = json!(model);
        }
    }
    if let Some(level) = body.get("thinkingLevel").and_then(Value::as_str) {
        if !LEVELS.contains(&level) {
            return Err(ApiError::bad_request("思考等级无效。"));
        }
        docs.settings["defaultThinkingLevel"] = json!(level);
    }
    if let Some(mode) = body.get("toolMode").and_then(Value::as_str) {
        if !["full", "custom"].contains(&mode) {
            return Err(ApiError::bad_request("工具模式无效。"));
        }
        docs.app["toolMode"] = json!(mode);
    }
    let mut view = finish(&state, &old, &mut docs, key_updated.then_some(id)).await?;
    view["apiKeyUpdated"] = json!(key_updated);
    view["defaultUpdated"] = json!(old.settings != docs.settings);
    Ok(Json(view))
}

async fn create_provider(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    create_provider_record(&state, &body, None)
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}

async fn create_provider_record(
    state: &AppState,
    body: &Value,
    clone: Option<&str>,
) -> Result<Value, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    ensure_idle(state)?;
    let old = state.providers.read()?;
    let mut docs = old.clone();
    let id = string(body, "id").trim();
    validate_id(id)?;
    if known(id)
        || !overlay(&docs, id).is_null()
        || state
            .runtime
            .session()
            .model_runtime()
            .get_provider(id)
            .await
            .is_some()
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "provider_exists",
            "Provider ID 已存在。",
        ));
    }
    let mut connection = if let Some(source) = clone {
        check_provider(&docs, source)?;
        overlay(&docs, source).clone()
    } else {
        json!({})
    };
    if connection.is_null() {
        connection = json!({});
    }
    validate_connection(body, &mut connection)?;
    if string(&connection, "name").is_empty() || string(&connection, "baseUrl").is_empty() {
        return Err(ApiError::bad_request("名称和 Base URL 不能为空。"));
    }
    if string(&connection, "api").is_empty() {
        connection["api"] = json!("openai-responses");
    }
    let model_id = string(body, "model").trim();
    if clone.is_none() {
        let model = model_draft(
            &json!({ "id": model_id, "kind": body.get("modelKind").and_then(Value::as_str).unwrap_or("chat") }),
            None,
        )?;
        connection["models"] = json!([model]);
    }
    connection.as_object_mut().unwrap().remove("apiKey");
    *connection_mut(&mut docs, id) = connection;
    let key_updated = apply_api_key(&mut docs, id, body)?;
    if let Some(source) = clone {
        if !key_updated {
            if let Some(credential) = docs
                .auth
                .get(source)
                .cloned()
                .filter(|v| string(v, "type") != "oauth")
            {
                docs.auth[id] = credential;
            }
        }
    }
    set_enabled(
        &mut docs,
        id,
        body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    );
    let provider_type = body
        .get("providerType")
        .and_then(Value::as_str)
        .unwrap_or("chat");
    if !["chat", "visual"].contains(&provider_type) {
        return Err(ApiError::bad_request("Provider 类型无效。"));
    }
    if !docs.app.get("providerTypes").is_some_and(Value::is_object) {
        docs.app["providerTypes"] = json!({});
    }
    docs.app["providerTypes"][id] = json!(provider_type);
    if provider_type == "chat" && !model_id.is_empty() {
        set_provider_model(&mut docs, id, model_id);
    }
    let mut view = finish(state, &old, &mut docs, key_updated.then_some(id)).await?;
    view["createdProviderId"] = json!(id);
    Ok(view)
}

async fn clone_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    create_provider_record(&state, &body, Some(&id))
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}

async fn update_connection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    body["provider"] = json!(id);
    body["setAsDefault"] = json!(false);
    put_config(State(state), Json(body)).await
}

async fn update_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    mutate_models(&state, &id, &body, false, false)
        .await
        .map(Json)
}
async fn add_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    mutate_models(&state, &id, &body, true, false)
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}
async fn add_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    mutate_models(&state, &id, &body, true, true)
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}
async fn mutate_models(
    state: &AppState,
    id: &str,
    body: &Value,
    create: bool,
    batch: bool,
) -> Result<Value, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    ensure_idle(state)?;
    let old = state.providers.read()?;
    check_provider(&old, id)?;
    let mut docs = old.clone();
    let drafts = if batch {
        body.get("models")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ApiError::bad_request("缺少模型列表。"))?
    } else {
        vec![body.clone()]
    };
    if drafts.is_empty() || drafts.len() > 250 {
        return Err(ApiError::bad_request("请添加 1 至 250 个模型。"));
    }
    let engine_models = state
        .runtime
        .session()
        .model_runtime()
        .get_models(Some(id))
        .await;
    let mut added = Vec::new();
    for draft in drafts {
        let model_id = if string(&draft, "id").is_empty() {
            string(&draft, "modelId")
        } else {
            string(&draft, "id")
        };
        let previous = definition(&docs, id, model_id).cloned().or_else(|| {
            engine_models
                .iter()
                .find(|model| model.id == model_id)
                .map(|model| serde_json::to_value(model).unwrap())
        });
        if create && previous.is_some() {
            if batch {
                continue;
            }
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "model_exists",
                "模型已经存在。",
            ));
        }
        if !create && previous.is_none() {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                "模型不存在。",
            ));
        }
        let model = model_draft(&draft, previous)?;
        let connection = connection_mut(&mut docs, id);
        let mut models = array(connection, "models");
        models.retain(|item| string(item, "id") != model_id);
        models.push(model);
        connection["models"] = json!(models);
        let mut excluded = array(connection, "excludedModels");
        excluded.retain(|value| value.as_str() != Some(model_id));
        connection["excludedModels"] = json!(excluded);
        added.push(model_id.to_string());
    }
    let mut view = finish(state, &old, &mut docs, None).await?;
    view["addedModelIds"] = json!(added);
    Ok(view)
}

async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    ensure_idle(&state)?;
    let old = state.providers.read()?;
    check_provider(&old, &id)?;
    let mut docs = old.clone();
    let model_id = string(&body, "modelId").trim();
    if model_id.is_empty() {
        return Err(ApiError::bad_request("缺少模型 ID。"));
    }
    if definition(&docs, &id, model_id).is_none()
        && state
            .runtime
            .session()
            .model_runtime()
            .get_model(&id, model_id)
            .await
            .is_none()
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "模型不存在。",
        ));
    }
    let connection = connection_mut(&mut docs, &id);
    let mut models = array(connection, "models");
    models.retain(|model| string(model, "id") != model_id);
    connection["models"] = json!(models);
    let mut excluded = array(connection, "excludedModels");
    if !excluded.iter().any(|v| v.as_str() == Some(model_id)) {
        excluded.push(json!(model_id));
    }
    connection["excludedModels"] = json!(excluded);
    if let Some(defaults) = docs
        .app
        .get_mut("providerDefaultModels")
        .and_then(Value::as_object_mut)
    {
        if defaults.get(&id).and_then(Value::as_str) == Some(model_id) {
            defaults.remove(&id);
        }
    }
    Ok(Json(finish(&state, &old, &mut docs, None).await?))
}

async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    ensure_idle(&state)?;
    let old = state.providers.read()?;
    check_provider(&old, &id)?;
    if known(&id) {
        return Err(ApiError::bad_request(
            "内置 Provider 可以停用，但不能删除。",
        ));
    }
    let mut docs = old.clone();
    docs.models["providers"]
        .as_object_mut()
        .unwrap()
        .remove(&id);
    docs.auth.as_object_mut().unwrap().remove(&id);
    for field in ["providerTypes", "providerDefaultModels"] {
        if let Some(map) = docs.app.get_mut(field).and_then(Value::as_object_mut) {
            map.remove(&id);
        }
    }
    set_enabled(&mut docs, &id, true);
    state
        .runtime
        .session()
        .model_runtime()
        .remove_runtime_api_key(&id, None)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(finish(&state, &old, &mut docs, None).await?))
}

/// release exportProviderConfig：桌面/移动配置导出视图。
/// providerKeyrings 是移动端钥匙串（诚实接缝）：本后端无此存储，导出为空对象。
async fn export_provider_config(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let docs = state.providers.read()?;
    Ok(Json(json!({
        "version": 1,
        "provider": string(&docs.settings, "defaultProvider"),
        "model": string(&docs.settings, "defaultModel"),
        "thinkingLevel": docs.settings.get("defaultThinkingLevel").and_then(Value::as_str).unwrap_or("medium"),
        "toolMode": docs.app.get("toolMode").and_then(Value::as_str).unwrap_or("full"),
        "disabledProviders": docs.app.get("disabledProviders").cloned().unwrap_or(json!([])),
        "providers": docs.models.get("providers").cloned().unwrap_or(json!({})),
        "credentials": docs.auth.clone(),
        "providerKeyrings": {},
    })))
}

/// release importProviderConfig：合并桌面端配置到本机（同名 Provider 覆盖）。
async fn import_provider_config(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    if input["version"] != 1 || !input["providers"].is_object() {
        return Err(ApiError::bad_request("桌面端模型配置格式无效。"));
    }
    let old = state.providers.read()?;
    let mut docs = old.clone();
    {
        let providers = docs
            .models
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("models.json 无效"))?
            .entry("providers")
            .or_insert_with(|| json!({}));
        if let (Some(target), Some(source)) = (providers.as_object_mut(), input["providers"].as_object()) {
            for (id, value) in source {
                target.insert(id.clone(), value.clone());
            }
        }
    }
    if input["credentials"].is_object() {
        if let (Some(target), Some(source)) = (docs.auth.as_object_mut(), input["credentials"].as_object()) {
            for (id, value) in source {
                target.insert(id.clone(), value.clone());
            }
        }
    }
    if input.get("toolMode").and_then(Value::as_str).is_some() {
        docs.app["toolMode"] = input["toolMode"].clone();
    }
    if input.get("disabledProviders").map(Value::is_array).unwrap_or(false) {
        docs.app["disabledProviders"] = input["disabledProviders"].clone();
    }
    let provider = string(&input, "provider").trim().to_string();
    let model = string(&input, "model").trim().to_string();
    if !provider.is_empty() && !model.is_empty() {
        docs.settings["defaultProvider"] = json!(provider);
        docs.settings["defaultModel"] = json!(model);
    }
    let thinking = string(&input, "thinkingLevel").trim().to_string();
    if !thinking.is_empty() {
        docs.settings["defaultThinkingLevel"] = json!(thinking);
    }
    state.providers.persist(&old, &docs)?;
    reload_engine(&state.runtime, &docs).await?;
    let imported: Vec<String> = input["providers"]
        .as_object()
        .map(|providers| providers.keys().cloned().collect())
        .unwrap_or_default();
    Ok(Json(json!({
        "imported": imported,
        "config": snapshot(&state, &docs).await?,
    })))
}

async fn unsupported_import() -> Result<Json<Value>, ApiError> {
    Err(ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "external_provider_import_not_supported",
        "Rust 后端暂不支持读取其他应用登录凭据；已有 Pisper 连接会直接使用。请通过连接向导添加。",
    ))
}

async fn import_local(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let docs = state.providers.read()?;
    let skipped: Vec<Value> = docs
        .models
        .get("providers")
        .and_then(Value::as_object)
        .map(|providers| {
            providers
                .keys()
                .map(|id| json!({"id":id,"source":"pisper","reason":"already_available"}))
                .collect()
        })
        .unwrap_or_default();
    // 应用原目录已经直接供 Pi 使用，不复制密钥、不重复导入已有连接。
    // 不支持的外部来源在报告中明确列出，自动设置页加载不会变成请求异常。
    Ok(Json(json!({"config":snapshot(&state,&docs).await?,
        "discovery":local_discovery_report(&state,&docs).await?,
        "imported":[],"skipped":skipped,"externalImportSupported":false})))
}

async fn local_discovery_report(state: &AppState, docs: &Documents) -> Result<Value, ApiError> {
    let config = snapshot(state, docs).await?;
    let loaded: Vec<Value> = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|provider| {
            !overlay(docs, string(provider, "id")).is_null() || provider["configured"] == true
        })
        .map(|provider| {
            json!({"id":provider["id"],"providerName":provider["name"],"source":"pisper-agent",
            "api":provider["api"],"baseUrl":provider["baseUrl"],"models":provider["models"],
            "configured":provider["configured"],"imported":true,"importable":false})
        })
        .collect();
    // providers 专指外部可导入项，已加载的原应用连接单独报告，避免界面误称为 Claude/Codex 来源。
    Ok(
        json!({"providers":[],"errors":[{"source":"external-cli","code":"external_provider_import_not_supported"}],
        "loadedProviders":loaded,"source":"pisper-agent","externalImportSupported":false,
        "message":"已读取 Pisper 原模型配置；Rust 后端暂不支持外部 Codex/Claude 登录导入。"}),
    )
}

async fn get_discovery(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let docs = state.providers.read()?;
    Ok(Json(local_discovery_report(&state, &docs).await?))
}

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/config",
            get(get_config).put(put_config).post(put_config),
        )
        .route("/api/providers", post(create_provider))
        .route("/api/providers/{id}", delete(delete_provider))
        .route("/api/providers/{id}/clone", post(clone_provider))
        .route("/api/providers/{id}/connection", put(update_connection))
        .route("/api/providers/{id}/api-key", put(update_connection))
        .route("/api/providers/{id}/enabled", put(update_connection))
        .route(
            "/api/providers/{id}/models",
            post(add_model).delete(delete_model),
        )
        .route("/api/providers/{id}/models/options", put(update_models))
        .route("/api/providers/{id}/models/batch", post(add_models))
        .route(
            "/api/providers/{id}/models/discover",
            post(discovery::discover_provider),
        )
        .route(
            "/api/providers/models/discover-connection",
            post(discovery::discover_connection),
        )
        .route("/api/providers/models/refresh", post(discovery::refresh))
        .route("/api/providers/export", get(export_provider_config))
        .route("/api/providers/import", post(import_provider_config))
        .route("/api/providers/import-local", post(import_local))
        .route("/api/providers/{id}/import", post(unsupported_import))
        .route("/api/providers/discovery", get(get_discovery))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pisper-provider-test-{}-{}",
            std::process::id(),
            crate::product::new_id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn configuration_mutation_preserves_unknown_fields_and_does_not_expose_credentials() {
        let dir = temporary_dir();
        write_object(&dir.join("models.json"), &json!({"vendorData": 7, "providers": {"local": {"name":"old", "api":"openai-responses", "baseUrl":"http://localhost:1234/v1", "customSetting": 42, "headers":{"X-Existing":"value"}}}})).unwrap();
        write_object(&dir.join("auth.json"), &json!({"other": {"type":"api_key", "key":"fictional-other-test-key", "extraAuth":true}})).unwrap();
        let store = ProviderConfigStore::new(dir.to_str().unwrap());
        let old = store.read().unwrap();
        let mut docs = old.clone();
        let body =
            json!({"name":"new", "organization":"test-org", "apiKey":"fictional-current-test-key"});
        validate_connection(&body, connection_mut(&mut docs, "local")).unwrap();
        apply_api_key(&mut docs, "local", &body).unwrap();
        store.persist(&old, &docs).unwrap();
        let restored = store.read().unwrap();
        assert_eq!(restored.models["vendorData"], 7);
        assert_eq!(restored.models["providers"]["local"]["customSetting"], 42);
        assert_eq!(
            restored.models["providers"]["local"]["headers"]["X-Existing"],
            "value"
        );
        assert_eq!(restored.auth["other"]["extraAuth"], true);
        assert!(restored
            .models
            .to_string()
            .find("fictional-current-test-key")
            .is_none());
        assert!(!apply_api_key(&mut docs, "local", &json!({"apiKey":""})).unwrap());
        assert_eq!(docs.auth["local"]["key"], "fictional-current-test-key");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_existing_configuration_is_never_replaced() {
        let dir = temporary_dir();
        std::fs::write(dir.join("models.json"), "broken {").unwrap();
        let store = ProviderConfigStore::new(dir.to_str().unwrap());
        assert!(store.read().is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("models.json")).unwrap(),
            "broken {"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn browser_notification_update_preserves_config_and_shares_preference_writer() {
        let dir = temporary_dir();
        write_object(
            &dir.join("pisper.json"),
            &json!({"vendor":{"keep":1},"notifications":{"futureChannel":{"enabled":true},"browser":{"enabled":false,"futureSetting":"keep"}}}),
        )
        .unwrap();
        let mut untouched = Vec::new();
        for name in ["models.json", "auth.json", "settings.json"] {
            write_object(&dir.join(name), &json!({"untouched": name})).unwrap();
            untouched.push((name, std::fs::read(dir.join(name)).unwrap()));
        }
        let store = Arc::new(ProviderConfigStore::new(dir.to_str().unwrap()));
        let held = store.write.lock().await;
        let notification = tokio::spawn({
            let store = store.clone();
            async move { store.update_browser_notifications_enabled(true).await }
        });
        let preference = tokio::spawn({
            let store = store.clone();
            async move {
                store
                    .update_app_preferences(&json!({"thinkingLevel":"high"}))
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!notification.is_finished());
        assert!(!preference.is_finished());
        drop(held);
        notification.await.unwrap().unwrap();
        preference.await.unwrap().unwrap();
        let restored = store.app_preferences().unwrap();
        assert_eq!(restored["notifications"]["browser"]["enabled"], true);
        assert_eq!(
            restored["notifications"]["browser"]["futureSetting"],
            "keep"
        );
        assert_eq!(restored["notifications"]["futureChannel"]["enabled"], true);
        assert_eq!(restored["vendor"]["keep"], 1);
        assert_eq!(restored["thinkingLevel"], "high");
        for (name, bytes) in untouched {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn browser_notification_update_never_replaces_an_unreadable_app_config() {
        let dir = temporary_dir();
        let original = b"{invalid existing app config";
        std::fs::write(dir.join("pisper.json"), original).unwrap();
        let store = ProviderConfigStore::new(dir.to_str().unwrap());
        assert!(store
            .update_browser_notifications_enabled(true)
            .await
            .is_err());
        assert_eq!(std::fs::read(dir.join("pisper.json")).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn domain_mutations_serialize_read_modify_write_and_roll_back_errors() {
        let dir = temporary_dir();
        write_object(
            &dir.join("pisper.json"),
            &json!({"counter":0,"future":{"keep":true}}),
        )
        .unwrap();
        let mut other = Vec::new();
        for name in ["models.json", "auth.json", "settings.json"] {
            write_object(&dir.join(name), &json!({"untouched":name})).unwrap();
            other.push((name, std::fs::read(dir.join(name)).unwrap()));
        }
        let store = Arc::new(ProviderConfigStore::new(dir.to_str().unwrap()));
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .mutate_app_preferences(|app| {
                        app["counter"] = json!(app["counter"].as_u64().unwrap() + 1);
                        Ok(())
                    })
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        let before = std::fs::read(dir.join("pisper.json")).unwrap();
        assert!(store
            .mutate_app_preferences(|app| {
                app["future"] = Value::Null;
                Err(ApiError::bad_request("fixture rejection"))
            })
            .await
            .is_err());
        assert_eq!(std::fs::read(dir.join("pisper.json")).unwrap(), before);
        assert_eq!(
            store.app_preferences().unwrap(),
            json!({"counter":16,"future":{"keep":true}})
        );
        for (name, bytes) in other {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn model_options_preserve_extensions_and_save_thinking_levels() {
        let previous = json!({"id":"chat-test", "name":"old", "kind":"chat", "reasoning":true, "input":["text"], "contextWindow":4096, "maxTokens":1024, "vendorOption":42});
        let model = model_draft(
            &json!({"modelId":"chat-test", "name":"new", "thinkingLevels":["low","high"]}),
            Some(previous),
        )
        .unwrap();
        assert_eq!(model["vendorOption"], 42);
        assert_eq!(model["thinkingLevelMap"]["low"], "low");
        assert_eq!(model["thinkingLevelMap"]["medium"], Value::Null);
        assert!(model_draft(
            &json!({"id":"test", "maxTokens":9000, "contextWindow":10}),
            None
        )
        .is_err());
    }

    #[tokio::test]
    async fn visual_reads_and_preference_writes_do_not_parse_unrelated_settings() {
        let dir = temporary_dir();
        write_object(&dir.join("models.json"), &json!({"providers":{}})).unwrap();
        write_object(&dir.join("auth.json"), &json!({"fixture":"synthetic-only"})).unwrap();
        write_object(
            &dir.join("pisper.json"),
            &json!({"visualDefaultModels":{"video":"keep/video"},"future":{"keep":true}}),
        )
        .unwrap();
        let invalid_settings = b"{invalid unrelated settings";
        std::fs::write(dir.join("settings.json"), invalid_settings).unwrap();
        let store = ProviderConfigStore::new(dir.to_str().unwrap());
        let (models, auth, app) = store.visual_documents().await.unwrap();
        assert_eq!(models["providers"], json!({}));
        assert_eq!(auth["fixture"], "synthetic-only");
        assert_eq!(app["future"]["keep"], true);
        let saved = store
            .mutate_visual_preferences(|mut app| {
                app["visualDefaultModels"]["image"] = json!("fixture/image");
                Ok(app)
            })
            .await
            .unwrap();
        assert_eq!(saved["visualDefaultModels"]["video"], "keep/video");
        assert_eq!(saved["future"]["keep"], true);
        assert_eq!(
            std::fs::read(dir.join("settings.json")).unwrap(),
            invalid_settings
        );
        // 清空偏好的持久化本身只依赖 app，随后状态读取可以独立报告模型文档错误。
        std::fs::write(dir.join("models.json"), b"{invalid models").unwrap();
        std::fs::write(dir.join("auth.json"), b"{invalid auth").unwrap();
        store
            .mutate_visual_preferences(|mut app| {
                app["visualDefaultModels"]
                    .as_object_mut()
                    .unwrap()
                    .remove("image");
                Ok(app)
            })
            .await
            .unwrap();
        assert!(store.visual_documents().await.is_err());
        assert_eq!(
            std::fs::read(dir.join("models.json")).unwrap(),
            b"{invalid models"
        );
        assert_eq!(
            std::fs::read(dir.join("auth.json")).unwrap(),
            b"{invalid auth"
        );
        let before = std::fs::read(dir.join("pisper.json")).unwrap();
        assert!(store
            .mutate_visual_preferences(|_| Err(ApiError::bad_request("fixture rejection")))
            .await
            .is_err());
        assert_eq!(std::fs::read(dir.join("pisper.json")).unwrap(), before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn disabled_excluded_and_visual_models_are_not_chat_candidates() {
        let docs = Documents {
            models: json!({"providers":{"local":{"models":[{"id":"image-test","kind":"image"}], "excludedModels":["removed-test"]}}}),
            app: json!({"disabledProviders":["disabled"]}),
            settings: json!({}),
            auth: json!({}),
        };
        assert!(model_allowed(&docs, "local", "chat-test"));
        assert!(!model_allowed(&docs, "local", "image-test"));
        assert!(!model_allowed(&docs, "local", "removed-test"));
        assert!(!model_allowed(&docs, "disabled", "chat-test"));
    }
}
