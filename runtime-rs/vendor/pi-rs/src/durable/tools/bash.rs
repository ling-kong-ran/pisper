//! Port of `src/tools/bash.ts`: runs a command through the environment's
//! shell. Its output streams to `api.output()`, where the Harness keeps the
//! tail within the tool's output limits; the result content is that retained
//! output. Output beyond the limits is spilled to a file whose path is
//! reported as a diagnostic. A nonzero exit or timeout throws, which makes an
//! error result that still carries the output and diagnostics.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::json;

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::Tool as AiTool;
use crate::durable::env::{ExecutionErrorCode, OnOutput, ShellExecOptions, ShellSpillOptions};
use crate::durable::errors::PlainError;
use crate::durable::harness::types::{
    OutputLimitsSpec, OutputRetain, ToolExecutionApiLike, ToolExecutionResult, ToolRegistration,
};
use crate::durable::tools::env::require_env;
use crate::durable::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;

/// `BashToolInput` (`tools/bash.ts`). Input reaching `execute` was validated
/// against the tool declaration; schema violations surface as the port's
/// plain error (D32).
#[derive(Debug, Clone, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}

/// `BashExecution` (`tools/bash.ts`); `env` mirrors the empty upstream
/// `Record<string, string>` that a prepare hook may fill.
#[derive(Debug, Clone)]
pub struct BashExecution {
    pub command: String,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub inherit_env: bool,
}

/// `BashPrepare` (`tools/bash.ts`): `(execution, api, context)`, may mutate
/// the execution.
pub type BashPrepareFuture = Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + 'static>>;
pub type BashPrepare = Arc<
    dyn Fn(&mut BashExecution, Arc<dyn ToolExecutionApiLike>, Context) -> BashPrepareFuture
        + Send
        + Sync,
>;

/// `BashToolOptions` (`tools/bash.ts`).
#[derive(Clone, Default)]
pub struct BashToolOptions {
    pub command_prefix: Option<String>,
    pub prepare: Option<BashPrepare>,
}

/// `validateTimeout(timeout)` (`tools/bash.ts`).
fn validate_timeout(timeout: Option<f64>) -> Result<(), PlainError> {
    let Some(timeout) = timeout else {
        return Ok(());
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(PlainError::new(
            "Invalid timeout: must be a finite number of seconds",
        ));
    }
    if timeout > MAX_TIMEOUT_SECONDS {
        return Err(PlainError::new(format!(
            "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
        )));
    }
    Ok(())
}

/// `createBashTool(options?)` (`tools/bash.ts`).
pub fn create_bash_tool(options: BashToolOptions) -> ToolRegistration {
    ToolRegistration {
        tool: AiTool {
            name: String::from("bash"),
            description: format!(
                "Execute a bash command in the current working directory. Returns combined stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.",
                DEFAULT_MAX_BYTES / 1024
            ),
            parameters: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {"type": "string", "description": "Bash command to execute"},
                    "timeout": {"type": "number", "description": "Timeout in seconds (optional, no default timeout)"},
                },
            }),
            constrained_sampling: None,
        },
        replay: None,
        execution_mode: None,
        prepare_arguments: None,
        output_limits: Some(OutputLimitsSpec {
            max_bytes: None,
            max_lines: None,
            retain: Some(OutputRetain::Tail),
        }),
        execute: Arc::new(
            move |args: serde_json::Map<String, serde_json::Value>,
                  api: Arc<dyn ToolExecutionApiLike>,
                  context: Context| {
                let options = options.clone();
                Box::pin(async move {
                    let input: BashToolInput =
                        serde_json::from_value(serde_json::Value::Object(args))
                            .map_err(|error| PlainError::new(error.to_string()))?;
                    validate_timeout(input.timeout)?;
                    let env = require_env(api.as_ref())?;
                    let mut execution = BashExecution {
                        command: match options.command_prefix.as_deref() {
                            // The upstream template predicate is JS truthiness.
                            Some(prefix) if !prefix.is_empty() => {
                                format!("{prefix}\n{}", input.command)
                            }
                            _ => input.command.clone(),
                        },
                        cwd: env.cwd().to_string(),
                        env: Vec::new(),
                        inherit_env: true,
                    };
                    if let Some(prepare) = &options.prepare {
                        prepare(&mut execution, Arc::clone(&api), context.clone()).await?;
                    }
                    let api_for_output = Arc::clone(&api);
                    let on_output: OnOutput = Arc::new(Mutex::new(move |text: &str| {
                        // An `api.output` rejection after the call settled is
                        // dropped: the callback cannot propagate it (D34).
                        let _ = api_for_output.output(text.as_bytes());
                    }));
                    let result = env.exec(
                        &execution.command,
                        Some(ShellExecOptions {
                            cwd: Some(execution.cwd),
                            env: Some(execution.env.clone()),
                            inherit_env: Some(execution.inherit_env),
                            timeout: input.timeout,
                            on_output: Some(on_output),
                            spill: Some(ShellSpillOptions {
                                after_bytes: DEFAULT_MAX_BYTES,
                                after_lines: DEFAULT_MAX_LINES,
                            }),
                        }),
                        &context,
                    );
                    let spill_path = match &result {
                        Ok(value) => value.spill_path.clone(),
                        Err(error) => error.spill_path.clone(),
                    };
                    if let Some(spill_path) = spill_path {
                        api.diagnostic(crate::durable::harness::types::ToolDiagnostic {
                            severity: crate::durable::harness::types::ToolDiagnosticSeverity::Info,
                            code: Some(String::from("full_output")),
                            message: format!("Full output: {spill_path}"),
                        })?;
                    }
                    match result {
                        Err(error) => {
                            if error.code == ExecutionErrorCode::Aborted
                                && context
                                    .abort_signal()
                                    .is_some_and(|signal| signal.is_cancelled())
                            {
                                return Err(PlainError::new(error.message));
                            }
                            if error.code == ExecutionErrorCode::Timeout {
                                return Err(PlainError::new(format!(
                                    "Command timed out after {} seconds",
                                    input.timeout.unwrap_or_default()
                                )));
                            }
                            if error.code == ExecutionErrorCode::Aborted {
                                return Err(PlainError::new("Command aborted"));
                            }
                            Err(PlainError::new(error.message))
                        }
                        Ok(value) if value.exit_code != 0 => Err(PlainError::new(format!(
                            "Command exited with code {}",
                            value.exit_code
                        ))),
                        Ok(_) => Ok(ToolExecutionResult::default()),
                    }
                })
            },
        ),
    }
}
