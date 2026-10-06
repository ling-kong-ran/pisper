//! Cwd-bound services and session factories from `agent-session-services.ts`.
//! Construction reuses real native runtime/settings handles, reloads resources,
//! drains providers in upstream order, refreshes offline and applies flags.
//! TS execution still depends on the ResourceLoader's installed module loader;
//! provider callbacks retain native identity rather than passing through JSON.
use super::{
    get_agent_dir,
    model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
    path_join,
    resource_loader::{
        DefaultResourceLoader, DefaultResourceLoaderOptions, ResourceLoaderReloadOptions,
    },
    sdk::{create_agent_session, CreateAgentSessionOptions, CreateAgentSessionResult, NoTools},
    settings_manager::{SettingsManager, SettingsManagerCreateOptions},
};
use crate::agent_core::ThinkingLevel;
use crate::ai::{models::ModelsRefreshOptions, types::Model};
use crate::coding_agent::{
    agent_session::{provider_config_from_value, ScopedModel},
    extensions::types::{FlagType, FlagValue, OrderedMap, ToolDefinition},
    session_manager::SessionManager,
    utils::paths::resolve_path_auto_base,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticType {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionRuntimeDiagnostic {
    #[serde(rename = "type")]
    pub kind: DiagnosticType,
    pub message: String,
}

/// Infrastructure for one effective cwd. Clones retain the same native handles.
#[derive(Clone)]
pub struct AgentSessionServices {
    pub cwd: String,
    pub agent_dir: String,
    pub model_runtime: ModelRuntime,
    pub settings_manager: SettingsManager,
    pub resource_loader: Arc<Mutex<DefaultResourceLoader>>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

/// CLI resource paths must already be absolute before switching cwd.
#[derive(Default)]
pub struct CreateAgentSessionServicesOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub settings_manager: Option<SettingsManager>,
    pub model_runtime: Option<ModelRuntime>,
    pub model_runtime_signal: Option<CancellationToken>,
    pub extension_flag_values: Option<OrderedMap<FlagValue>>,
    pub resource_loader_options: Option<DefaultResourceLoaderOptions>,
    pub resource_loader_reload_options: Option<ResourceLoaderReloadOptions>,
}

pub struct CreateAgentSessionFromServicesOptions {
    pub services: AgentSessionServices,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub session_start_event: Option<Value>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub scoped_models: Vec<ScopedModel>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub no_tools: Option<NoTools>,
    pub custom_tools: Vec<Arc<ToolDefinition>>,
}

impl CreateAgentSessionFromServicesOptions {
    pub fn new(
        services: AgentSessionServices,
        session_manager: Arc<Mutex<SessionManager>>,
    ) -> Self {
        Self {
            services,
            session_manager,
            session_start_event: None,
            model: None,
            thinking_level: None,
            scoped_models: Vec::new(),
            tools: None,
            exclude_tools: None,
            no_tools: None,
            custom_tools: Vec::new(),
        }
    }
}

fn error(message: String) -> AgentSessionRuntimeDiagnostic {
    AgentSessionRuntimeDiagnostic {
        kind: DiagnosticType::Error,
        message,
    }
}

fn apply_extension_flag_values(
    resource_loader: &DefaultResourceLoader,
    values: Option<&OrderedMap<FlagValue>>,
) -> Vec<AgentSessionRuntimeDiagnostic> {
    let Some(values) = values else {
        return Vec::new();
    };
    let extensions = resource_loader.get_extensions();
    let mut registered = OrderedMap::new();
    for extension in &extensions.extensions {
        for (name, flag) in extension.flags.iter() {
            registered.set(name, flag.flag_type);
        }
    }
    let mut diagnostics = Vec::new();
    let mut unknown = Vec::new();
    for (name, value) in values.iter() {
        match registered.get(name) {
            None => unknown.push(format!("--{name}")),
            Some(FlagType::Boolean) => extensions
                .runtime
                .set_flag_value(name, FlagValue::Bool(true)),
            Some(FlagType::String) => match value {
                FlagValue::Str(_) => extensions.runtime.set_flag_value(name, value.clone()),
                FlagValue::Bool(_) => diagnostics.push(error(format!(
                    "Extension flag \"--{name}\" requires a value"
                ))),
            },
        }
    }
    if !unknown.is_empty() {
        diagnostics.push(error(format!(
            "Unknown option{}: {}",
            if unknown.len() == 1 { "" } else { "s" },
            unknown.join(", ")
        )));
    }
    diagnostics
}

/// Upstream createAgentSessionServices: construction failures propagate, but
/// individual provider registration errors become ordered non-fatal diagnostics.
pub async fn create_agent_session_services(
    options: CreateAgentSessionServicesOptions,
) -> anyhow::Result<AgentSessionServices> {
    let cwd = resolve_path_auto_base(&options.cwd)?;
    let agent_dir = match options.agent_dir.filter(|path| !path.is_empty()) {
        Some(path) => resolve_path_auto_base(&path)?,
        None => get_agent_dir(),
    };
    let model_runtime = match options.model_runtime {
        Some(runtime) => runtime,
        None => ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: Some(path_join(&agent_dir, "auth.json")),
            models_path: Some(Some(path_join(&agent_dir, "models.json"))),
            signal: options.model_runtime_signal,
            ..CreateModelRuntimeOptions::default()
        })
        .await
        .map_err(anyhow::Error::msg)?,
    };
    let settings_manager = match options.settings_manager {
        Some(settings) => settings,
        None => {
            SettingsManager::create_with(&cwd, &agent_dir, SettingsManagerCreateOptions::default())?
        }
    };
    let mut loader_options = options.resource_loader_options.unwrap_or_default();
    loader_options.cwd = cwd.clone();
    loader_options.agent_dir = agent_dir.clone();
    loader_options.settings_manager = Some(Arc::new(settings_manager.clone()));
    let mut resource_loader = DefaultResourceLoader::new(loader_options);
    resource_loader
        .reload(options.resource_loader_reload_options)
        .await
        .map_err(anyhow::Error::msg)?;
    let extensions = resource_loader.get_extensions();
    let mut diagnostics = Vec::new();
    extensions.runtime.flush_pending_providers(|registration| {
        let result = provider_config_from_value(&registration.config).and_then(|input| {
            model_runtime
                .register_provider_sync(&registration.name, input)
                .map_err(|failure| failure.0)
        });
        if let Err(failure) = result {
            diagnostics.push(error(format!(
                "Extension \"{}\" error: {}",
                registration.extension_path, failure
            )));
        }
    });
    extensions
        .runtime
        .flush_pending_native_providers(|registration| {
            if let Err(failure) = model_runtime.register_native_provider_sync(registration.provider)
            {
                diagnostics.push(error(format!(
                    "Extension \"{}\" error: {}",
                    registration.extension_path, failure.0
                )));
            }
        });
    model_runtime
        .refresh(ModelsRefreshOptions {
            allow_network: Some(false),
            ..Default::default()
        })
        .await
        .map_err(anyhow::Error::msg)?;
    diagnostics.extend(apply_extension_flag_values(
        &resource_loader,
        options.extension_flag_values.as_ref(),
    ));
    Ok(AgentSessionServices {
        cwd,
        agent_dir,
        model_runtime,
        settings_manager,
        resource_loader: Arc::new(Mutex::new(resource_loader)),
        diagnostics,
    })
}

/// Calls the real SDK factory with the same service and session identities.
pub async fn create_agent_session_from_services(
    options: CreateAgentSessionFromServicesOptions,
) -> anyhow::Result<CreateAgentSessionResult> {
    create_agent_session(CreateAgentSessionOptions {
        cwd: Some(options.services.cwd),
        agent_dir: Some(options.services.agent_dir),
        model_runtime: Some(options.services.model_runtime),
        settings_manager: Some(options.services.settings_manager),
        resource_loader: Some(options.services.resource_loader),
        session_manager: Some(options.session_manager),
        session_start_event: options.session_start_event,
        model: options.model,
        thinking_level: options.thinking_level,
        scoped_models: options.scoped_models,
        tools: options.tools,
        exclude_tools: options.exclude_tools,
        no_tools: options.no_tools,
        custom_tools: options.custom_tools,
    })
    .await
}

#[cfg(test)]
#[path = "agent_session_services_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_session_services_oracle_tests.rs"]
mod oracle_tests;
