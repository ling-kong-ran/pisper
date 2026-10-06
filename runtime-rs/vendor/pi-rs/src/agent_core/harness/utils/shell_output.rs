//! Port of `packages/agent/src/harness/utils/shell-output.ts` (112 lines):
//! the compatibility collector for callers that need one bounded final view.
//! Source-side capture, adaptive publication, and spilling remain owned by
//! the execution environment. Ported with the env (M3b Task 6) because the
//! `nodejs-env` oracle test drives it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::output_capture::apply_shell_output_update;
use super::truncate::{truncate_tail, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::types::{
    ExecutionEnv, ExecutionError, ExecutionErrorCode, ShellExecOptions, ShellOutputCaptureOptions,
    ShellOutputLimits, ShellOutputMetadata, ShellOutputRetention, ShellOutputUpdate,
    ShellOutputView,
};

/// Upstream `ShellCaptureProgress` (`shell-output.ts:14-19`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellCaptureProgress {
    pub output: String,
    pub truncation: TruncationResult,
    /// Upstream `fullOutputPath` (the capture's spill path).
    pub full_output_path: Option<String>,
    pub last_line_bytes: u64,
}

/// Upstream `ShellCaptureOptions` (`shell-output.ts:21-25`): the
/// `ShellExecOptions` fields minus the capture plumbing, plus the chunk
/// callback. The upstream `getProgress()` lazily-evaluated argument becomes a
/// closure with the same shape.
pub type OnChunkFn = dyn Fn(&str, &dyn Fn() -> ShellCaptureProgress, &Context) + Send + Sync;

#[derive(Default)]
pub struct ShellCaptureOptions {
    pub cwd: Option<String>,
    pub env: Option<BTreeMap<String, String>>,
    pub inherit_env: Option<bool>,
    pub timeout: Option<f64>,
    pub on_chunk: Option<Arc<OnChunkFn>>,
    /// Return shell execution failures with captured output instead of as a
    /// failed `Result` (`shell-output.ts:23-24`).
    pub return_execution_errors: bool,
}

/// Upstream `ShellCaptureResult` (`shell-output.ts:27-32`). Runtime result
/// (carries the error value), so no serde/derive plumbing.
pub struct ShellCaptureResult {
    pub output: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
    pub last_line_bytes: u64,
    /// `None` on cancellation (upstream `exitCode: undefined`).
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub execution_error: Option<ExecutionError>,
}

/// Upstream `progressFrom` (`shell-output.ts:34-41`).
fn progress_from(output: ShellOutputView) -> ShellCaptureProgress {
    ShellCaptureProgress {
        truncation: TruncationResult::from_metadata(&output.text, &output.metadata.truncation),
        output: output.text.clone(),
        full_output_path: output.metadata.spill_path.clone(),
        last_line_bytes: output.metadata.last_line_bytes.unwrap_or(0),
    }
}

/// Upstream `executeShellWithCapture` (`shell-output.ts:48-110`).
pub fn execute_shell_with_capture<'a>(
    env: &'a dyn ExecutionEnv,
    command: &str,
    options: Option<ShellCaptureOptions>,
    context: Context,
) -> BoxFuture<'a, anyhow::Result<ShellCaptureResult>> {
    let command = command.to_string();
    Box::pin(async move {
        let options = options.unwrap_or_default();
        let on_chunk = options.on_chunk.clone();
        // The update callback lives inside `exec_options` and outlives the
        // exec call, so the view is shared instead of captured by move.
        let output: Arc<Mutex<Option<ShellOutputView>>> = Arc::new(Mutex::new(None));
        let on_update_output = Arc::clone(&output);
        let exec_options = ShellExecOptions {
            cwd: options.cwd.clone(),
            env: options.env.clone(),
            inherit_env: options.inherit_env,
            timeout: options.timeout,
            capture: Some(ShellOutputCaptureOptions {
                limits: ShellOutputLimits {
                    max_bytes: DEFAULT_MAX_BYTES,
                    max_lines: DEFAULT_MAX_LINES,
                    retain: Some(ShellOutputRetention::Tail),
                },
                spill: Some(true),
            }),
            on_update: Some(Arc::new(
                move |update: ShellOutputUpdate, update_context: &Context| {
                    let previous = on_update_output.lock().unwrap().take();
                    let next = apply_shell_output_update(previous.clone(), update.clone());
                    let chunk = match &update {
                        ShellOutputUpdate::Append { text, .. }
                        | ShellOutputUpdate::Slide { text, .. } => Some(text.clone()),
                        ShellOutputUpdate::Replace { .. } if previous.is_none() => {
                            Some(next.text.clone())
                        }
                        _ => None,
                    };
                    *on_update_output.lock().unwrap() = Some(next);
                    // A metadata-only update and a post-cap replacement contain no
                    // new incremental chunk (shell-output.ts:75-78).
                    if let (Some(chunk), Some(on_chunk)) = (chunk, &on_chunk) {
                        let get_progress_output = Arc::clone(&on_update_output);
                        let get_progress = move || {
                            progress_from(
                                get_progress_output
                                    .lock()
                                    .unwrap()
                                    .clone()
                                    .expect("output initialized"),
                            )
                        };
                        on_chunk(&chunk, &get_progress, update_context);
                    }
                },
            )),
        };

        let result = env
            .exec(&command, Some(&exec_options), context.clone())
            .await;

        let output_view = match output.lock().unwrap().take() {
            Some(view) => view,
            None => {
                // shell-output.ts:84-87: the empty-exec fallback view.
                let retained = truncate_tail("", Default::default());
                ShellOutputView {
                    metadata: ShellOutputMetadata {
                        truncation: retained.truncation_metadata(),
                        spill_path: None,
                        last_line_bytes: None,
                    },
                    text: String::new(),
                }
            }
        };
        let progress = progress_from(output_view);
        let progress_truncated = progress.truncation.truncated;
        match result {
            Err(error) => {
                let aborted = error.code == ExecutionErrorCode::Aborted
                    || context
                        .abort_signal()
                        .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    Ok(ShellCaptureResult {
                        output: progress.output,
                        truncation: progress.truncation,
                        full_output_path: progress.full_output_path,
                        last_line_bytes: progress.last_line_bytes,
                        exit_code: None,
                        cancelled: true,
                        truncated: progress_truncated,
                        execution_error: None,
                    })
                } else if options.return_execution_errors {
                    Ok(ShellCaptureResult {
                        output: progress.output,
                        truncation: progress.truncation,
                        full_output_path: progress.full_output_path,
                        last_line_bytes: progress.last_line_bytes,
                        exit_code: None,
                        cancelled: false,
                        truncated: progress_truncated,
                        execution_error: Some(error),
                    })
                } else {
                    Err(anyhow::Error::new(error))
                }
            }
            Ok(result) => Ok(ShellCaptureResult {
                output: progress.output,
                truncation: progress.truncation,
                full_output_path: progress.full_output_path,
                last_line_bytes: progress.last_line_bytes,
                exit_code: Some(result.exit_code),
                cancelled: false,
                truncated: result.metadata.truncation.truncated,
                execution_error: None,
            }),
        }
    })
}

// Re-exported for callers matching upstream's
// `export { sanitizeShellOutput as sanitizeBinaryOutput }`
// (shell-output.ts:112).
pub use super::output_capture::sanitize_shell_output as sanitize_binary_output;
