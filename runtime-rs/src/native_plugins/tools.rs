//! 工具注册采用模板快照，执行权限与 cwd/会话身份必须取自当前 Pi 调用上下文。
use super::{PluginError, Result, ToolPluginService};
use futures::future::BoxFuture;
use pi_rust::coding_agent::{
    core::resource_loader::InlineExtension,
    extensions::{
        loader::ExtensionFactory,
        types::{ExtensionContext, ToolDefinition},
    },
    session_manager::SessionManager,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ExecutionScope {
    pub session_id: String,
    pub execution_mode: String,
}
pub type ExecutionScopePort = Arc<dyn Fn(&str) -> Result<ExecutionScope> + Send + Sync>;
pub type OnPluginsChanged = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

fn current_scope(
    ctx: &ExtensionContext,
    port: &ExecutionScopePort,
) -> Result<(PathBuf, String, ExecutionScope)> {
    let cwd = PathBuf::from(ctx.cwd().map_err(PluginError::new)?);
    let manager = ctx
        .session_manager()
        .map_err(PluginError::new)?
        .downcast::<Mutex<SessionManager>>()
        .map_err(|_| PluginError::new("插件工具的会话上下文不可用。"))?;
    let native_id = manager
        .lock()
        .map_err(|_| PluginError::new("会话上下文锁失败。"))?
        .get_session_id()
        .to_owned();
    let scope = port(&native_id)?;
    if scope.session_id.is_empty() || scope.execution_mode.is_empty() {
        return Err(PluginError::new("插件工具的实际执行范围不可用。"));
    }
    Ok((cwd, native_id, scope))
}

pub fn create_extension(
    service: Arc<ToolPluginService>,
    scope_port: ExecutionScopePort,
    on_changed: OnPluginsChanged,
) -> InlineExtension {
    let factory: ExtensionFactory = Arc::new(move |api| {
        let app = service.app_config().map_err(|error| error.message)?;
        let enabled = service
            .enabled_tools(&app, "full-access")
            .map_err(|error| error.message)?;
        for (plugin_id, manifest) in service
            .registered_local_tools()
            .map_err(|error| error.message)?
        {
            if !enabled.contains(&manifest.name) {
                continue;
            }
            let mut tool = ToolDefinition::new(
                &manifest.name,
                &manifest.label,
                &manifest.description,
                manifest.parameters,
            );
            tool.default_active = Some(false);
            let service = service.clone();
            let scope_port = scope_port.clone();
            let name = manifest.name;
            tool.execute_async = Some(Arc::new(move |_, arguments, signal, _, ctx| {
                let (service, scope_port, plugin_id, name) = (
                    service.clone(),
                    scope_port.clone(),
                    plugin_id.clone(),
                    name.clone(),
                );
                Box::pin(async move {
                    let (cwd, native_id, scope) =
                        current_scope(&ctx, &scope_port).map_err(|error| error.message)?;
                    if scope.execution_mode != "full-access" {
                        return Err(
                            "第三方插件能力仅在“完全访问”执行模式下提供给 Agent。".to_owned()
                        );
                    }
                    let app = service.app_config().map_err(|error| error.message)?;
                    if !service
                        .enabled_tools(&app, &scope.execution_mode)
                        .map_err(|error| error.message)?
                        .contains(&name)
                    {
                        return Err(format!("插件能力 {name} 已停用。"));
                    }
                    let cancel = CancellationToken::new();
                    let token = cancel.clone();
                    let _subscription =
                        signal.map(|signal| signal.on_abort(Arc::new(move || token.cancel())));
                    service
                        .execute(&plugin_id, &name, arguments, cwd, native_id, Some(cancel))
                        .await
                        .map_err(|error| error.message)
                })
            }));
            api.register_tool(tool)?;
        }
        if enabled.iter().any(|name| name == "plugin_create") {
            let mut tool = ToolDefinition::new("plugin_create", "Plugin Create", "Create and install a standards-compliant local Pisper plugin for all projects. This is the registered tool for creating reusable Pisper plugins and new Agent tools; the call requires user approval unless execution is already in full-access mode.", create_schema());
            tool.default_active = Some(false);
            tool.prompt_snippet = Some("Create, validate, and install a globally available local Pisper plugin with plugin_create".into());
            tool.prompt_guidelines = Some(vec![
                "Use plugin_create directly when the user explicitly asks to create and install a reusable Pisper plugin or new Agent tool. Do not substitute ordinary scripts or one-off code changes.".into(),
                "A plugin is one installable unit that may provide multiple related tools. Define every tool with a precise description, object JSON Schema, and honest scope.".into(),
                "entryCode must export async function execute({ toolName, arguments: input, context }). Return a string or a Pi tool result with a content array.".into(),
                "Use context.cwd for the current workspace, context.sessionId for the chat identity, and context.dataDir for persistent plugin-owned data.".into(),
                "The native Rust host currently implements a subset of Node built-ins and bundled JavaScript. Unsupported Node/npm APIs fail explicitly; complete Node compatibility is not advertised.".into(),
                "Plugin code runs as the current operating-system user in an isolated Worker, not an OS sandbox. Minimize file and network access and never embed credentials.".into(),
                "The tool writes to the global Pisper plugin-sources/<plugin-id> directory, refuses to overwrite source or installed plugins, validates the same manifest used by the Plugins page, and installs the plugin for all projects from the next Agent turn.".into(),
            ]);
            let (service, scope_port, on_changed) =
                (service.clone(), scope_port.clone(), on_changed.clone());
            tool.execute_async = Some(Arc::new(move |_, arguments, signal, _, ctx| {
                let (service, scope_port, on_changed) =
                    (service.clone(), scope_port.clone(), on_changed.clone());
                Box::pin(async move {
                    current_scope(&ctx, &scope_port).map_err(|error| error.message)?;
                    if signal.as_ref().is_some_and(|signal| signal.is_aborted()) {
                        return Err("插件创建已取消。".into());
                    }
                    if !service
                        .enabled_tools(
                            &service.app_config().map_err(|error| error.message)?,
                            "workspace-write",
                        )
                        .map_err(|error| error.message)?
                        .iter()
                        .any(|name| name == "plugin_create")
                    {
                        return Err("插件创建工具已停用。".into());
                    }
                    // 源码创建与安装提交持有服务所有权；中途取消不能把已写入的源码误报为未发生。
                    let task = tokio::spawn(async move {
                        let result = service
                            .create(arguments)
                            .await
                            .map_err(|error| error.message)?;
                        if let Err(error) = on_changed().await {
                            tracing::warn!(%error, "installed plugin runtime refresh failed");
                            return Err(error.message);
                        }
                        Ok(created_result(result))
                    });
                    task.await.map_err(|error| error.to_string())?
                })
            }));
            api.register_tool(tool)?;
        }
        Ok(())
    });
    InlineExtension::Named {
        factory,
        name: "builtin:pisper-local-plugins".into(),
        hidden: false,
    }
}

pub(crate) fn created_result(result: Value) -> Value {
    let names = result["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    json!({"content":[{"type":"text","text":format!("Created and installed plugin \"{}\" ({}@{}).\nSource: {}\nTools: {}\nThe tools will be available to all projects on the next Agent turn through discover_tools and call_tool.", result["name"].as_str().unwrap_or(""),result["id"].as_str().unwrap_or(""),result["version"].as_str().unwrap_or(""),result["sourcePath"].as_str().unwrap_or(""),names)}],"details":result})
}
fn create_schema() -> Value {
    json!({"type":"object","properties":{
        "id":{"type":"string","minLength":1,"maxLength":96,"pattern":"^[a-z0-9](?:[a-z0-9.-]{0,94}[a-z0-9])?$"},
        "name":{"type":"string","minLength":1,"maxLength":100},
        "version":{"type":"string","pattern":"^\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?$"},
        "description":{"type":"string","maxLength":1000},
        "permissions":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":100}},
        "tools":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"object","properties":{
            "name":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[a-z][a-z0-9_]{0,63}$"},
            "label":{"type":"string","minLength":1,"maxLength":100},"description":{"type":"string","minLength":1,"maxLength":1000},
            "scope":{"type":"string","minLength":1,"maxLength":500},"parameters":{"type":"object","additionalProperties":true}},"required":["name","description","parameters"]}},
        "entryCode":{"type":"string","minLength":1,"maxLength":2000000},
        "files":{"type":"array","maxItems":64,"items":{"type":"object","properties":{"path":{"type":"string","minLength":1,"maxLength":500},"content":{"type":"string","maxLength":2000000}},"required":["path","content"]}}
    },"required":["id","name","tools","entryCode"]})
}
