//! Cwd-bound main.ts runtime construction, including async project consent.
//!
//! Native factories/module loaders are explicit host bindings, not a claim that
//! unmodified TS extensions execute yet. Every returned runtime uses production
//! services, SDK and AgentSession; replacement sessions reuse this same factory.
use super::options::{build_session_options, resolve_cli_paths};
use crate::{
    ai::auth::types::AuthOperationOptions,
    coding_agent::{
        cli::{
            args::{Args, UnknownFlagValue},
            project_trust::{
                create_runtime_project_trust_context, AppMode, CreateProjectTrustContextOptions,
            },
        },
        core::{
            agent_session_runtime::{
                CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeOptions,
                CreateAgentSessionRuntimeResult,
            },
            agent_session_services::{
                create_agent_session_from_services, create_agent_session_services,
                AgentSessionRuntimeDiagnostic, CreateAgentSessionFromServicesOptions,
                CreateAgentSessionServicesOptions, DiagnosticType,
            },
            model_resolver::{resolve_model_scope, ConsoleLog, PrefetchedRuntime},
            model_runtime::ModelRuntime,
            project_trust::{resolve_project_trusted, ResolveProjectTrustedOptions},
            resource_loader::{
                DefaultResourceLoaderOptions, InlineExtension, ResolveProjectTrust,
                ResourceLoaderReloadOptions,
            },
            settings_diagnostics::collect_settings_diagnostics,
            settings_manager::{SettingsManager, SettingsManagerCreateOptions},
            trust_manager::{has_trust_requiring_project_resources, ProjectTrustStore},
        },
        extensions::{
            loader::ExtensionModuleLoader,
            types::{FlagValue, OrderedMap, ToolDefinition},
        },
    },
};
use anyhow::Result;
use futures::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Test/embedding seam returns a REAL native ModelRuntime for each effective cwd.
/// The default path delegates to createAgentSessionServices' real constructor.
pub type CliModelRuntimeFactory = Arc<
    dyn Fn(String, String, CancellationToken) -> BoxFuture<'static, Result<ModelRuntime>>
        + Send
        + Sync,
>;

/// Cwd-bound host tool definitions survive the SDK's tool registry rebuilds.
pub type CliCustomToolFactory = Arc<
    dyn Fn(String, SettingsManager) -> BoxFuture<'static, Result<Vec<Arc<ToolDefinition>>>>
        + Send
        + Sync,
>;

pub struct CliRuntimeFactoryOptions {
    pub parsed: Args,
    pub startup_cwd: String,
    pub initial_session_cwd: String,
    pub agent_dir: String,
    pub startup_settings_manager: SettingsManager,
    pub app_mode: AppMode,
    /// Supply built-in and caller factories in upstream order.
    pub extension_factories: Vec<InlineExtension>,
    pub extension_module_loader: Option<Arc<dyn ExtensionModuleLoader>>,
    pub model_runtime_factory: Option<CliModelRuntimeFactory>,
    pub custom_tool_factory: Option<CliCustomToolFactory>,
    pub model_scope_warning: Option<ConsoleLog>,
}

pub struct CliRuntimeFactory {
    pub create_runtime: CreateAgentSessionRuntimeFactory,
    pub auto_trust_on_reload_cwd: Option<String>,
}

/// Unlike wrapping the whole services future in a timeout, only the model
/// operation receives the deadline. A user may spend arbitrarily long in the
/// trust dialog after the catalog has initialized. Dropped operations cancel
/// their children, and finished operations do not leave a sleeping task behind.
pub(super) struct OperationDeadline {
    pub(super) token: CancellationToken,
    timer: tokio::task::JoinHandle<()>,
    finished: bool,
}
impl OperationDeadline {
    pub(super) fn new() -> Self {
        let token = CancellationToken::new();
        let signal = token.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            signal.cancel();
        });
        Self {
            token,
            timer,
            finished: false,
        }
    }
    pub(super) fn finish(&mut self) {
        self.finished = true;
        self.timer.abort();
    }
}
impl Drop for OperationDeadline {
    fn drop(&mut self) {
        self.timer.abort();
        if !self.finished {
            self.token.cancel();
        }
    }
}

pub fn create_cli_runtime_factory(options: CliRuntimeFactoryOptions) -> Result<CliRuntimeFactory> {
    let parsed = options.parsed;
    let auto_trust_on_reload_cwd = if parsed.project_trust_override.is_none()
        && !has_trust_requiring_project_resources(&options.initial_session_cwd)
            .map_err(anyhow::Error::msg)?
    {
        Some(options.initial_session_cwd.clone())
    } else {
        None
    };
    let trust_store = ProjectTrustStore::new(&options.agent_dir).map_err(anyhow::Error::msg)?;
    let trust_by_cwd = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
    let trust_prompt_mode = if parsed.help == Some(true) || parsed.list_models.is_some() {
        AppMode::Print
    } else {
        options.app_mode
    };
    let base_loader = DefaultResourceLoaderOptions {
        additional_extension_paths: resolve_cli_paths(
            &options.startup_cwd,
            parsed.extensions.as_deref(),
        )
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default(),
        additional_skill_paths: resolve_cli_paths(&options.startup_cwd, parsed.skills.as_deref())
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default(),
        additional_prompt_template_paths: resolve_cli_paths(
            &options.startup_cwd,
            parsed.prompt_templates.as_deref(),
        )
        .map_err(anyhow::Error::msg)?
        .unwrap_or_default(),
        additional_theme_paths: resolve_cli_paths(&options.startup_cwd, parsed.themes.as_deref())
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default(),
        no_extensions: parsed.no_extensions == Some(true),
        no_skills: parsed.no_skills == Some(true),
        no_prompt_templates: parsed.no_prompt_templates == Some(true),
        no_themes: parsed.no_themes == Some(true),
        no_context_files: parsed.no_context_files == Some(true),
        system_prompt: parsed.system_prompt.clone(),
        append_system_prompt: parsed.append_system_prompt.clone(),
        extension_factories: options.extension_factories,
        extension_module_loader: options.extension_module_loader,
        ..Default::default()
    };
    let mut flag_values = OrderedMap::new();
    for (name, value) in parsed.unknown_flags.iter() {
        flag_values.set(
            name,
            match value {
                UnknownFlagValue::Boolean(value) => FlagValue::Bool(*value),
                UnknownFlagValue::Text(value) => FlagValue::Str(value.clone()),
            },
        );
    }
    let parsed = Arc::new(parsed);
    let app_mode = options.app_mode;
    let startup_settings = options.startup_settings_manager;
    let model_runtime_factory = options.model_runtime_factory;
    let warning = options.model_scope_warning;
    let custom_tool_factory = options.custom_tool_factory;
    let create_runtime: CreateAgentSessionRuntimeFactory = Arc::new(
        move |input: CreateAgentSessionRuntimeOptions| {
            let custom_tool_factory = custom_tool_factory.clone();
            let (
                parsed,
                trust_store,
                trust_by_cwd,
                startup_settings,
                base_loader,
                flag_values,
                model_runtime_factory,
                warning,
            ) = (
                parsed.clone(),
                trust_store.clone(),
                trust_by_cwd.clone(),
                startup_settings.clone(),
                base_loader.clone(),
                flag_values.clone(),
                model_runtime_factory.clone(),
                warning.clone(),
            );
            Box::pin(async move {
                let cached = trust_by_cwd
                    .lock()
                    .expect("CLI trust cache")
                    .get(&input.cwd)
                    .copied();
                let has_resources = has_trust_requiring_project_resources(&input.cwd)
                    .map_err(anyhow::Error::msg)?;
                let should_resolve =
                    parsed.project_trust_override.is_none() && cached.is_none() && has_resources;
                let trusted = if should_resolve {
                    false
                } else {
                    match cached.or(parsed.project_trust_override) {
                        Some(value) => value,
                        None => {
                            !has_resources
                                || trust_store.get(&input.cwd).map_err(anyhow::Error::msg)?
                                    == Some(true)
                        }
                    }
                };
                let settings = SettingsManager::create_with(
                    &input.cwd,
                    &input.agent_dir,
                    SettingsManagerCreateOptions {
                        project_trusted: trusted,
                    },
                )?;
                let trust_diagnostics =
                    Arc::new(Mutex::new(Vec::<AgentSessionRuntimeDiagnostic>::new()));
                let reload_options = if should_resolve {
                    let cwd = input.cwd.clone();
                    let is_initial = input.session_start_event.is_none();
                    let context = input.project_trust_context.clone().unwrap_or_else(|| {
                        create_runtime_project_trust_context(CreateProjectTrustContextOptions {
                            cwd: cwd.clone(),
                            mode: if is_initial {
                                trust_prompt_mode
                            } else {
                                app_mode
                            },
                            settings_manager: Arc::new(startup_settings.clone()),
                            has_ui: is_initial && trust_prompt_mode == AppMode::Interactive,
                        })
                    });
                    let diagnostics = trust_diagnostics.clone();
                    let trust_parsed = parsed.clone();
                    let resolve: ResolveProjectTrust = Arc::new(move |extensions| {
                        let (cwd, context, diagnostics, cache, store, startup, parsed) = (
                            cwd.clone(),
                            context.clone(),
                            diagnostics.clone(),
                            trust_by_cwd.clone(),
                            trust_store.clone(),
                            startup_settings.clone(),
                            trust_parsed.clone(),
                        );
                        Box::pin(async move {
                            let report = |message| {
                                diagnostics.lock().expect("CLI trust diagnostics").push(
                                    AgentSessionRuntimeDiagnostic {
                                        kind: DiagnosticType::Warning,
                                        message,
                                    },
                                )
                            };
                            let trusted = resolve_project_trusted(ResolveProjectTrustedOptions {
                                cwd: &cwd,
                                trust_store: &store,
                                trust_override: parsed.project_trust_override,
                                default_project_trust: Some(startup.get_default_project_trust()),
                                extensions_result: Some(extensions),
                                project_trust_context: &context,
                                on_extension_error: Some(&report),
                            })
                            .await?;
                            cache.lock().expect("CLI trust cache").insert(cwd, trusted);
                            Ok(trusted)
                        })
                    });
                    Some(ResourceLoaderReloadOptions {
                        resolve_project_trust: Some(resolve),
                    })
                } else {
                    None
                };
                let mut deadline = OperationDeadline::new();
                let model_runtime = match model_runtime_factory {
                    Some(factory) => Some(
                        factory(
                            input.cwd.clone(),
                            input.agent_dir.clone(),
                            deadline.token.clone(),
                        )
                        .await?,
                    ),
                    None => None,
                };
                let services = create_agent_session_services(CreateAgentSessionServicesOptions {
                    cwd: input.cwd,
                    agent_dir: Some(input.agent_dir),
                    settings_manager: Some(settings),
                    model_runtime,
                    model_runtime_signal: Some(deadline.token.clone()),
                    extension_flag_values: Some(flag_values),
                    resource_loader_options: Some(base_loader),
                    resource_loader_reload_options: reload_options,
                })
                .await?;
                deadline.finish();
                let mut diagnostics = trust_diagnostics
                    .lock()
                    .expect("CLI trust diagnostics")
                    .clone();
                diagnostics.extend(services.diagnostics.iter().cloned());
                diagnostics.extend(collect_settings_diagnostics(&services.settings_manager));
                {
                    let loader = services.resource_loader.lock().expect("resource loader");
                    diagnostics.extend(loader.get_extensions().errors.iter().map(|error| {
                        AgentSessionRuntimeDiagnostic {
                            kind: DiagnosticType::Error,
                            message: format!(
                                "Failed to load extension \"{}\": {}",
                                error.path, error.error
                            ),
                        }
                    }));
                }
                let patterns = parsed
                    .models
                    .clone()
                    .or_else(|| services.settings_manager.get_enabled_models());
                let available = if patterns.as_ref().is_some_and(|p| !p.is_empty()) {
                    let mut deadline = OperationDeadline::new();
                    let auth_options = AuthOperationOptions::new(deadline.token.clone());
                    let models = services
                        .model_runtime
                        .get_available(None, Some(&auth_options))
                        .await?;
                    deadline.finish();
                    models
                } else {
                    services.model_runtime.get_available_snapshot()
                };
                let models = services.model_runtime.get_models(None).await;
                let reads = PrefetchedRuntime {
                    configured_auth: models
                        .iter()
                        .filter(|m| services.model_runtime.has_configured_auth(&m.provider))
                        .map(|m| m.provider.clone())
                        .collect(),
                    model_lookup: models
                        .iter()
                        .map(|m| ((m.provider.clone(), m.id.clone()), m.clone()))
                        .collect(),
                    models,
                    available,
                };
                let scoped_models = match &patterns {
                    Some(patterns) if !patterns.is_empty() => {
                        resolve_model_scope(patterns, &reads, warning.as_ref()).await
                    }
                    _ => vec![],
                };
                let has_existing = !input
                    .session_manager
                    .lock()
                    .expect("session manager")
                    .build_session_context()
                    .messages
                    .is_empty();
                let built = build_session_options(
                    &parsed,
                    &scoped_models,
                    has_existing,
                    &reads,
                    &services.settings_manager,
                );
                diagnostics.extend(built.diagnostics);
                let mut session_options = built.options;
                if let Some(factory) = custom_tool_factory {
                    session_options.custom_tools.extend(
                        factory(services.cwd.clone(), services.settings_manager.clone()).await?,
                    );
                }
                if let Some(key) = parsed.api_key.as_ref().filter(|k| !k.is_empty()) {
                    match &session_options.model {
                    Some(model)=>services.model_runtime.set_runtime_api_key(&model.provider,key,None).await?,
                    None=>diagnostics.push(AgentSessionRuntimeDiagnostic {kind:DiagnosticType::Error,message:"--api-key requires a model to be specified via --model, --provider/--model, or --models".into()}),
                }
                }
                let created =
                    create_agent_session_from_services(CreateAgentSessionFromServicesOptions {
                        services: services.clone(),
                        session_manager: input.session_manager,
                        session_start_event: input
                            .session_start_event
                            .map(serde_json::to_value)
                            .transpose()?,
                        model: session_options.model,
                        thinking_level: session_options.thinking_level,
                        scoped_models: session_options.scoped_models,
                        tools: session_options.tools,
                        exclude_tools: session_options.exclude_tools,
                        no_tools: session_options.no_tools,
                        custom_tools: session_options.custom_tools,
                    })
                    .await?;
                if created.session.model().is_some()
                    && (parsed.thinking.is_some() || built.cli_thinking_from_model)
                {
                    created
                        .session
                        .set_thinking_level(created.session.thinking_level(), None);
                }
                Ok(CreateAgentSessionRuntimeResult {
                    session: created.session,
                    extensions_result: created.extensions_result,
                    model_fallback_message: created.model_fallback_message,
                    services: Arc::new(services),
                    diagnostics,
                })
            })
        },
    );
    Ok(CliRuntimeFactory {
        create_runtime,
        auto_trust_on_reload_cwd,
    })
}
