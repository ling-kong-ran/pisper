//! PowerShell configuration of the shared shell tool (upstream powershell.ts).
use super::bash::{
    create_shell_tool_definition, BashSpawnHook, BashToolOptions, ShellToolConfig,
    SHELL_PROMPT_GUIDELINE,
};
use super::bash_process::{create_local_powershell_operations, ShellOperations};
use crate::coding_agent::extensions::types::ToolDefinition;
use std::sync::Arc;
#[derive(Clone, Default)]
pub struct PowerShellToolOptions {
    pub operations: Option<ShellOperations>,
    pub expose_session_environment: Option<bool>,
    pub spawn_hook: Option<BashSpawnHook>,
}
pub fn create_powershell_tool_definition(
    cwd: &str,
    options: PowerShellToolOptions,
) -> Arc<ToolDefinition> {
    create_shell_tool_definition(
        cwd,
        ShellToolConfig {
            name: "powershell".into(),
            label: "powershell".into(),
            shell_name: "PowerShell".into(),
            prompt: "PS>".into(),
            prompt_snippet: "Execute PowerShell commands".into(),
            prompt_guidelines: vec![SHELL_PROMPT_GUIDELINE.into()],
            temp_file_prefix: "pi-powershell".into(),
        },
        BashToolOptions {
            operations: Some(
                options
                    .operations
                    .unwrap_or_else(create_local_powershell_operations),
            ),
            expose_session_environment: options.expose_session_environment,
            spawn_hook: options.spawn_hook,
            ..Default::default()
        },
    )
}
