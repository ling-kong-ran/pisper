//! Native coding-agent shell tools: session environment, spawn hooks, streaming
//! bounded output, throttled updates and upstream status/truncation messages.
//! Tool renderers remain an interactive-host seam; process and wire behavior
//! are implemented here rather than falling back to harness tool semantics.
use super::bash_process::{create_local_bash_operations, ShellExecOptions, ShellOperations};
use super::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot};
use super::truncate::{self, format_size, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use super::{definition, ToolFuture};
use crate::agent_core::harness::types::TruncatedBy;
use crate::coding_agent::{
    extensions::types::{
        AbortSignal, AgentToolUpdateCallbackValue, ExtensionContext, ToolDefinition,
    },
    session_manager::SessionManager,
    utils::shell_config::{get_shell_env, ShellEnvironment},
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

pub const BASH_UPDATE_THROTTLE_MS: u64 = 100;
pub const BASH_PROMPT_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
pub const SHELL_PROMPT_GUIDELINE: &str =
    "You can inspect PI_* environment variables for current model and session details.";
const SESSION_KEYS: [&str; 5] = [
    "PI_SESSION_ID",
    "PI_SESSION_FILE",
    "PI_PROVIDER",
    "PI_MODEL",
    "PI_REASONING_LEVEL",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: String,
    pub env: ShellEnvironment,
}
pub type BashSpawnHook =
    Arc<dyn Fn(BashSpawnContext) -> Result<BashSpawnContext, String> + Send + Sync>;
#[derive(Clone, Default)]
pub struct BashToolOptions {
    pub operations: Option<ShellOperations>,
    pub command_prefix: Option<String>,
    pub shell_path: Option<String>,
    pub expose_session_environment: Option<bool>,
    pub spawn_hook: Option<BashSpawnHook>,
}
#[derive(Clone, Debug)]
pub struct ShellToolConfig {
    pub name: String,
    pub label: String,
    pub shell_name: String,
    pub prompt: String,
    pub prompt_snippet: String,
    pub prompt_guidelines: Vec<String>,
    pub temp_file_prefix: String,
}
impl ShellToolConfig {
    pub fn bash() -> Self {
        Self {
            name: "bash".into(),
            label: "bash".into(),
            shell_name: "bash".into(),
            prompt: "$".into(),
            prompt_snippet: BASH_PROMPT_SNIPPET.into(),
            prompt_guidelines: vec![SHELL_PROMPT_GUIDELINE.into()],
            temp_file_prefix: "pi-bash".into(),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct ShellSessionMetadata {
    pub session_id: String,
    pub session_file: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_level: Option<String>,
}
/// Read current session/model for each execution, not tool registration. The
/// native session manager is the same typed handle the extension runner uses.
fn session_metadata(ctx: &ExtensionContext) -> Result<ShellSessionMetadata, String> {
    let handle = ctx.session_manager()?;
    let session = handle
        .downcast_ref::<Mutex<SessionManager>>()
        .ok_or("Shell environment requires a native SessionManager handle")?;
    let (session_id, session_file) = {
        let session = session.lock().map_err(|_| "SessionManager lock poisoned")?;
        (
            session.get_session_id().to_owned(),
            session.get_session_file().map(str::to_owned),
        )
    };
    let model = ctx.model()?;
    let level = serde_json::to_value(ctx.thinking_level()?).map_err(|e| e.to_string())?;
    Ok(ShellSessionMetadata {
        session_id,
        session_file,
        provider: model
            .as_ref()
            .and_then(|m| m["provider"].as_str())
            .map(str::to_owned),
        model: model
            .as_ref()
            .and_then(|m| m["id"].as_str())
            .map(str::to_owned),
        reasoning_level: level.as_str().filter(|s| !s.is_empty()).map(str::to_owned),
    })
}
pub fn resolve_spawn_context(
    command: &str,
    cwd: &str,
    options: &BashToolOptions,
    mut environment: ShellEnvironment,
    metadata: Option<&ShellSessionMetadata>,
) -> Result<BashSpawnContext, String> {
    environment.retain(|(key, _)| !SESSION_KEYS.contains(&key.as_str()));
    if options.expose_session_environment.unwrap_or(true) {
        if let Some(meta) = metadata {
            environment.push(("PI_SESSION_ID".into(), meta.session_id.clone()));
            for (key, value) in [
                ("PI_SESSION_FILE", &meta.session_file),
                ("PI_PROVIDER", &meta.provider),
                ("PI_MODEL", &meta.model),
                ("PI_REASONING_LEVEL", &meta.reasoning_level),
            ] {
                if let Some(value) = value
                    .as_ref()
                    .filter(|s| matches!(key, "PI_PROVIDER" | "PI_MODEL") || !s.is_empty())
                {
                    environment.push((key.into(), value.clone()));
                }
            }
        }
    }
    let command = match options.command_prefix.as_deref().filter(|p| !p.is_empty()) {
        Some(prefix) => format!("{prefix}\n{command}"),
        None => command.to_owned(),
    };
    let context = BashSpawnContext {
        command,
        cwd: cwd.to_owned(),
        env: environment,
    };
    match &options.spawn_hook {
        Some(hook) => hook(context),
        None => Ok(context),
    }
}
pub fn create_bash_tool_definition(cwd: &str, options: BashToolOptions) -> Arc<ToolDefinition> {
    create_shell_tool_definition(cwd, ShellToolConfig::bash(), options)
}
pub fn create_shell_tool_definition(
    cwd: &str,
    config: ShellToolConfig,
    options: BashToolOptions,
) -> Arc<ToolDefinition> {
    let ops = options
        .operations
        .clone()
        .unwrap_or_else(|| create_local_bash_operations(options.shell_path.clone()));
    let description = format!("Execute a {} command in the current working directory. Returns stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.", config.shell_name, DEFAULT_MAX_BYTES/1024);
    let guidelines = if options.expose_session_environment.unwrap_or(true) {
        config
            .prompt_guidelines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut def = definition(
        &config.name,
        &description,
        &config.prompt_snippet,
        &guidelines,
        json!({"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}},"required":["command"]}),
        true,
    );
    def.label = config.label.clone();
    let cwd = cwd.to_owned();
    def.execute_async = Some(Arc::new(move |_, params, signal, update, ctx| {
        let cwd = cwd.clone();
        let ops = ops.clone();
        let options = options.clone();
        let config = config.clone();
        Box::pin(async move {
            let command = params["command"]
                .as_str()
                .ok_or("command must be a string")?;
            let timeout = params
                .get("timeout")
                .map(|value| value.as_f64().ok_or("timeout must be a number"))
                .transpose()?;
            let cwd = super::context_cwd(&ctx, &cwd)?;
            let metadata = if options.expose_session_environment.unwrap_or(true) {
                Some(session_metadata(&ctx)?)
            } else {
                None
            };
            let spawn =
                resolve_spawn_context(command, &cwd, &options, get_shell_env(), metadata.as_ref())?;
            let output = OutputAccumulator::new(OutputAccumulatorOptions {
                temp_file_prefix: Some(config.temp_file_prefix.clone()),
                ..Default::default()
            });
            execute_shell_tool(spawn, timeout, &ops, output, signal, update).await
        })
    }));
    Arc::new(def)
}
struct OutputState {
    output: OutputAccumulator,
    accepting: bool,
    dirty: bool,
    last_update: Option<Instant>,
    deadline: Option<Instant>,
}
// Remote operations may retain on_data after their execution future is dropped.
// Revoke their output sink as well as dropping the local timer/child future.
struct OutputLifetime(Arc<Mutex<OutputState>>);
impl Drop for OutputLifetime {
    fn drop(&mut self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting = false;
        state.dirty = false;
        state.deadline = None;
        let _ = state.output.close_temp_file();
    }
}
impl OutputState {
    fn take_update(&mut self) -> Result<Option<Value>, String> {
        if !self.dirty {
            return Ok(None);
        }
        self.dirty = false;
        self.last_update = Some(Instant::now());
        self.deadline = None;
        let snapshot = self.output.snapshot(true).map_err(|e| e.to_string())?;
        Ok(Some(
            json!({"content":[{"type":"text","text":snapshot.content}],"details":snapshot_details(&snapshot)}),
        ))
    }
}
fn snapshot_details(snapshot: &OutputSnapshot) -> Value {
    let mut details = json!({});
    if snapshot.truncation.truncated {
        details["truncation"] = truncate::as_value(&snapshot.truncation);
    }
    if let Some(path) = &snapshot.full_output_path {
        details["fullOutputPath"] = json!(path);
    }
    details
}
fn format_output(
    snapshot: &OutputSnapshot,
    last_line_bytes: u64,
    empty: &str,
) -> (String, Option<Value>) {
    let t = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty.to_owned()
    } else {
        snapshot.content.clone()
    };
    if !t.truncated {
        return (text, None);
    }
    let path = snapshot
        .full_output_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "undefined".into());
    let start = t.total_lines - t.output_lines + 1;
    if t.last_line_partial {
        text += &format!(
            "\n\n[Showing last {} of line {} (line is {}). Full output: {path}]",
            format_size(t.output_bytes),
            t.total_lines,
            format_size(last_line_bytes)
        );
    } else if t.truncated_by == Some(TruncatedBy::Lines) {
        text += &format!(
            "\n\n[Showing lines {start}-{} of {}. Full output: {path}]",
            t.total_lines, t.total_lines
        );
    } else {
        text += &format!(
            "\n\n[Showing lines {start}-{} of {} ({} limit). Full output: {path}]",
            t.total_lines,
            t.total_lines,
            format_size(DEFAULT_MAX_BYTES)
        );
    }
    (text, Some(snapshot_details(snapshot)))
}
fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.into()
    } else {
        format!("{text}\n\n{status}")
    }
}
/// The timer is polled with the exec future, never spawned in the background.
/// Dropping this future therefore cancels pending updates and the native
/// backend's child lifetime guard kills its process tree.
pub async fn execute_shell_tool(
    context: BashSpawnContext,
    timeout: Option<f64>,
    ops: &ShellOperations,
    output: OutputAccumulator,
    signal: Option<Arc<AbortSignal>>,
    update: Option<AgentToolUpdateCallbackValue>,
) -> Result<Value, String> {
    let state = Arc::new(Mutex::new(OutputState {
        output,
        accepting: true,
        dirty: false,
        last_update: None,
        deadline: None,
    }));
    let _lifetime = OutputLifetime(state.clone());
    let changed = Arc::new(Notify::new());
    if let Some(update) = &update {
        update(&json!({"content":[]}));
    }
    let on_data = {
        let state = state.clone();
        let changed = changed.clone();
        let update = update.clone();
        Arc::new(move |data: &[u8]| {
            let value = {
                let mut state = state.lock().map_err(|_| "Shell output lock poisoned")?;
                if !state.accepting {
                    return Ok(());
                }
                state.output.append(data).map_err(|e| e.to_string())?;
                if update.is_none() {
                    return Ok(());
                }
                state.dirty = true;
                let deadline = state
                    .last_update
                    .map(|last| last + Duration::from_millis(BASH_UPDATE_THROTTLE_MS));
                if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
                    state.take_update()?
                } else {
                    state.deadline = deadline;
                    None
                }
            };
            changed.notify_one();
            if let (Some(update), Some(value)) = (&update, value) {
                update(&value);
            }
            Ok(())
        })
    };
    let execution: ToolFuture<_> = (ops.exec)(
        context.command,
        context.cwd,
        ShellExecOptions {
            on_data,
            signal,
            timeout,
            env: Some(context.env),
        },
    );
    tokio::pin!(execution);
    let result = loop {
        let notified = changed.notified();
        let deadline = state
            .lock()
            .map_err(|_| "Shell output lock poisoned")?
            .deadline;
        tokio::select! {
            result=&mut execution=>break result,
            _=notified=>{},
            _=wait_for_update(deadline)=>{
                let value=state.lock().map_err(|_|"Shell output lock poisoned")?.take_update()?;
                if let (Some(update),Some(value))=(&update,value){update(&value);}
            }
        }
    };
    let (pending, snapshot, last_line) = {
        let mut state = state.lock().map_err(|_| "Shell output lock poisoned")?;
        state.accepting = false;
        state.output.finish().map_err(|e| e.to_string())?;
        state.deadline = None;
        let pending = state.take_update()?;
        let snapshot = state.output.snapshot(true).map_err(|e| e.to_string())?;
        let last_line = state.output.get_last_line_bytes();
        state.output.close_temp_file().map_err(|e| e.to_string())?;
        (pending, snapshot, last_line)
    };
    if let (Some(update), Some(value)) = (&update, pending) {
        update(&value);
    }
    let exit = match result {
        Ok(exit) => exit,
        Err(error) => {
            let (text, _) = format_output(&snapshot, last_line, "");
            if error == "aborted" {
                return Err(append_status(&text, "Command aborted"));
            }
            if error.starts_with("timeout:") {
                let seconds = error.split(':').nth(1).unwrap_or("");
                return Err(append_status(
                    &text,
                    &format!("Command timed out after {seconds} seconds"),
                ));
            }
            return Err(error);
        }
    };
    let (text, details) = format_output(&snapshot, last_line, "(no output)");
    match exit.exit_code {
        None => Err(append_status(
            &text,
            "Command terminated without an exit code",
        )),
        Some(code) if code != 0 => Err(append_status(
            &text,
            &format!("Command exited with code {code}"),
        )),
        Some(_) => {
            let mut result = json!({"content":[{"type":"text","text":text}]});
            if let Some(details) = details {
                result["details"] = details;
            }
            Ok(result)
        }
    }
}
async fn wait_for_update(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}
#[cfg(test)]
#[path = "bash_tests.rs"]
mod tests;
