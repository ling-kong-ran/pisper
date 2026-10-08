//! Real native factory for upstream core/sdk.ts. All returned sessions use
//! ModelRuntime, Agent and AgentSession; no test factory or provider bypass.
//!
//! Native seams: resource loading is synchronous and TS extension execution
//! requires the existing injected loader; callback Futures are poll-driven.
//! Weak session handles replace the JS runner-ref (no ownership cycle). Header
//! runner is captured once per stream factory, other hooks read it per call.
//! Invalid JSON from an extension cannot become a typed transcript/header map:
//! it emits a diagnostic and preserves the input, rather than dropping entries.
//! Numeric request options must fit the existing unsigned native fields.
//! The SDK does not set a process-global default stream function (Agent's
//! per-instance Models fallback is retained). Services construction is separate.
use super::{
    auth_guidance::format_no_models_available_message,
    defaults::DEFAULT_THINKING_LEVEL,
    get_agent_dir,
    messages::convert_to_llm,
    model_resolver::{find_initial_model, FindInitialModelOptions, PrefetchedRuntime},
    model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
    path_join,
    provider_attribution::merge_provider_attribution_headers,
    resource_loader::{DefaultResourceLoader, DefaultResourceLoaderOptions},
    settings_manager::{SettingsManager, SettingsManagerCreateOptions, SettingsValue},
};
use crate::agent_core::{
    Agent, AgentInitialState, AgentMessage, AgentOptions, QueueMode, ThinkingLevel,
};
use crate::ai::models::{
    create_models, get_supported_thinking_levels, CreateModelsOptions, ModelsSimpleStreamOptions,
};
use crate::ai::transcript::Context;
use crate::ai::types::request_callbacks::RequestCallbacks;
use crate::ai::types::{
    Message, Model, ProviderHeaders, StringOrBlocks, TextContent, TextOrImageBlock, ThinkingBudgets,
};
use crate::coding_agent::agent_session::{AgentSession, AgentSessionConfig, ScopedModel};
use crate::coding_agent::extensions::runner::ExtensionRunner;
use crate::coding_agent::extensions::types::LoadExtensionsResult;
use crate::coding_agent::extensions::types::{ExtensionError, ToolDefinition};
use crate::coding_agent::session_manager::{
    get_default_session_dir_with, SessionEntry, SessionManager,
};
use crate::coding_agent::utils::paths::resolve_path_auto_base;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTools {
    All,
    Builtin,
}

#[derive(Default)]
pub struct CreateAgentSessionOptions {
    pub cwd: Option<String>,
    pub agent_dir: Option<String>,
    pub model_runtime: Option<ModelRuntime>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub scoped_models: Vec<ScopedModel>,
    pub no_tools: Option<NoTools>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub custom_tools: Vec<Arc<ToolDefinition>>,
    pub resource_loader: Option<Arc<Mutex<DefaultResourceLoader>>>,
    pub session_manager: Option<Arc<Mutex<SessionManager>>>,
    pub settings_manager: Option<SettingsManager>,
    pub session_start_event: Option<Value>,
}

pub struct CreateAgentSessionResult {
    pub session: Arc<AgentSession>,
    pub extensions_result: LoadExtensionsResult,
    pub model_fallback_message: Option<String>,
}

type RunnerSlot = Arc<Mutex<Weak<AgentSession>>>;
fn current_runner(slot: &RunnerSlot) -> Option<ExtensionRunner> {
    slot.lock()
        .expect("SDK runner slot")
        .upgrade()
        .map(|session| session.extension_runner())
}
fn invalid_hook(runner: &ExtensionRunner, event: &str, error: impl std::fmt::Display) {
    runner.emit_error(ExtensionError {
        extension_path: "<native-sdk-boundary>".into(),
        event: event.into(),
        error: format!("Invalid typed {event} result: {error}"),
        stack: None,
    });
}
fn thinking(value: &str) -> Option<ThinkingLevel> {
    serde_json::from_value(Value::String(value.into())).ok()
}
fn thinking_name(value: ThinkingLevel) -> String {
    serde_json::to_value(value)
        .expect("thinking enum")
        .as_str()
        .unwrap()
        .into()
}
fn clamp_thinking(model: &Model, level: ThinkingLevel) -> ThinkingLevel {
    let supported = get_supported_thinking_levels(model);
    let levels = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Xhigh,
        ThinkingLevel::Max,
    ];
    let index = levels
        .iter()
        .position(|candidate| *candidate == level)
        .unwrap();
    levels[index..]
        .iter()
        .chain(levels[..index].iter().rev())
        .copied()
        .find(|candidate| supported.contains(&thinking_name(*candidate).as_str()))
        .unwrap_or(ThinkingLevel::Off)
}
fn queue_mode(value: &str) -> QueueMode {
    if value == "all" {
        QueueMode::All
    } else {
        QueueMode::OneAtATime
    }
}
fn unsigned<T: TryFrom<i64>>(value: i64, field: &str) -> anyhow::Result<T> {
    T::try_from(value).map_err(|_| {
        anyhow::anyhow!("{field} cannot be represented by the native request option: {value}")
    })
}

// SettingsValue numbers are JS f64s; serializing 99.0 through serde_json and
// decoding as u32 rejects an otherwise valid integer. Check the exact native
// range here without truncating fractions or saturating invalid values.
fn native_thinking_budgets(value: SettingsValue) -> anyhow::Result<ThinkingBudgets> {
    anyhow::ensure!(value.is_object(), "thinkingBudgets must be an object");
    let budget = |name: &str| -> anyhow::Result<Option<u32>> {
        match value.get(name) {
            None | Some(SettingsValue::Null) => Ok(None),
            Some(SettingsValue::Num(number))
                if number.is_finite()
                    && *number >= 0.0
                    && *number <= f64::from(u32::MAX)
                    && number.fract() == 0.0 =>
            {
                Ok(Some(*number as u32))
            }
            _ => anyhow::bail!("thinkingBudgets.{name} must fit a native unsigned integer"),
        }
    };
    Ok(ThinkingBudgets {
        minimal: budget("minimal")?,
        low: budget("low")?,
        medium: budget("medium")?,
        high: budget("high")?,
    })
}

/// Convert custom messages first, then dynamically apply the image policy.
/// Only consecutive disabled-image placeholders in a message containing an
/// image are deduplicated, matching the upstream map-then-filter algorithm.
fn convert_with_images(messages: &[AgentMessage], settings: &SettingsManager) -> Vec<Message> {
    let mut messages = convert_to_llm(messages);
    if !settings.get_block_images() {
        return messages;
    }
    for message in &mut messages {
        let content = match message {
            Message::User(user) => match &mut user.content {
                StringOrBlocks::Blocks(content) => content,
                _ => continue,
            },
            Message::ToolResult(result) => &mut result.content,
            _ => continue,
        };
        if !content
            .iter()
            .any(|block| matches!(block, TextOrImageBlock::Image(_)))
        {
            continue;
        }
        let mut previous_placeholder = false;
        content.retain_mut(|block| {
            if matches!(block, TextOrImageBlock::Image(_)) {
                *block = TextOrImageBlock::Text(TextContent { text: "Image reading is disabled.".into(), text_signature: None });
            }
            let placeholder = matches!(block, TextOrImageBlock::Text(text) if text.text == "Image reading is disabled.");
            let keep = !(placeholder && previous_placeholder);
            previous_placeholder = placeholder;
            keep
        });
    }
    messages
}

// Upstream uses a truthy check for agentDir, unlike cwd's nullish fallback.
// Empty string means default directory and must not force auth/models paths.
fn resolve_sdk_agent_dir(path: Option<&str>) -> anyhow::Result<(String, bool)> {
    match path.filter(|path| !path.is_empty()) {
        Some(path) => Ok((resolve_path_auto_base(path)?, true)),
        None => Ok((get_agent_dir(), false)),
    }
}

pub async fn create_agent_session(
    options: CreateAgentSessionOptions,
) -> anyhow::Result<CreateAgentSessionResult> {
    let cwd = match options.cwd {
        Some(cwd) => cwd,
        None => match &options.session_manager {
            Some(manager) => manager
                .lock()
                .expect("session manager")
                .get_cwd()
                .to_owned(),
            None => std::env::current_dir()?.to_string_lossy().into_owned(),
        },
    };
    let cwd = resolve_path_auto_base(&cwd)?;
    let (agent_dir, explicit_agent_dir) = resolve_sdk_agent_dir(options.agent_dir.as_deref())?;
    let model_runtime = match options.model_runtime {
        Some(runtime) => runtime,
        None => ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: explicit_agent_dir.then(|| path_join(&agent_dir, "auth.json")),
            models_path: explicit_agent_dir.then(|| Some(path_join(&agent_dir, "models.json"))),
            ..Default::default()
        })
        .await
        .map_err(anyhow::Error::msg)?,
    };
    let settings = match options.settings_manager {
        Some(settings) => settings,
        None => {
            SettingsManager::create_with(&cwd, &agent_dir, SettingsManagerCreateOptions::default())?
        }
    };
    let session_manager = match options.session_manager {
        Some(manager) => manager,
        None => Arc::new(Mutex::new(SessionManager::create(
            &cwd,
            Some(&get_default_session_dir_with(&cwd, &agent_dir)),
            None,
        )?)),
    };
    let resource_loader = match options.resource_loader {
        Some(loader) => loader,
        None => {
            let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
                cwd: cwd.clone(),
                agent_dir,
                settings_manager: Some(Arc::new(settings.clone())),
                ..Default::default()
            });
            loader.reload_without_trust().map_err(anyhow::Error::msg)?;
            Arc::new(Mutex::new(loader))
        }
    };
    let (existing, has_thinking, session_id) = {
        let manager = session_manager.lock().expect("session manager");
        (
            manager.build_session_context(),
            manager
                .get_branch(None)
                .iter()
                .any(|entry| matches!(entry, SessionEntry::ThinkingLevelChange(_))),
            manager.get_session_id().to_owned(),
        )
    };
    let has_existing = !existing.messages.is_empty();
    let mut model = options.model;
    let mut fallback = None;
    if model.is_none() && has_existing {
        if let Some(saved) = &existing.model {
            model = model_runtime
                .get_model(&saved.provider, &saved.model_id)
                .await
                .filter(|model| model_runtime.has_configured_auth(&model.provider));
            if model.is_none() {
                fallback = Some(format!(
                    "Could not restore model {}/{}",
                    saved.provider, saved.model_id
                ));
            }
        }
    }
    if model.is_none() {
        // Existing native resolver adapter: one collection read, no network and
        // no reconstruction of a second ModelRuntime or credential store.
        let models = model_runtime.get_models(None).await;
        let reads = PrefetchedRuntime {
            available: model_runtime.get_available_snapshot(),
            configured_auth: models
                .iter()
                .filter(|model| model_runtime.has_configured_auth(&model.provider))
                .map(|model| model.provider.clone())
                .collect(),
            model_lookup: models
                .iter()
                .map(|model| ((model.provider.clone(), model.id.clone()), model.clone()))
                .collect(),
            models,
        };
        model = find_initial_model(FindInitialModelOptions {
            cli_provider: None,
            cli_model: None,
            scoped_models: vec![],
            is_continuing: has_existing,
            default_provider: settings.get_default_provider().as_deref(),
            default_model_id: settings.get_default_model().as_deref(),
            default_thinking_level: settings
                .get_default_thinking_level()
                .as_deref()
                .and_then(thinking),
            model_thinking_levels: settings
                .get_all_model_thinking_levels()
                .into_iter()
                .filter_map(|(key, value)| thinking(&value).map(|level| (key, level)))
                .collect(),
            model_runtime: &reads,
        })
        .await?
        .model;
        if let Some(model) = &model {
            if let Some(message) = &mut fallback {
                message.push_str(&format!(". Using {}/{}", model.provider, model.id));
            }
        } else {
            fallback = Some(format_no_models_available_message());
        }
    }
    let mut level = options.thinking_level;
    if level.is_none() && has_existing {
        level = Some(if has_thinking {
            thinking(&existing.thinking_level).unwrap_or(ThinkingLevel::Off)
        } else {
            settings
                .get_default_thinking_level()
                .as_deref()
                .and_then(thinking)
                .unwrap_or(DEFAULT_THINKING_LEVEL)
        });
    }
    if level.is_none() {
        if let Some(model) = &model {
            level = settings
                .get_model_thinking_level(&model.provider, &model.id)
                .as_deref()
                .and_then(thinking);
        }
    }
    let level = level
        .or_else(|| {
            settings
                .get_default_thinking_level()
                .as_deref()
                .and_then(thinking)
        })
        .unwrap_or(DEFAULT_THINKING_LEVEL);
    let level = model
        .as_ref()
        .map_or(ThinkingLevel::Off, |model| clamp_thinking(model, level));
    let allowed = options
        .tools
        .clone()
        .or_else(|| (options.no_tools == Some(NoTools::All)).then(Vec::new));
    // Upstream `usesDefaultTools: options.tools === undefined && !options.noTools`
    // — whether the initial tools came from the `defaultTools` setting.
    let tools_used = options.tools.is_some();
    let active = options
        .tools
        .unwrap_or_else(|| {
            if options.no_tools.is_some() {
                vec![]
            } else {
                settings.get_default_tools().unwrap_or_else(|| {
                    ["read", "bash", "edit", "write"]
                        .map(str::to_owned)
                        .to_vec()
                })
            }
        })
        .into_iter()
        .filter(|name| {
            !options
                .exclude_tools
                .as_ref()
                .is_some_and(|names| names.contains(name))
        })
        .collect();
    let slot: RunnerSlot = Arc::new(Mutex::new(Weak::new()));
    let stream_slot = slot.clone();
    let stream_settings = settings.clone();
    let stream_runtime = model_runtime.clone();
    let image_settings = settings.clone();
    let payload_slot = slot.clone();
    let response_slot = slot.clone();
    let context_slot = slot.clone();
    let agent = Arc::new(Agent::new(
        AgentOptions {
            initial_state: AgentInitialState {
                system_prompt: Some(String::new()),
                model: model.clone(),
                thinking_level: Some(level),
                ..Default::default()
            },
            convert_to_llm: Some(Arc::new(move |messages| {
                let settings = image_settings.clone();
                Box::pin(async move { convert_with_images(&messages, &settings) })
            })),
            stream_fn: Some(Arc::new(move |model, context, mut options| {
                let settings = stream_settings.clone();
                let runtime = stream_runtime.clone();
                let slot = stream_slot.clone();
                Box::pin(async move {
                    let retry = settings.get_provider_retry_settings();
                    let idle = settings
                        .get_http_idle_timeout_ms()
                        .map_err(anyhow::Error::msg)?;
                    options.stream.timeout_ms = Some(match options.stream.timeout_ms {
                        Some(value) => value,
                        None => match retry.timeout_ms {
                            Some(value) => unsigned(value, "timeoutMs")?,
                            None => {
                                if idle == 0 {
                                    2147483647
                                } else {
                                    idle
                                }
                            }
                        },
                    });
                    if options.stream.websocket_connect_timeout_ms.is_none() {
                        options.stream.websocket_connect_timeout_ms = settings
                            .get_websocket_connect_timeout_ms()
                            .map_err(anyhow::Error::msg)?
                            .map(|value| unsigned(value, "websocketConnectTimeoutMs"))
                            .transpose()?;
                    }
                    if options.stream.max_retries.is_none() {
                        options.stream.max_retries = retry
                            .max_retries
                            .map(|value| unsigned(value, "maxRetries"))
                            .transpose()?;
                    }
                    if options.stream.max_retry_delay_ms.is_none() {
                        options.stream.max_retry_delay_ms =
                            Some(unsigned(retry.max_retry_delay_ms, "maxRetryDelayMs")?);
                    }
                    let header_runner = current_runner(&slot);
                    let session_id = options.stream.session_id.clone();
                    let header_model = model.clone();
                    let transform = Arc::new(move |request_headers: ProviderHeaders| {
                        let headers = merge_provider_attribution_headers(
                            &header_model,
                            &settings,
                            session_id.as_deref(),
                            &[Some(&request_headers)],
                        )
                        .unwrap_or_default();
                        let runner = header_runner.clone();
                        Box::pin(async move {
                            if let Some(runner) = runner
                                .filter(|runner| runner.has_handlers("before_provider_headers"))
                            {
                                match serde_json::from_value(
                                    runner.emit_before_provider_headers(json!(headers)).await,
                                ) {
                                    Ok(headers) => return headers,
                                    Err(error) => {
                                        invalid_hook(&runner, "before_provider_headers", error)
                                    }
                                }
                            }
                            headers
                        })
                            as futures::future::BoxFuture<'static, ProviderHeaders>
                    });
                    Ok(runtime.stream_simple(
                        &model,
                        &Context {
                            messages: context.messages().to_vec(),
                            ..Default::default()
                        },
                        Some(ModelsSimpleStreamOptions {
                            simple: options,
                            transform_headers: Some(transform),
                        }),
                    ))
                })
            })),
            callbacks: RequestCallbacks {
                on_payload: Some(Arc::new(move |payload, _model| {
                    let slot = payload_slot.clone();
                    Box::pin(async move {
                        let runner = current_runner(&slot);
                        Ok(Some(
                            match runner
                                .filter(|runner| runner.has_handlers("before_provider_request"))
                            {
                                Some(runner) => runner.emit_before_provider_request(payload).await,
                                None => payload,
                            },
                        ))
                    })
                })),
                on_response: Some(Arc::new(move |response, _model| {
                    let slot = response_slot.clone();
                    Box::pin(async move {
                        if let Some(runner) = current_runner(&slot)
                            .filter(|runner| runner.has_handlers("after_provider_response"))
                        {
                            runner.emit(&mut json!({"type":"after_provider_response", "status":response.status,"headers":response.headers})).await;
                        }
                        Ok(())
                    })
                })),
                on_provider_stream_event: None,
            },
            transform_context: Some(Arc::new(move |messages| {
                let slot = context_slot.clone();
                Box::pin(async move {
                    if let Some(runner) = current_runner(&slot) {
                        let values = messages
                            .iter()
                            .map(|message| {
                                serde_json::to_value(message).expect("AgentMessage JSON")
                            })
                            .collect::<Vec<_>>();
                        match serde_json::from_value(Value::Array(
                            runner.emit_context(&values).await,
                        )) {
                            Ok(messages) => return messages,
                            Err(error) => invalid_hook(&runner, "context", error),
                        }
                    }
                    messages
                })
            })),
            session_id: Some(session_id),
            steering_mode: Some(queue_mode(&settings.get_steering_mode())),
            follow_up_mode: Some(queue_mode(&settings.get_follow_up_mode())),
            transport: Some(serde_json::from_value(json!(settings.get_transport()))?),
            thinking_budgets: settings
                .get_thinking_budgets()
                .map(native_thinking_budgets)
                .transpose()?,
            max_retry_delay_ms: Some(unsigned(
                settings.get_provider_retry_settings().max_retry_delay_ms,
                "maxRetryDelayMs",
            )?),
            ..Default::default()
        },
        Arc::new(create_models(CreateModelsOptions::default())),
    ));
    {
        let mut manager = session_manager.lock().expect("session manager");
        if has_existing {
            agent.state().messages = existing.messages;
            if !has_thinking {
                manager.append_thinking_level_change(&thinking_name(level))?;
            }
        } else {
            if let Some(model) = &model {
                manager.append_model_change(&model.provider, &model.id)?;
            }
            manager.append_thinking_level_change(&thinking_name(level))?;
        }
    }
    let session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings_manager: settings,
        cwd,
        scoped_models: options.scoped_models,
        resource_loader: resource_loader.clone(),
        custom_tools: options.custom_tools,
        model_runtime,
        initial_active_tool_names: Some(active),
        // Upstream `usesDefaultTools: options.tools === undefined && !options.noTools`.
        uses_default_tools: Some(!tools_used && options.no_tools.is_none()),
        allowed_tool_names: allowed,
        excluded_tool_names: options.exclude_tools,
        base_tools_override: vec![],
        session_start_event: options.session_start_event,
        html_exporter: None,
        // The session-factory CacheWarmer construction (upstream sdk.ts
        // `new CacheWarmer(modelRuntime, sessionManager, ...)`) is deferred
        // with the cache-warming wiring wave; the session accepts the option.
        cache_warmer: None,
    })?;
    *slot.lock().expect("SDK runner slot") = Arc::downgrade(&session);
    let extensions_result = resource_loader
        .lock()
        .expect("resource loader")
        .get_extensions();
    Ok(CreateAgentSessionResult {
        session,
        extensions_result,
        model_fallback_message: fallback,
    })
}

#[cfg(test)]
#[path = "sdk_tests.rs"]
mod tests;
