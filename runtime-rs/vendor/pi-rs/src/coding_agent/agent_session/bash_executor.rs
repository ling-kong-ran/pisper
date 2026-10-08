//! Streaming user-shell execution from upstream `core/bash-executor.ts`.
//! AgentSession/RPC and model tools share the same native process backend.
//! Unlike model-tool OutputAccumulator, this executor sanitizes every decoded
//! chunk before spilling and deliberately does NOT flush TextDecoder at EOF.

use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::coding_agent::{
    core::tools::{
        bash_process::{self, ShellDataCallback, ShellExecOptions},
        output_accumulator::TextDecoder,
        truncate::{truncate_tail, TruncationOptions, DEFAULT_MAX_BYTES},
    },
    extensions::types::AbortSignal,
    utils::{ansi::strip_ansi, shell_config::sanitize_binary_output},
};

// Despite upstream's variable name `outputBytes`, the rolling buffer counts
// JavaScript string.length (UTF-16 code units), NOT UTF-8 bytes. Raw bytes alone
// decide when to start the sanitized spill file.
const MAX_OUTPUT_UNITS: usize = (DEFAULT_MAX_BYTES as usize) * 2;
pub type BashResult = crate::coding_agent::extensions::types::BashResult;
pub type OnChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct BashExecutorOptions {
    pub on_chunk: Option<OnChunkCallback>,
    pub signal: Option<CancellationToken>,
}

pub struct BashExecCallbacks {
    /// Compatibility adapter for existing native string-based extensions.
    /// New backends should use on_bytes so split/invalid UTF-8 is not lost.
    pub on_data: Arc<dyn Fn(&str) + Send + Sync>,
    /// Original transport bytes, with fallible output/spill handling.
    pub on_bytes: ShellDataCallback,
    pub signal: CancellationToken,
}
pub type OperationsExec = Arc<
    dyn Fn(String, String, BashExecCallbacks) -> BoxFuture<'static, Result<Option<i64>, String>>
        + Send
        + Sync,
>;
#[derive(Clone)]
pub struct BashOperationsHandle {
    pub exec: OperationsExec,
}

struct ExecutorState {
    decoder: TextDecoder,
    chunks: VecDeque<String>,
    output_units: usize,
    total_bytes: usize,
    temp_directory: PathBuf,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<File>,
    io_error: Option<String>,
    accepting: bool,
}
impl ExecutorState {
    fn new(temp_directory: &Path) -> Self {
        Self {
            decoder: TextDecoder::default(),
            chunks: VecDeque::new(),
            output_units: 0,
            total_bytes: 0,
            temp_directory: temp_directory.to_owned(),
            temp_file_path: None,
            temp_file: None,
            io_error: None,
            accepting: true,
        }
    }
    fn ensure_temp_file(&mut self) -> Result<(), String> {
        if self.temp_file_path.is_some() {
            return Ok(());
        }
        let bytes: [u8; 8] = rand::random();
        let id = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let path = self.temp_directory.join(format!("pi-bash-{id}.log"));
        let mut file = File::create(&path).map_err(|e| e.to_string())?;
        for chunk in &self.chunks {
            file.write_all(chunk.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
        Ok(())
    }
    fn append(&mut self, data: &[u8]) -> Result<String, String> {
        self.total_bytes += data.len();
        let text = sanitize_binary_output(&strip_ansi(&self.decoder.decode(data, false)))
            .replace('\r', "");
        if self.total_bytes > DEFAULT_MAX_BYTES as usize {
            self.ensure_temp_file()?;
        }
        if let Some(file) = &mut self.temp_file {
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        self.output_units += text.encode_utf16().count();
        self.chunks.push_back(text.clone());
        while self.output_units > MAX_OUTPUT_UNITS && self.chunks.len() > 1 {
            let removed = self.chunks.pop_front().expect("nonempty rolling buffer");
            self.output_units -= removed.encode_utf16().count();
        }
        Ok(text)
    }
}
// An extension may retain a callback after returning, or after the execution
// future is dropped. Such a callback must not keep appending to a closed run.
struct OutputLifetime(Arc<Mutex<ExecutorState>>);
impl Drop for OutputLifetime {
    fn drop(&mut self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting = false;
        state.temp_file.take();
    }
}

pub async fn execute_bash_with_operations(
    command: &str,
    cwd: &str,
    operations: BashOperationsHandle,
    options: BashExecutorOptions,
) -> Result<BashResult, String> {
    execute_with_temp_directory(command, cwd, operations, options, &std::env::temp_dir()).await
}

async fn execute_with_temp_directory(
    command: &str,
    cwd: &str,
    operations: BashOperationsHandle,
    options: BashExecutorOptions,
    temp_directory: &Path,
) -> Result<BashResult, String> {
    let state = Arc::new(Mutex::new(ExecutorState::new(temp_directory)));
    let _lifetime = OutputLifetime(state.clone());
    let on_bytes: ShellDataCallback = {
        let state = state.clone();
        let on_chunk = options.on_chunk;
        Arc::new(move |bytes| {
            let text = {
                let mut state = state.lock().expect("bash executor state");
                if !state.accepting {
                    return Ok(());
                }
                if let Some(error) = &state.io_error {
                    return Err(error.clone());
                }
                match state.append(bytes) {
                    Ok(text) => text,
                    Err(error) => {
                        state.io_error = Some(error.clone());
                        return Err(error);
                    }
                }
            };
            if let Some(on_chunk) = &on_chunk {
                on_chunk(&text);
            }
            Ok(())
        })
    };
    let callbacks = BashExecCallbacks {
        on_data: {
            let on_bytes = on_bytes.clone();
            // The legacy infallible callback records any IO error in state;
            // it is propagated after the remote operation returns.
            Arc::new(move |text| {
                let _ = on_bytes(text.as_bytes());
            })
        },
        on_bytes,
        signal: options.signal.clone().unwrap_or_default(),
    };
    let run = (operations.exec)(command.to_owned(), cwd.to_owned(), callbacks).await;
    let cancelled = options
        .signal
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled);
    let mut state = state.lock().expect("bash executor state");
    state.accepting = false;
    if let Some(error) = state.io_error.take() {
        return Err(error);
    }
    // Non-abort errors close an existing spill but do not perform a new
    // line-truncation spill (upstream's catch/rethrow branch).
    let exit_code = match run {
        Ok(code) if !cancelled => code,
        Ok(_) | Err(_) if cancelled => None,
        Err(error) => return Err(error),
        Ok(code) => code,
    };
    let full_output = state.chunks.iter().cloned().collect::<String>();
    let truncation = truncate_tail(&full_output, TruncationOptions::default());
    if truncation.truncated {
        state.ensure_temp_file()?;
    }
    if let Some(mut file) = state.temp_file.take() {
        file.flush().map_err(|e| e.to_string())?;
    }
    Ok(BashResult {
        output: if truncation.truncated {
            truncation.content
        } else {
            full_output
        },
        exit_code,
        cancelled,
        truncated: truncation.truncated,
        full_output_path: state
            .temp_file_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    })
}

/// Same local discovery/environment/stdio lifetime as model-facing bash. The
/// token adapter remains within this future (no detached cancellation task).
pub fn create_local_bash_operations(shell_path: Option<&str>) -> BashOperationsHandle {
    let operations = bash_process::create_local_bash_operations(shell_path.map(str::to_owned));
    BashOperationsHandle {
        exec: Arc::new(move |command, cwd, callbacks| {
            let operations = operations.clone();
            Box::pin(async move {
                let signal = Arc::new(AbortSignal::new());
                if callbacks.signal.is_cancelled() {
                    signal.abort();
                }
                let execution = (operations.exec)(
                    command,
                    cwd,
                    ShellExecOptions {
                        on_data: callbacks.on_bytes,
                        signal: Some(signal.clone()),
                        timeout: None,
                        env: None,
                    },
                );
                tokio::pin!(execution);
                let outcome = tokio::select! {
                    biased;
                    _ = callbacks.signal.cancelled() => {
                        signal.abort();
                        execution.await
                    },
                    result = &mut execution => result,
                };
                outcome.map(|result| result.exit_code.map(i64::from))
            })
        }),
    }
}

#[cfg(test)]
#[path = "bash_executor_tests.rs"]
mod tests;
