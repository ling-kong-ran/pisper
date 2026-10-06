//! 组合根适配：工具领域只接收配置、执行范围及刷新端口，不持有 AppState。
use crate::{
    approval_api::{AuthorizationRequest, CancelFuture},
    native_plugins::{
        Catalog, ConfigPort, ExecutionScope, ExecutionScopePort, OnPluginsChanged, PluginError,
        Result, ToolPluginService,
    },
    native_tool_gateway::{GatewayBlock, GatewayPort, GatewaySession},
    plugins_api::PluginHooks,
    provider_config::ProviderConfigStore,
    AppState,
};
use pi_rust::coding_agent::{agent_session::AgentSession, extensions::types::ToolExposure};
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, OnceLock, Weak},
};

pub(crate) fn config_port(providers: Arc<ProviderConfigStore>) -> ConfigPort {
    let read = providers.clone();
    ConfigPort {
        read: Arc::new(move || {
            read.app_preferences()
                .map_err(|error| PluginError::new(error.message))
        }),
        update: Arc::new(move |mutation| {
            let providers = providers.clone();
            Box::pin(async move {
                providers
                    .mutate_app_preferences(move |value| {
                        mutation(value).map_err(|error| crate::ApiError::bad_request(error.message))
                    })
                    .await
                    .map_err(|error| PluginError::new(error.message))
            })
        }),
        normalize_web_search: Arc::new(|value| {
            crate::native_web_search::normalize_config_checked(&value)
                .map_err(|error| PluginError::new(error.message))
        }),
    }
}

pub(crate) async fn migrate_defaults(service: &ToolPluginService) -> Result<()> {
    for (ids, key) in [
        (&["memory_search", "memory_remember"][..], "memoryToolsV1"),
        (&["mcp_list", "mcp_manage"][..], "mcpManagementToolsV1"),
        (&["web_search"][..], "webSearchToolV1"),
        (&["browser_automation"][..], "browserAutomationToolV1"),
        (&["skill_create"][..], "skillCreateToolV1"),
        (&["plugin_create"][..], "pluginCreateToolV1"),
        (&["mobile_device"][..], "mobileDeviceToolV2"),
        (&["generate_visual"][..], "visualGenerateToolV1"),
    ] {
        service.ensure_default_tools(ids, key).await?;
    }
    Ok(())
}

pub(crate) struct PluginIntegration {
    state: OnceLock<Weak<AppState>>,
    catalog: Catalog,
    providers: Arc<ProviderConfigStore>,
    plugins: Arc<ToolPluginService>,
}
impl PluginIntegration {
    pub(crate) fn new(
        catalog: Catalog,
        providers: Arc<ProviderConfigStore>,
        plugins: Arc<ToolPluginService>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: OnceLock::new(),
            catalog,
            providers,
            plugins,
        })
    }
    pub(crate) fn attach(&self, state: &Arc<AppState>) -> Result<()> {
        self.state
            .set(Arc::downgrade(state))
            .map_err(|_| PluginError::new("工具适配器已连接。"))
    }
    fn state(&self) -> Result<Arc<AppState>> {
        self.state
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| PluginError::new("工具运行时尚未连接或已关闭。"))
    }
    fn mode(&self, state: &AppState, native_id: &str) -> Result<(String, String, String)> {
        let owner = state.executor.tool_owner(native_id);
        let meta = state
            .session_meta
            .lock()
            .map_err(|_| PluginError::new("工具权限元数据锁失败。"))?
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let configured_mode = meta
            .execution_mode
            .unwrap_or_else(|| crate::execution_modes::DEFAULT_EXECUTION_MODE.to_owned());
        let mode = crate::execution_modes::normalize(&configured_mode)
            .unwrap_or(crate::execution_modes::DEFAULT_EXECUTION_MODE)
            .to_owned();
        let permission = meta
            .permission_mode
            .and_then(|value| match value.as_str() {
                "full" => Some("ignore".to_owned()),
                "ask" | "auto" | "ignore" => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| crate::execution_modes::permission_mode(&mode).to_owned());
        Ok((owner, mode, permission))
    }
    pub(crate) fn scope_port(self: &Arc<Self>) -> ExecutionScopePort {
        let integration = self.clone();
        Arc::new(move |native_id| {
            let state = integration.state()?;
            let (session_id, execution_mode, _) = integration.mode(&state, native_id)?;
            Ok(ExecutionScope {
                session_id,
                execution_mode,
            })
        })
    }
    pub(crate) fn changed_port(self: &Arc<Self>) -> OnPluginsChanged {
        let integration = self.clone();
        Arc::new(move || {
            let integration = integration.clone();
            Box::pin(async move {
                let state = integration.state()?;
                // 忙碌会话保留本轮运行所有权；下一次空闲准入创建新的注册表。
                state.sessions.refresh_tools();
                integration.apply_loadout(state.runtime.session())
            })
        })
    }
    fn session(&self, state: &AppState, native_id: &str) -> Result<Arc<AgentSession>> {
        if state.runtime.session().session_id() == native_id {
            return Ok(state.runtime.session());
        }
        state
            .sessions
            .get(native_id)
            .map(|host| host.session())
            .or_else(|| state.executor.tool_session(native_id))
            .ok_or_else(|| PluginError::new("工具实际会话不可用。"))
    }
    fn allowed(&self, session: &AgentSession, mode: &str) -> Result<HashSet<String>> {
        let app = self
            .providers
            .app_preferences()
            .map_err(|error| PluginError::new(error.message))?;
        let enabled = self.plugins.enabled_tools(&app, mode)?;
        let configured = self.catalog.ids();
        let state = self.state()?;
        let limit = state.executor.tool_limit(&session.session_id());
        Ok(session
            .get_all_tools()
            .into_iter()
            .filter_map(|tool| {
                let name = tool.name;
                if limit.as_ref().is_some_and(|names| !names.contains(&name)) {
                    return None;
                }
                if crate::native_tool_catalog::is_legacy(&name) || name == "powershell" {
                    return None;
                }
                if configured.contains(&name) && !enabled.contains(&name) {
                    return None;
                }
                if self.plugins.is_third_party_tool(&name)
                    && (mode != "full-access" || !enabled.contains(&name))
                {
                    return None;
                }
                if !crate::execution_modes::visible(&name, mode, self.plugins.get_tool_risk(&name))
                {
                    return None;
                }
                let definition = session.get_tool_definition(&name)?;
                if matches!(
                    definition.exposure,
                    ToolExposure::Hidden | ToolExposure::ModelOnly
                ) {
                    return None;
                }
                Some(name)
            })
            .collect())
    }
    pub(crate) fn apply_loadout(&self, session: Arc<AgentSession>) -> Result<()> {
        let state = self.state()?;
        let (_, mode, _) = self.mode(&state, &session.session_id())?;
        let allowed = self.allowed(&session, &mode)?;
        let mut names: Vec<_> = allowed.into_iter().filter(|name| {
            crate::native_tool_catalog::is_hot(name)
                // Goal/Team 的运行状态接线保留；其独立契约仍在完整 parity 审计范围。
                || matches!(name.as_str(), "get_goal" | "update_goal" | "send_team_message" | "list_team_members" | "update_team_task" | "run_team_workflow")
        }).collect();
        names.sort();
        session.set_active_tools_by_name(names);
        Ok(())
    }
    pub(crate) fn gateway_port(self: &Arc<Self>) -> GatewayPort {
        let integration = self.clone();
        Arc::new(move |native_id| {
            let integration = integration.clone();
            Box::pin(async move {
                let state = integration.state().map_err(|error| error.message)?;
                let session = integration
                    .session(&state, &native_id)
                    .map_err(|error| error.message)?;
                let (owner, mode, permission) = integration
                    .mode(&state, &native_id)
                    .map_err(|error| error.message)?;
                let allowed = integration
                    .allowed(&session, &mode)
                    .map_err(|error| error.message)?;
                let callable = allowed
                    .into_iter()
                    .filter_map(|name| session.get_tool_definition(&name))
                    .collect();
                let active_names = session.get_active_tool_names().into_iter().collect();
                let cwd = session
                    .session_manager
                    .lock()
                    .map_err(|_| "工具会话锁失败".to_owned())?
                    .get_cwd()
                    .to_owned();
                let approvals = state.approvals.clone();
                let owned_files = state.executor.tool_owned_files(&native_id);
                Ok(GatewaySession {
                    callable,
                    active_names,
                    authorize: Arc::new(move |tool_name, tool_call_id, args, signal| {
                        let (approvals, owner, cwd, mode, permission, owned_files) = (
                            approvals.clone(),
                            owner.clone(),
                            cwd.clone(),
                            mode.clone(),
                            permission.clone(),
                            owned_files.clone(),
                        );
                        Box::pin(async move {
                            let cancelled = tokio_util::sync::CancellationToken::new();
                            let token = cancelled.clone();
                            let _subscription = signal
                                .as_ref()
                                .map(|signal| signal.on_abort(Arc::new(move || token.cancel())));
                            let cancel: Option<CancelFuture> = signal.map(|_| {
                                Box::pin(async move { cancelled.cancelled().await }) as CancelFuture
                            });
                            let outcome = approvals
                                .authorize(
                                    AuthorizationRequest {
                                        session_id: owner,
                                        cwd,
                                        tool_name,
                                        tool_call_id,
                                        args,
                                        permission_mode: permission,
                                        execution_mode: mode,
                                        owned_files,
                                    },
                                    cancel,
                                )
                                .await;
                            Ok(outcome
                                .filter(|value| value.block == Some(true))
                                .map(|value| GatewayBlock {
                                    reason: value.reason,
                                }))
                        })
                    }),
                })
            })
        })
    }
    pub(crate) fn http_hooks(self: &Arc<Self>) -> PluginHooks {
        let integration = self.clone();
        PluginHooks {
            reload: self.changed_port(),
            project: Some(Arc::new(move |mut value, session_id| {
                let integration = integration.clone();
                Box::pin(async move {
                    let state = integration.state()?;
                    let id = session_id.unwrap_or_else(|| state.runtime.session().session_id());
                    let (_, mode, _) = integration.mode(&state, &id)?;
                    let mut registered: HashSet<_> = state
                        .runtime
                        .session()
                        .get_all_tools()
                        .into_iter()
                        .map(|tool| tool.name)
                        .collect();
                    // 模板初始停用不等于实现缺失；启用后公共运行时会按版本重建。
                    registered.extend(["plugin_create".to_owned(), "web_search".to_owned()]);
                    let visible: HashSet<_> = value["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|tool| tool["id"].as_str())
                        .filter(|name| {
                            registered.contains(*name)
                                || integration.plugins.is_third_party_tool(name)
                        })
                        .map(str::to_owned)
                        .collect();
                    if let Some(tools) = value["tools"].as_array_mut() {
                        tools.retain(|tool| {
                            tool["id"]
                                .as_str()
                                .is_some_and(|name| visible.contains(name))
                        });
                    }
                    if let Some(enabled) = value["enabledTools"].as_array_mut() {
                        enabled.retain(|name| {
                            name.as_str().is_some_and(|name| visible.contains(name))
                        });
                    }
                    if let Some(plugins) = value["plugins"].as_array_mut() {
                        for plugin in plugins.iter_mut() {
                            if plugin["builtIn"] == true {
                                if let Some(capabilities) = plugin["capabilities"].as_array_mut() {
                                    capabilities.retain(|capability| {
                                        capability["name"]
                                            .as_str()
                                            .is_some_and(|name| visible.contains(name))
                                    });
                                }
                            }
                        }
                        plugins.retain(|plugin| {
                            plugin["capabilities"]
                                .as_array()
                                .is_some_and(|capabilities| !capabilities.is_empty())
                        });
                    }
                    if let Some(presets) = value["presets"].as_object_mut() {
                        for tools in presets.values_mut() {
                            if let Some(tools) = tools.as_array_mut() {
                                tools.retain(|name| {
                                    name.as_str().is_some_and(|name| visible.contains(name))
                                });
                            }
                        }
                    }
                    value["computerUseEnabled"] = json!(false);
                    let app = integration
                        .providers
                        .app_preferences()
                        .map_err(|error| PluginError::new(error.message))?;
                    let mut callable = integration.allowed(&state.runtime.session(), &mode)?;
                    callable.extend(
                        integration
                            .plugins
                            .enabled_tools(&app, &mode)?
                            .into_iter()
                            .filter(|name| {
                                visible.contains(name)
                                    && crate::execution_modes::visible(
                                        name,
                                        &mode,
                                        integration.plugins.get_tool_risk(name),
                                    )
                            }),
                    );
                    let mut callable: Vec<_> = callable.into_iter().collect();
                    callable.sort();
                    value["callableToolNames"] = json!(callable);
                    value["nativeCompatibility"] = crate::native_plugins::compatibility();
                    Ok(value)
                })
            })),
        }
    }
}
