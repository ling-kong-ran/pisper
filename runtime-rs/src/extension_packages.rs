//! 扩展包直接使用 Pi 原有 settings.json/packages 和 npm/git 安装目录。

use crate::{ApiError, AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use pi_rust::coding_agent::{
    core::settings_manager::{SettingsManager, SettingsManagerCreateOptions, SettingsValue},
    package_manager::{
        DefaultPackageManager, PackageManagerOptions, PackageSourceEntry, SettingsData,
        SettingsManagerHandle, SourceScope,
    },
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct PackageSettings {
    inner: SettingsManager,
    error: Mutex<Option<String>>,
}

fn snapshot(settings: SettingsValue) -> SettingsData {
    let value = serde_json::to_value(settings).expect("serializable settings");
    let strings = |key| {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect()
    };
    SettingsData {
        packages: serde_json::from_value(value.get("packages").cloned().unwrap_or(json!([])))
            .expect("packages validated at the storage boundary"),
        extensions: strings("extensions"),
        skills: strings("skills"),
        prompts: strings("prompts"),
        themes: strings("themes"),
        npm_command: None,
    }
}

impl PackageSettings {
    fn check(&self) -> Result<(), ApiError> {
        if self.error.lock().unwrap().take().is_some() || !self.inner.drain_errors().is_empty() {
            return Err(ApiError::internal("无法读写扩展包设置，原有配置未被替换。"));
        }
        for value in [
            self.inner.get_global_settings(),
            self.inner.get_project_settings(),
        ] {
            let value =
                serde_json::to_value(value).map_err(|e| ApiError::internal(e.to_string()))?;
            serde_json::from_value::<Vec<PackageSourceEntry>>(
                value.get("packages").cloned().unwrap_or(json!([])),
            )
            .map_err(|_| ApiError::internal("settings.json 的 packages 格式无效。"))?;
        }
        Ok(())
    }
}

impl SettingsManagerHandle for PackageSettings {
    fn global_settings(&self) -> SettingsData {
        let mut data = snapshot(self.inner.get_global_settings());
        data.npm_command = self.inner.get_npm_command();
        data
    }
    fn project_settings(&self) -> SettingsData {
        snapshot(self.inner.get_project_settings())
    }
    fn is_project_trusted(&self) -> bool {
        self.inner.is_project_trusted()
    }
    fn set_project_trusted(&self, trusted: bool) {
        self.inner.set_project_trusted(trusted);
    }
    fn npm_command(&self) -> Option<Vec<String>> {
        self.inner.get_npm_command()
    }
    fn set_packages(&self, packages: Vec<PackageSourceEntry>) {
        self.inner
            .set_packages(serde_json::from_value(serde_json::to_value(packages).unwrap()).unwrap());
    }
    fn set_project_packages(&self, packages: Vec<PackageSourceEntry>) {
        if let Err(error) = self.inner.set_project_packages(
            serde_json::from_value(serde_json::to_value(packages).unwrap()).unwrap(),
        ) {
            *self.error.lock().unwrap() = Some(error);
        }
    }
}

fn manager(
    cwd: &str,
    agent_dir: &str,
) -> Result<(DefaultPackageManager, Arc<PackageSettings>), ApiError> {
    let settings = Arc::new(PackageSettings {
        inner: SettingsManager::create_with(
            cwd,
            agent_dir,
            SettingsManagerCreateOptions::default(),
        )
        .map_err(|e| ApiError::internal(e.to_string()))?,
        error: Mutex::new(None),
    });
    settings.check()?;
    Ok((
        DefaultPackageManager::new(PackageManagerOptions {
            cwd: cwd.into(),
            agent_dir: agent_dir.into(),
            settings_manager: settings.clone(),
            command_runner: None,
        }),
        settings,
    ))
}

fn dashboard(manager: &DefaultPackageManager, cwd: &str) -> Value {
    let packages: Vec<_> = manager.list_configured_packages().into_iter().map(|package| {
        let manifest: Value = package.installed_path.as_ref()
            .and_then(|path| std::fs::read(std::path::Path::new(path).join("package.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(json!({}));
        let mut types = Vec::new();
        for (field, kind) in [("extensions", "tool"), ("prompts", "prompt"), ("skills", "skill")] {
            if manifest["pi"][field].as_array().is_some_and(|items| !items.is_empty()) { types.push(kind); }
        }
        json!({"source":package.source,"scope":if package.scope == SourceScope::Project {"project"} else {"user"},
            "filtered":package.filtered,"installed":package.installed_path.is_some(),
            "name":manifest["name"].as_str().unwrap_or(&package.source),
            "version":manifest["version"].as_str().unwrap_or(""),"description":manifest["description"].as_str().unwrap_or(""),
            "extensions":manifest["pi"].get("extensions").cloned().unwrap_or(json!([])),
            "prompts":manifest["pi"].get("prompts").cloned().unwrap_or(json!([])),
            "skills":manifest["pi"].get("skills").cloned().unwrap_or(json!([])),"resourceTypes":types})
    }).collect();
    json!({"cwd":cwd,"packages":packages})
}

async fn cwd(
    state: &AppState,
    params: &std::collections::HashMap<String, String>,
) -> Result<String, ApiError> {
    match params.get("sessionId").filter(|id| !id.is_empty()) {
        Some(id) => crate::session_api::session_cwd(state, id),
        None => Ok(state.cwd.clone()),
    }
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &params).await?;
    tokio::task::spawn_blocking(move || {
        let (manager, _) = manager(&cwd, &state.agent_dir)?;
        Ok(Json(dashboard(&manager, &cwd)))
    })
    .await
    .map_err(|_| ApiError::internal("扩展包读取任务失败。"))?
}

async fn change(
    state: Arc<AppState>,
    params: std::collections::HashMap<String, String>,
    body: Value,
    remove: bool,
) -> Result<Json<Value>, ApiError> {
    let cwd = cwd(&state, &params).await?;
    let source = body["source"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .ok_or_else(|| ApiError::bad_request("缺少有效的扩展包来源。"))?
        .trim()
        .to_owned();
    let local = body["scope"] == "project";
    tokio::task::spawn_blocking(move || {
        let (manager, settings) = manager(&cwd, &state.agent_dir)?;
        if remove {
            if !manager
                .remove_and_persist(&source, local)
                .map_err(|e| ApiError::bad_request(e.to_string()))?
            {
                return Err(ApiError::new(
                    axum::http::StatusCode::NOT_FOUND,
                    "extension_not_found",
                    "扩展包不存在。",
                ));
            }
        } else {
            manager
                .install_and_persist(&source, local)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
        settings.check()?;
        Ok(Json(dashboard(&manager, &cwd)))
    })
    .await
    .map_err(|_| ApiError::internal("扩展包更新任务失败。"))?
}

pub(crate) async fn install(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    change(state, params, body, false).await
}
pub(crate) async fn remove(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    change(state, params, body, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_package_settings_are_read_and_updated_in_place() {
        let root = std::env::temp_dir().join(format!("pisper-packages-{}", uuid::Uuid::new_v4()));
        let agent = root.join("agent");
        let extension = root.join("extension");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("package.json"),
            br#"{"name":"fixture","pi":{"extensions":["index.js"]}}"#,
        )
        .unwrap();
        let source = extension.to_str().unwrap();
        let original = json!({"packages":[{"source":source,"extensions":["index.js"],"futureFilter":42}],"futureSetting":{"keep":true}});
        std::fs::write(
            agent.join("settings.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        let (manager, settings) = manager(root.to_str().unwrap(), agent.to_str().unwrap()).unwrap();
        let listed = dashboard(&manager, root.to_str().unwrap());
        assert_eq!(listed["packages"][0]["name"], "fixture");
        assert_eq!(listed["packages"][0]["filtered"], true);
        manager.install_and_persist(source, false).unwrap();
        settings.check().unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(agent.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["futureSetting"], original["futureSetting"]);
        assert_eq!(saved["packages"][0]["futureFilter"], 42);
        assert!(manager.remove_and_persist(source, false).unwrap());
        settings.check().unwrap();
        assert!(manager.list_configured_packages().is_empty());
        assert!(!agent.join("extensions.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
