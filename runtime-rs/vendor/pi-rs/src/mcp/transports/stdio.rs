//! Stdio transport, ported from upstream
//! `packages/mcp/src/transports/stdio.ts`.
//!
//! Framing is NEWLINE-DELIMITED JSON in both directions: every message is
//! `JSON.stringify(message) + "\n"` (upstream `send`), and incoming lines are
//! split on `\n`, stripped of one trailing `\r`, skipped when blank, parsed
//! with the shared JSON-RPC predicates, and dispatched to the message
//! listeners. Oversized lines and oversized unterminated buffers emit
//! `"MCP stdio message exceeds N bytes"` and are dropped exactly like the
//! upstream `handleStdout`.
//!
//! Port notes (disclosed divergences):
//! - Upstream spawns via `cross-spawn` (which routes `.cmd` shims through
//!   `cmd.exe` on Windows); the port spawns the command directly through
//!   `tokio::process`. A `.cmd`/`.bat` command must therefore be spelled with
//!   its interpreter or resolved by the caller.
//! - Process groups: on Unix the child is spawned with `process_group(0)` and
//!   the group is signalled through `/bin/kill -- -pid` (upstream
//!   `detached: true` + `process.kill(-pid)`; the external kill binary keeps
//!   the port unsafe-free, same as the agent-core nodejs harness). On Windows
//!   shutdown uses `taskkill /pid <pid> /T /F` with `CREATE_NO_WINDOW`
//!   (upstream skips it once the child already exited, which the port's close
//!   path guarantees).
//! - Upstream registers a process `exit` hook that SIGTERMs the live process
//!   groups when the host dies without closing transports; Rust has no safe
//!   at-exit hook, so transports must be `close()`d explicitly (leaked
//!   children are reaped by their parent chain, not the port).
//! - Node reports spawn failures (e.g. ENOENT) through the child `error`
//!   event; `tokio::process::Command::spawn` returns them directly from
//!   `start()`. Post-spawn child errors do not exist in the Rust model (the
//!   exit status is observed by the watcher task).

// Only the Windows kill-tree helper needs `PathBuf`; the unix path shells
// out to the external `kill` binary instead.
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::mcp::protocol::jsonrpc::{parse_json_rpc_message, JsonRpcMessage, McpClientError};
use crate::mcp::transports::{
    McpTransport, TransportCloseListener, TransportErrorListener, TransportEvents,
    TransportMessageListener, Unsubscribe, DEFAULT_MAX_MESSAGE_BYTES,
};

const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_CLOSE_TIMEOUT_MS: u64 = 2_000;
/// How long a server gets to exit on its own after stdin closes, before it is
/// sent SIGTERM.
const STDIN_CLOSE_GRACE_MS: u64 = 500;

/// The `onStderr` callback type.
pub type StderrCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Upstream `StdioTransportOptions`.
#[derive(Clone)]
pub struct StdioTransportOptions {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Explicit environment; combined with the parent environment unless
    /// `inherit_env` is false.
    pub env: Option<Vec<(String, String)>>,
    pub inherit_env: bool,
    /// Upstream `stderr: "inherit"` (default pipe).
    pub inherit_stderr: bool,
    /// Upstream `onStderr` chunk callback (UTF-8 lossy per chunk).
    pub on_stderr: Option<StderrCallback>,
    pub max_message_bytes: Option<usize>,
    pub max_stderr_bytes: Option<usize>,
    /// Time to wait for the server to exit after SIGTERM before sending
    /// SIGKILL. Default: 2000.
    pub close_timeout_ms: Option<u64>,
}

impl StdioTransportOptions {
    pub fn new(command: impl Into<String>) -> Self {
        StdioTransportOptions {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: None,
            inherit_env: true,
            inherit_stderr: false,
            on_stderr: None,
            max_message_bytes: None,
            max_stderr_bytes: None,
            close_timeout_ms: None,
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

struct StdioShared {
    options: StdioTransportOptions,
    events: Arc<TransportEvents>,
    started: AtomicBool,
    closed: AtomicBool,
    /// Set by the watcher when `child.wait()` returns (upstream
    /// `child.exitCode !== null`).
    exited: AtomicBool,
    /// Watch-side of the exit flag; `shutdown` and the stdout reader await it
    /// instead of racing `Notify` (upstream awaits the child `close` event).
    exited_watch: tokio::sync::watch::Receiver<bool>,
    /// Sender half of the exit watch, held so `changed()` never disconnects.
    exited_sender: Mutex<tokio::sync::watch::Sender<bool>>,
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pid: Mutex<Option<u32>>,
    stdout_buffer: Mutex<Vec<u8>>,
    stderr_buffer: Mutex<Vec<u8>>,
}

/// Upstream `StdioTransport`.
pub struct StdioTransport {
    shared: Arc<StdioShared>,
}

impl StdioTransport {
    pub fn new(options: StdioTransportOptions) -> Self {
        let (exited_sender, exited_watch) = tokio::sync::watch::channel(false);
        StdioTransport {
            shared: Arc::new(StdioShared {
                events: Arc::new(TransportEvents::default()),
                options,
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                exited: AtomicBool::new(false),
                exited_sender: Mutex::new(exited_sender),
                exited_watch,
                stdin: Mutex::new(None),
                pid: Mutex::new(None),
                stdout_buffer: Mutex::new(Vec::new()),
                stderr_buffer: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Upstream `get pid`.
    pub fn pid(&self) -> Option<u32> {
        *self.shared.pid.lock().expect("pid cannot be poisoned")
    }

    /// Upstream `get stderr`: the captured stderr (last `maxStderrBytes`).
    pub fn stderr(&self) -> String {
        let buffer = self
            .shared
            .stderr_buffer
            .lock()
            .expect("stderr buffer cannot be poisoned")
            .clone();
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// Upstream `start()`: spawns the child, wires the stdout/stderr readers
    /// and the exit watcher.
    pub async fn start(&self) -> Result<(), McpClientError> {
        let shared = &self.shared;
        if shared.started.swap(true, Ordering::SeqCst) {
            return Err(McpClientError::Other(
                "MCP stdio transport already started".into(),
            ));
        }
        if shared.closed.load(Ordering::SeqCst) {
            return Err(McpClientError::connection_closed());
        }
        let mut command = Command::new(&shared.options.command);
        command.args(&shared.options.args);
        if let Some(cwd) = &shared.options.cwd {
            command.current_dir(cwd);
        }
        if shared.options.inherit_env {
            // Upstream `{ ...process.env, ...this.options.env }`.
            for (key, value) in std::env::vars() {
                command.env(key, value);
            }
        }
        if let Some(env) = &shared.options.env {
            for (key, value) in env {
                command.env(key, value);
            }
        }
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(if shared.options.inherit_stderr {
            Stdio::inherit()
        } else {
            Stdio::piped()
        });
        // Upstream `windowsHide: true` (tokio::process::Command exposes
        // `creation_flags` natively on Windows).
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        // Upstream `detached: USE_PROCESS_GROUPS` on non-Windows: the child
        // gets its own process group so closing the transport can terminate
        // the server's children too.
        #[cfg(unix)]
        {
            // `tokio::process::Command` exposes `process_group` natively on
            // unix (no std `CommandExt` import needed).
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .map_err(|error| McpClientError::Other(error.to_string()))?;
        let pid = child.id();
        *shared.pid.lock().expect("pid cannot be poisoned") = pid;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *shared.stdin.lock().expect("stdin cannot be poisoned") = stdin;

        if let Some(stdout) = stdout {
            let shared_reader = Arc::clone(shared);
            tokio::spawn(async move {
                run_stdout_reader(stdout, &shared_reader).await;
            });
        } else {
            // No stdout: nothing can signal the close path.
            shared.exited.store(true, Ordering::SeqCst);
        }
        if let Some(stderr) = stderr {
            let shared_reader = Arc::clone(shared);
            tokio::spawn(async move {
                run_stderr_reader(stderr, &shared_reader).await;
            });
        }
        let shared_watcher = Arc::clone(shared);
        tokio::spawn(async move {
            let mut child: Child = child;
            let _ = child.wait().await;
            // Upstream flips `stdin.writable` to false when the child exits;
            // dropping the handle makes later sends fail as connection-closed.
            let _ = shared_watcher
                .stdin
                .lock()
                .expect("stdin cannot be poisoned")
                .take();
            shared_watcher.exited.store(true, Ordering::SeqCst);
            let _ = shared_watcher
                .exited_sender
                .lock()
                .expect("exit sender cannot be poisoned")
                .send(true);
        });
        Ok(())
    }

    /// Upstream `close()` per the spec: close stdin and let the server exit,
    /// then SIGTERM, then SIGKILL.
    pub async fn shutdown(&self) {
        let shared = &self.shared;
        if shared.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if shared.exited.load(Ordering::SeqCst) {
            // Upstream: `child.stdin?.end(); return;` — the close event has
            // fired or is imminent through the watcher.
            end_stdin(shared).await;
            return;
        }
        let Some(pid) = self.pid() else {
            // No child was ever spawned: emitClose directly (upstream sees
            // `child === undefined`).
            shared.events.emit_close();
            return;
        };
        let close_timeout_ms = shared
            .options
            .close_timeout_ms
            .unwrap_or(DEFAULT_CLOSE_TIMEOUT_MS);
        let grace = STDIN_CLOSE_GRACE_MS.min(close_timeout_ms);
        let mut exited = shared.exited_watch.clone();
        // Close stdin and race the graceful exit against the timers.
        end_stdin(shared).await;
        tokio::select! {
            _ = wait_for_exit(&mut exited) => {}
            _ = tokio::time::sleep(Duration::from_millis(grace)) => {
                // Upstream `killProcessTree` skips the taskkill once the child
                // already exited (the `exitCode !== null` check).
                if !shared.exited.load(Ordering::SeqCst) {
                    kill_process_tree(pid, KillSignal::Terminate);
                }
                tokio::select! {
                    _ = wait_for_exit(&mut exited) => {}
                    _ = tokio::time::sleep(Duration::from_millis(close_timeout_ms)) => {
                        if !shared.exited.load(Ordering::SeqCst) {
                            kill_process_tree(pid, KillSignal::Kill);
                        }
                        let _ = wait_for_exit(&mut exited).await;
                    }
                }
            }
        }
    }
}

/// Resolves when the child exit watch flips (upstream awaiting the `close`
/// event after the stdio streams flush).
async fn wait_for_exit(exited: &mut tokio::sync::watch::Receiver<bool>) {
    while !*exited.borrow_and_update() {
        if exited.changed().await.is_err() {
            return;
        }
    }
}

async fn end_stdin(shared: &StdioShared) {
    let mut stdin = shared
        .stdin
        .lock()
        .expect("stdin cannot be poisoned")
        .take();
    if let Some(stdin) = stdin.as_mut() {
        // `child.stdin.end()` in Node: flush pending writes, then EOF.
        let _ = stdin.flush().await;
    }
    // Dropping the ChildStdin closes the pipe (EOF for the child).
}

#[derive(Clone, Copy)]
enum KillSignal {
    Terminate,
    Kill,
}

/// The `taskkill.exe` under the Windows system root (the PATH can differ).
#[cfg(windows)]
fn taskkill_path() -> PathBuf {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(system_root)
        .join("System32")
        .join("taskkill.exe")
}

#[cfg(windows)]
fn kill_process_tree(pid: u32, _signal: KillSignal) {
    // Windows has no graceful signals, and a `.cmd` shim would survive
    // killing only its host; tree-force the whole job. Upstream skips the
    // call once the child already exited, which this port's close path
    // guarantees before reaching here.
    let mut command = std::process::Command::new(taskkill_path());
    command
        .args(["/pid", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    // Upstream ignores spawn errors (`on("error", () => {})`).
    let _ = command.spawn();
}

/// Signals the process group through the external `kill` binary (no unsafe
/// libc FFI, same approach as the agent-core nodejs harness): the child was
/// spawned as its own process-group leader, so the negative pid signals the
/// whole tree. The `--` end-of-options marker is required — procps kill
/// otherwise parses the negative pid as an option and exits 0 having
/// signalled nothing.
#[cfg(unix)]
fn kill_process_tree(pid: u32, signal: KillSignal) {
    let number = match signal {
        KillSignal::Terminate => "-15",
        KillSignal::Kill => "-9",
    };
    let grouped = std::process::Command::new("kill")
        .args([number, "--", &format!("-{pid}")])
        .status();
    if !grouped.is_ok_and(|status| status.success()) {
        // The group is gone or was never created; fall back to the direct
        // child (upstream `child.kill(signal)`).
        let _ = std::process::Command::new("kill")
            .args([number, &pid.to_string()])
            .status();
    }
}

impl McpTransport for StdioTransport {
    fn start(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        Box::pin(async move { StdioTransport::start(self).await })
    }

    /// Upstream `send`: `JSON.stringify(message) + "\n"` to the child stdin.
    fn send(&self, message: &JsonRpcMessage) -> BoxFuture<'_, Result<(), McpClientError>> {
        let shared = Arc::clone(&self.shared);
        let payload = format!("{}\n", message.to_json_string());
        Box::pin(async move {
            // Upstream `!this.started || this.closed || !stdin?.writable`:
            // once the child exited the watcher drops the stdin handle.
            if !shared.started.load(Ordering::SeqCst) || shared.closed.load(Ordering::SeqCst) {
                return Err(McpClientError::connection_closed());
            }
            // Take the handle out for the duration of the write so the mutex
            // is not held across the await.
            let stdin = {
                let mut guard = shared.stdin.lock().expect("stdin cannot be poisoned");
                guard.take()
            };
            let Some(mut stdin) = stdin else {
                return Err(McpClientError::connection_closed());
            };
            let result = async {
                stdin.write_all(payload.as_bytes()).await?;
                stdin.flush().await?;
                Ok(())
            };
            let outcome = result
                .await
                .map_err(|error: std::io::Error| McpClientError::Other(error.to_string()));
            // Hand the pipe back unless it is gone (child exit raced the write;
            // the watcher owns the drop then).
            let mut guard = shared.stdin.lock().expect("stdin cannot be poisoned");
            if guard.is_none() && !shared.exited.load(Ordering::SeqCst) {
                *guard = Some(stdin);
            }
            outcome
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        Box::pin(async move {
            StdioTransport::shutdown(self).await;
            Ok(())
        })
    }

    fn on_message(&self, listener: TransportMessageListener) -> Unsubscribe {
        self.shared.events.on_message(listener)
    }

    fn on_error(&self, listener: TransportErrorListener) -> Unsubscribe {
        self.shared.events.on_error(listener)
    }

    fn on_close(&self, listener: TransportCloseListener) -> Unsubscribe {
        self.shared.events.on_close(listener)
    }
}

/// Upstream `handleStdout`: split on `\n`, enforce the byte budget, strip one
/// trailing `\r`, skip blank lines, parse and dispatch — with events and
/// errors emitted in stream order.
async fn run_stdout_reader<R: AsyncRead + Unpin>(mut reader: R, shared: &Arc<StdioShared>) {
    let max_message_bytes = shared
        .options
        .max_message_bytes
        .unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
    let mut chunk = [0u8; 4096];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                // Upstream `child.stdout.on("error", ...)` — suppressed once
                // the transport is closed.
                if !shared.closed.load(Ordering::SeqCst) {
                    shared
                        .events
                        .emit_error(&McpClientError::Other(error.to_string()));
                }
                break;
            }
        };
        let mut buffer = shared
            .stdout_buffer
            .lock()
            .expect("stdout buffer cannot be poisoned");
        buffer.extend_from_slice(&chunk[..read]);
        loop {
            let Some(newline) = buffer.iter().position(|byte| *byte == 0x0a) else {
                if buffer.len() > max_message_bytes {
                    buffer.clear();
                    shared.events.emit_error(&McpClientError::Other(format!(
                        "MCP stdio message exceeds {max_message_bytes} bytes"
                    )));
                }
                break;
            };
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let line = &line[..line.len() - 1];
            if line.len() > max_message_bytes {
                shared.events.emit_error(&McpClientError::Other(format!(
                    "MCP stdio message exceeds {max_message_bytes} bytes"
                )));
                continue;
            }
            let text = String::from_utf8_lossy(line);
            // Upstream `.replace(/\r$/, "")`: exactly one trailing \r.
            let text = match text.strip_suffix('\r') {
                Some(stripped) => stripped.to_string(),
                None => text.into_owned(),
            };
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|error| McpClientError::Other(error.to_string()))
                .and_then(|value| parse_json_rpc_message(&value).map_err(McpClientError::from))
            {
                Ok(message) => shared.events.emit_message(&message),
                Err(error) => {
                    // Upstream swallows write-after-close listener noise the
                    // same way: errors are suppressed once closed.
                    if !shared.closed.load(Ordering::SeqCst) {
                        shared.events.emit_error(&error);
                    }
                }
            }
        }
        drop(buffer);
    }
    // All stdout data events have been delivered; run the upstream child
    // `close` handler sequence.
    let mut exited = shared.exited_watch.clone();
    if !shared.exited.load(Ordering::SeqCst) {
        let _ = wait_for_exit(&mut exited).await;
    }
    let trailing_nonblank = {
        let mut buffer = shared
            .stdout_buffer
            .lock()
            .expect("stdout buffer cannot be poisoned");
        let trailing = String::from_utf8_lossy(&buffer);
        let nonblank = !trailing.trim().is_empty();
        buffer.clear();
        nonblank
    };
    if trailing_nonblank {
        shared.events.emit_error(&McpClientError::Other(
            "MCP stdio server closed with an incomplete JSON-RPC message".into(),
        ));
    }
    // Children of the server that ignored stdin closing would otherwise
    // outlive it (upstream killProcessTree(child, "SIGTERM") at close). On
    // Unix the group may still hold grandchildren even though the leader
    // exited; on Windows upstream skips the taskkill once the child exited,
    // which is guaranteed here.
    if let Some(pid) = *shared.pid.lock().expect("pid cannot be poisoned") {
        #[cfg(unix)]
        kill_process_tree(pid, KillSignal::Terminate);
        let _ = pid;
    }
    shared.events.emit_close();
}

/// Upstream `handleStderr`: keep the last `maxStderrBytes` and forward each
/// chunk to `onStderr`.
async fn run_stderr_reader<R: AsyncRead + Unpin>(mut reader: R, shared: &Arc<StdioShared>) {
    let max_stderr_bytes = shared
        .options
        .max_stderr_bytes
        .unwrap_or(DEFAULT_MAX_STDERR_BYTES);
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => {
                let bytes = chunk[..read].to_vec();
                {
                    let mut buffer = shared
                        .stderr_buffer
                        .lock()
                        .expect("stderr buffer cannot be poisoned");
                    buffer.extend_from_slice(&bytes);
                    if buffer.len() > max_stderr_bytes {
                        let overflow = buffer.len() - max_stderr_bytes;
                        buffer.drain(..overflow);
                    }
                }
                if let Some(on_stderr) = &shared.options.on_stderr {
                    on_stderr(&String::from_utf8_lossy(&bytes));
                }
            }
            Err(error) => {
                if !shared.closed.load(Ordering::SeqCst) {
                    shared
                        .events
                        .emit_error(&McpClientError::Other(error.to_string()));
                }
                break;
            }
        }
    }
}
