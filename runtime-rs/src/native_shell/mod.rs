//! Pisper's host bash policy around the native Pi shell executor.
//! Registered as the SDK `bash` definition, so native reload, streaming,
//! timeout, cancellation, output spilling and process ownership stay in Pi.
mod environment;
mod guard;
#[cfg(test)]
mod tests;

#[cfg(test)]
use pi_rust::coding_agent::core::tools::bash::BashSpawnContext;
use pi_rust::coding_agent::{
    core::tools::{
        bash::{create_bash_tool_definition, BashSpawnHook, BashToolOptions},
        bash_process::{create_local_shell_operations, ShellOperations},
    },
    extensions::types::ToolDefinition,
    utils::shell_config::{get_shell_config, ShellConfig, ShellEnvironment},
};
use serde_json::json;
use std::sync::Arc;

pub(crate) use environment::host_command_environment;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Other,
}
impl Platform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Native resolve_spawn_context applies the configured command prefix before
/// this hook. Approval remains the separate Agent before-tool-call hook.
fn host_spawn_hook(platform: Platform, previous: Option<BashSpawnHook>) -> BashSpawnHook {
    Arc::new(move |context| {
        let mut context = match &previous {
            Some(previous) => previous(context)?,
            None => context,
        };
        if let Some(decision) =
            guard::guard_command(&context.command, platform == Platform::Windows)
        {
            if decision.block {
                return Err(guard::format_guard_error(&decision, &context.command));
            }
        }
        context.env = host_command_environment(context.env, platform);
        if platform == Platform::Windows {
            for (name, value) in [
                ("PYTHONIOENCODING", "utf-8"),
                ("PYTHONUTF8", "1"),
                ("LANG", "C.UTF-8"),
                ("LC_ALL", "C.UTF-8"),
            ] {
                context
                    .env
                    .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
                context.env.push((name.into(), value.into()));
            }
        }
        Ok(context)
    })
}

fn windows_system_shell(environment: &ShellEnvironment) -> ShellConfig {
    let root = ["SystemRoot", "windir"]
        .into_iter()
        .find_map(|name| {
            environment
                .iter()
                .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.is_empty())
                .map(|(_, value)| value.as_str())
        })
        .unwrap_or(r"C:\Windows");
    ShellConfig {
        shell: format!(
            r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
            root.trim_end_matches(['\\', '/'])
        ),
        args: ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        command_transport: None,
    }
}

fn windows_system_operations(environment: &ShellEnvironment) -> ShellOperations {
    let config = windows_system_shell(environment);
    create_local_shell_operations(
        "PowerShell",
        Arc::new(move || {
            let config = config.clone();
            Box::pin(async move { Ok(config) })
        }),
    )
}

fn definition(
    cwd: &str,
    mut options: BashToolOptions,
    platform: Platform,
    fallback: bool,
    environment: &ShellEnvironment,
) -> Arc<ToolDefinition> {
    if fallback {
        options.operations = Some(windows_system_operations(environment));
    }
    options.spawn_hook = Some(host_spawn_hook(platform, options.spawn_hook.take()));
    let mut tool = create_bash_tool_definition(cwd, options).as_ref().clone();
    if fallback {
        tool.description = tool
            .description
            .replace("Execute a bash command", "Execute a command");
        tool.description.push_str("\nBash was not found on Windows, so commands run with the system Windows PowerShell (powershell.exe).");
        tool.prompt_snippet = Some("Execute commands with Windows PowerShell".into());
    } else {
        tool.description.push_str("\nCommands run as the current operating-system user and can access host files and networks.");
    }
    tool.parameters = json!({"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout":{"type":"number","description":"Timeout in seconds"}},"required":["command"]});
    Arc::new(tool)
}

/// Extend CreateAgentSessionOptions.custom_tools with this result for every
/// runtime creation. SDK custom tools replace the same-name built-in and
/// survive AgentSession reload. This only exposes the release `bash` tool.
pub(crate) async fn tools(cwd: &str, options: BashToolOptions) -> Vec<Arc<ToolDefinition>> {
    let platform = Platform::current();
    let fallback = platform == Platform::Windows
        && options.operations.is_none()
        && get_shell_config(options.shell_path.as_deref())
            .await
            .is_err();
    vec![definition(
        cwd,
        options,
        platform,
        fallback,
        &std::env::vars().collect(),
    )]
}

/// Useful for controlled process fixtures without modifying global environment.
#[cfg(test)]
fn prepare(
    command: &str,
    cwd: &str,
    options: &BashToolOptions,
    environment: ShellEnvironment,
    platform: Platform,
) -> Result<BashSpawnContext, String> {
    let mut options = options.clone();
    options.expose_session_environment = Some(false);
    options.spawn_hook = Some(host_spawn_hook(platform, options.spawn_hook.take()));
    pi_rust::coding_agent::core::tools::bash::resolve_spawn_context(
        command,
        cwd,
        &options,
        environment,
        None,
    )
}
