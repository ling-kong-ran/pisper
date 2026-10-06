//! 通用视觉生成的组合根：规范配置、共享模型目录和实际会话归档分别通过窄端口接入。
use crate::{
    asset_api::store::AssetStore,
    execution_adapter::PiExecutor,
    native_visual::{
        Result, VisualConfigPort, VisualConfigSnapshot, VisualContextPort, VisualError,
        VisualGeneratedFilePort, VisualToolContext,
    },
    provider_config::ProviderConfigStore,
    AppState,
};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub(crate) struct VisualIntegration {
    state: OnceLock<Weak<AppState>>,
    providers: Arc<ProviderConfigStore>,
    executor: Weak<PiExecutor>,
    assets: Arc<Mutex<AssetStore>>,
    agent_dir: String,
    locale: String,
}

impl VisualIntegration {
    pub(crate) fn new(
        providers: Arc<ProviderConfigStore>,
        executor: &Arc<PiExecutor>,
        assets: Arc<Mutex<AssetStore>>,
        agent_dir: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: OnceLock::new(),
            providers,
            executor: Arc::downgrade(executor),
            assets,
            agent_dir,
            locale: process_locale(),
        })
    }

    pub(crate) fn attach(&self, state: &Arc<AppState>) -> Result<()> {
        self.state
            .set(Arc::downgrade(state))
            .map_err(|_| VisualError::new("视觉运行时已连接。"))
    }

    pub(crate) fn config_port(self: &Arc<Self>) -> VisualConfigPort {
        let integration = self.clone();
        let providers = self.providers.clone();
        VisualConfigPort {
            read: Arc::new(move || {
                let integration = integration.clone();
                Box::pin(async move {
                    let state = integration
                        .state
                        .get()
                        .and_then(Weak::upgrade)
                        .ok_or_else(|| VisualError::new("视觉运行时尚未连接或已关闭。"))?;
                    let _configuration = state.engine_mutation.read().await;
                    let (models_json, auth_json, app_json) = integration
                        .providers
                        .visual_documents()
                        .await
                        .map_err(|error| VisualError::new(error.message))?;
                    let runtime = state.runtime.session().model_runtime().clone();
                    let mut runtime_provider_names = Map::new();
                    let mut runtime_models = Vec::new();
                    for provider in runtime.get_providers().await {
                        runtime_provider_names.insert(
                            provider.id().to_owned(),
                            Value::String(provider.name().to_owned()),
                        );
                        for model in runtime.get_models(Some(provider.id())).await {
                            runtime_models.push(
                                serde_json::to_value(model)
                                    .map_err(|error| VisualError::new(error.to_string()))?,
                            );
                        }
                    }
                    let registered_ids = runtime_provider_names.keys().cloned().collect::<Vec<_>>();
                    let version = serde_json::from_str::<Value>(include_str!("../../package.json"))
                        .ok()
                        .and_then(|package| package["version"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    let user_agent = if version.trim().is_empty() {
                        "Pisper".to_owned()
                    } else {
                        format!("Pisper/{}", version.trim())
                    };
                    let runtime_models = crate::visual_catalog_cache::augment_runtime_models(
                        std::path::Path::new(&state.data_dir),
                        &models_json["providers"],
                        &registered_ids,
                        runtime_models,
                        &user_agent,
                    )
                    .map_err(VisualError::new)?;
                    Ok(VisualConfigSnapshot {
                        models_json,
                        auth_json,
                        app_json,
                        runtime_provider_names,
                        runtime_models,
                        locale: integration.locale.clone(),
                    })
                })
            }),
            write_preference: Arc::new(move |kind, reference| {
                let providers = providers.clone();
                Box::pin(async move {
                    providers
                        .mutate_visual_preferences(move |app| {
                            if app.is_null() {
                                return Err(crate::ApiError::bad_request(
                                "Cannot read properties of null (reading 'visualDefaultModels')",
                            ));
                            }
                            let mut preferred = spread(&app["visualDefaultModels"]);
                            if let Some(reference) = reference {
                                preferred
                                    .insert(kind.as_str().to_owned(), Value::String(reference));
                            } else {
                                preferred.remove(kind.as_str());
                            }
                            let mut app = spread(&app);
                            app.insert("visualDefaultModels".into(), Value::Object(preferred));
                            Ok(Value::Object(app))
                        })
                        .await
                        .map_err(|error| VisualError::new(error.message))?;
                    Ok(())
                })
            }),
        }
    }

    pub(crate) fn context_port(&self) -> VisualContextPort {
        Arc::new(move |session_id, cwd| {
            Box::pin(async move {
                if session_id.is_empty() || !cwd.is_absolute() {
                    return Err(VisualError::new("Visual tool session context unavailable"));
                }
                Ok(VisualToolContext { cwd, session_id })
            })
        })
    }

    pub(crate) fn generated_file_port(self: &Arc<Self>) -> VisualGeneratedFilePort {
        let integration = self.clone();
        Arc::new(move |context, result| {
            let integration = integration.clone();
            Box::pin(async move {
                let owner = integration
                    .executor
                    .upgrade()
                    .ok_or_else(|| VisualError::new("Visual asset owner unavailable"))?
                    .tool_owner(&context.session_id);
                tokio::task::spawn_blocking(move || {
                    let name = crate::session_api::session_infos(&integration.agent_dir)
                        .into_iter()
                        .find(|session| session.id == owner)
                        .and_then(|session| session.name)
                        .unwrap_or_default();
                    integration
                        .assets
                        .lock()
                        .map_err(|error| VisualError::new(error.to_string()))?
                        .archive_generated(&result.path, &owner, &name)
                        .map_err(|error| VisualError::new(error.to_string()))?;
                    Ok(())
                })
                .await
                .map_err(|error| VisualError::new(error.to_string()))?
            })
        })
    }
}

// 对应对象展开；保留视觉偏好的未知键和历史索引属性，不能以空对象覆盖。
fn spread(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(values) => values.clone(),
        Value::Array(values) => values
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        Value::String(value) => value
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                (
                    index.to_string(),
                    Value::String(String::from_utf16_lossy(&[unit])),
                )
            })
            .collect(),
        _ => Map::new(),
    }
}

fn process_locale() -> String {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(locale) = std::env::var(name) {
            let language = locale.split(['.', '@']).next().unwrap_or("");
            if !language.is_empty() && !matches!(language, "C" | "POSIX") {
                return language.replace('_', "-");
            }
        }
    }
    platform_locale()
}

#[cfg(windows)]
fn platform_locale() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, capacity: i32) -> i32;
    }
    let mut name = [0_u16; 85];
    // Win32 写入固定长度 UTF-16 缓冲区，并将终止符算在返回长度中。
    let length = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    if length > 1 && length <= name.len() as i32 {
        return String::from_utf16_lossy(&name[..length as usize - 1]);
    }
    "en-US".into()
}
#[cfg(not(windows))]
fn platform_locale() -> String {
    "en-US".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_spread_preserves_unknown_fields_and_legacy_indices() {
        assert_eq!(
            spread(&serde_json::json!({"future":{"keep":true}}))["future"]["keep"],
            true
        );
        assert_eq!(spread(&serde_json::json!(["keep", 7]))["1"], 7);
        assert_eq!(spread(&serde_json::json!("ab"))["0"], "a");
        assert!(spread(&Value::Null).is_empty());
    }
}
