//! Upstream definition-first base registry and native async AgentTool overrides.
//!
//! `read`, `edit`, and `write` use the native coding-agent tool implementations,
//! with full metadata and filesystem execution. `bash` and `powershell` use
//! native streaming shell execution. `grep`, `find`, and `ls` use native
//! search/directory operations. All eight built-ins await native operations
//! and forward cancellation/updates without blocking the runtime thread.
//! Tool TUI renderers remain a separate migration surface.

use std::sync::Arc;

use crate::agent_core::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use crate::coding_agent::extensions::types::{
    AbortSignal, AgentToolResultValue, AgentToolUpdateCallbackValue, PrepareArgumentsShim,
    ToolDefinition, ToolExposure,
};

/// Upstream `ToolName` (`tools/index.ts:180`): the eight built-in tool names
/// in definition order.
pub const TOOL_NAMES: &[&str] = &[
    "read",
    "bash",
    "powershell",
    "edit",
    "write",
    "grep",
    "find",
    "ls",
];

/// Upstream `createToolDefinitionFromAgentTool`
/// (`tools/tool-definition-wrapper.ts:31-42`): synthesize a minimal
/// ToolDefinition from an AgentTool so the registry stays definition-first
/// even for plain overrides without prompt metadata or renderers.
pub fn create_tool_definition_from_agent_tool(tool: &Arc<AgentTool>) -> Arc<ToolDefinition> {
    let tool = Arc::clone(tool);
    let prepare: Option<PrepareArgumentsShim> = tool.prepare_arguments.clone().map(|prepare| {
        Arc::new(move |value: &serde_json::Value| prepare(value.clone())) as PrepareArgumentsShim
    });
    Arc::new(ToolDefinition {
        name: tool.name.clone(),
        label: tool.label.clone(),
        description: tool.description.clone(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: tool.parameters.clone(),
        constrained_sampling: tool
            .constrained_sampling
            .as_ref()
            .and_then(|sampling| serde_json::to_value(sampling).ok()),
        render_shell: None,
        prepare_arguments: prepare,
        execution_mode: tool.execution_mode.map(|mode| match mode {
            crate::agent_core::types::ToolExecutionMode::Sequential => "sequential".to_string(),
            crate::agent_core::types::ToolExecutionMode::Parallel => "parallel".to_string(),
        }),
        output_schema: None,
        exposure: ToolExposure::Direct,
        namespace: None,
        annotations: None,
        default_active: None,
        prepare_loadout: None,
        execute: None,
        execute_async: Some(Arc::new(move |id, params, signal, on_update, _ctx| {
            let tool = tool.clone();
            Box::pin(async move {
                run_agent_tool(&tool, &id, &params, signal.as_ref(), on_update.as_ref()).await
            })
        })),
    })
}

/// Await an [`AgentTool`], forwarding abort and updates for the lifetime of
/// this execution only.
async fn run_agent_tool(
    tool: &Arc<AgentTool>,
    tool_call_id: &str,
    params: &serde_json::Value,
    signal: Option<&Arc<AbortSignal>>,
    on_update: Option<&AgentToolUpdateCallbackValue>,
) -> Result<AgentToolResultValue, String> {
    // Forward the extension AbortSignal onto the loop-side token the tool
    // receives (the reverse of extensions::wrapper's cancellation bridge).
    let token = tokio_util::sync::CancellationToken::new();
    let _subscription = signal.map(|abort_signal| {
        let token = token.clone();
        abort_signal.on_abort(Arc::new(move || token.cancel()))
    });
    let token = if signal.is_some() { Some(token) } else { None };
    let forward_update: Option<Arc<AgentToolUpdateCallback>> = on_update.map(|value| {
        let value = Arc::clone(value);
        Arc::new(move |result: &AgentToolResult| {
            if let Ok(json) = serde_json::to_value(result) {
                value(&json);
            }
        }) as Arc<AgentToolUpdateCallback>
    });
    let future = (tool.execute)(
        tool_call_id.to_string(),
        params.clone(),
        token,
        forward_update,
    );
    future
        .await
        .map(|result| serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
        .map_err(|error| error.to_string())
}

/// The eight native built-ins in upstream registry order.
pub fn create_all_tool_definitions(
    cwd: &str,
    auto_resize_images: bool,
) -> Vec<Arc<ToolDefinition>> {
    create_all_tool_definitions_with_shell_options(cwd, auto_resize_images, Default::default())
}
pub fn create_all_tool_definitions_with_shell_options(
    cwd: &str,
    auto_resize_images: bool,
    shell_options: crate::coding_agent::core::tools::bash::BashToolOptions,
) -> Vec<Arc<ToolDefinition>> {
    use crate::coding_agent::core::tools::{bash, edit, find, grep, ls, powershell, read, write};
    TOOL_NAMES
        .iter()
        .map(|name| match *name {
            "bash" => bash::create_bash_tool_definition(cwd, shell_options.clone()),
            "powershell" => powershell::create_powershell_tool_definition(cwd, Default::default()),
            "read" => read::create_read_tool_definition(
                cwd,
                read::ReadToolOptions {
                    auto_resize_images: Some(auto_resize_images),
                    ..Default::default()
                },
            ),
            "write" => write::create_write_tool_definition(cwd, Default::default()),
            "edit" => edit::create_edit_tool_definition(cwd, Default::default()),
            "find" => find::create_find_tool_definition(cwd, Default::default()),
            "grep" => grep::create_grep_tool_definition(cwd, Default::default()),
            "ls" => ls::create_ls_tool_definition(cwd, Default::default()),
            _ => unreachable!("unknown built-in tool in static registry"),
        })
        .collect()
}
