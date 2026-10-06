//! Port of `packages/agent/src/harness/env/nodejs.ts` (924 lines): the
//! filesystem + process execution environment, over `tokio`/`std` in place
//! of `node:fs`/`node:child_process`, implementing the M3b Task 2
//! [`FileSystem`](crate::agent_core::harness::types::FileSystem),
//! [`Shell`](crate::agent_core::harness::types::Shell), and
//! [`ExecutionEnv`](crate::agent_core::harness::types::ExecutionEnv) traits.
//!
//! Disclosed substitutions:
//! - **Platform gates.** Upstream branches on `process.platform` at runtime;
//!   the port uses `cfg!(windows)`/`#[cfg(unix)]`. Oracle tests that
//!   monkeypatch `process.platform` (the taskkill-spawn-error case) are
//!   therefore not portable and are disclosed in the test module.
//! - **Process trees.** Upstream kills detached trees with
//!   `process.kill(-pid, "SIGKILL")` (children spawn `detached`, i.e. as
//!   their own process-group leaders); the port spawns children with
//!   `process_group(0)` on unix and signals the group through `/bin/kill`,
//!   keeping `taskkill /F /T /PID` on Windows. `windowsHide` maps to
//!   `CREATE_NO_WINDOW`.
//! - **Spill backpressure.** Upstream pauses/resumes the child's stdio
//!   around a Node `WriteStream` with an 8MB high-water mark; the port feeds
//!   the spill through a bounded mpsc channel, so a slow spill file applies
//!   the same pressure one step earlier (the reader blocks on send, the OS
//!   pipe blocks the child). The observable contract — complete output in
//!   the spill file, no unbounded buffering — is preserved.
//! - **Stdio grace.** The post-exit grace timer (`EXIT_STDIO_GRACE_MS`,
//!   `nodejs.ts:292-371`) re-arms on arriving data and while the spill is
//!   draining, then destroys the pipes; the port polls the same conditions
//!   on a 5ms tick and cancels the reader tasks at grace expiry.
//! - **Decoding.** Child output and text files decode lossily (invalid
//!   sequences become U+FFFD), matching Node's non-fatal `TextDecoder` /
//!   `Buffer.toString("utf8")`.
//! - **Path semantics.** `path.resolve`/`path.join` are ported as lexical
//!   normalizations (collapse `.`, resolve `..` against the previous
//!   segment, drop separators); Windows verbatim (`\\?\`) prefixes are
//!   stripped from canonical paths, the same convention the harness test
//!   fixture uses. Symlinks are never followed by addressing. `join`
//!   concatenates like node (an absolute later segment does not reset the
//!   base).
//! - **Abort mid-IO.** Upstream threads the abort signal into `node:fs`
//!   calls; tokio filesystem calls take no signal, so every cancellable
//!   operation checks the token before and after the operation. The
//!   observable contract (an aborted operation returns an `aborted` result)
//!   holds; an operation that completes within one poll is indistinguishable
//!   from a completed one upstream too.
//! - **`mtimeMs`.** `f64` milliseconds since the epoch, like upstream's
//!   `number` (microsecond precision).
//! - **Timeout.** `ShellExecOptions.timeout` is whole seconds in the port
//!   (M3b Task 2 type); the upstream fractional-second cap message
//!   (`2147483.647` seconds) is kept verbatim but whole-second inputs only.
//! - **Virtual dispatch.** Upstream subclasses override `createTempFile` and
//!   the spill path follows; here the spawn machinery runs against
//!   `Arc<dyn ExecutionEnv>`, so implementors pass themselves — the dispatch
//!   stays virtual ([`exec_via`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::types::{
    CreateDirOptions, ExecutionEnv, ExecutionError, ExecutionErrorCode, FileContent, FileError,
    FileErrorCode, FileInfo, FileKind, FileSystem, ReadTextLinesOptions, RemoveOptions, Shell,
    ShellExecOptions, ShellExecResult, TempFileOptions, TextLine, TextLineReader,
};
use crate::agent_core::harness::utils::adaptive_publisher::panic_message;
use crate::agent_core::harness::utils::output_capture::{OutputCapture, OutputCaptureHandlers};
use crate::ai::uuid::uuid_v7;

/// Upstream `MAX_TIMEOUT_MS` (`nodejs.ts:39`).
const MAX_TIMEOUT_MS: u64 = 2_147_483_647;
/// Upstream `EXIT_STDIO_GRACE_MS` (`nodejs.ts:41`).
const EXIT_STDIO_GRACE_MS: u64 = 100;
/// Reader chunk size (upstream lets the stream chunk; pinned here for
/// deterministic spill behavior).
const READ_CHUNK: usize = 64 * 1024;
/// Grace-poll tick.
const GRACE_TICK_MS: u64 = 5;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------------------
// Path helpers (nodejs.ts:59-73; node path.resolve/join semantics)
// ---------------------------------------------------------------------------

/// Upstream `homedir()` (`nodejs.ts:62-64`).
fn home_dir() -> Option<String> {
    dirs::home_dir().map(|path| path_string(&path))
}

fn path_string(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    // Windows `fs::canonicalize` returns a `\\?\C:\...` verbatim prefix;
    // strip it so addressed and canonical paths stay comparable.
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        text = format!(r"\\{stripped}");
    } else if let Some(stripped) = text.strip_prefix(r"\\?\") {
        text = stripped.to_string();
    }
    text
}

/// Lexically normalize an absolute path like node's `path.resolve`
/// (collapse `.`, resolve `..`, drop trailing separators).
fn normalize_absolute(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the previous normal component; `..` past the root is
                // dropped (the root's parent is the root).
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    path_string(&normalized)
}

fn normalize_relative(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    path_string(&normalized)
}

/// Upstream `resolvePath` (`nodejs.ts:59-73`): expand `~`, `~/`, `~\`, and
/// `file://` URLs, then resolve against `cwd`.
fn resolve_path(cwd: &str, path: &str) -> String {
    let mut normalized = path.to_string();
    if normalized == "~" {
        if let Some(home) = home_dir() {
            normalized = home;
        }
    } else if normalized.starts_with("~/") || (cfg!(windows) && normalized.starts_with("~\\")) {
        if let Some(home) = home_dir() {
            normalized = join_raw(&home, &normalized[2..]);
        }
    } else if normalized.starts_with("file://") {
        // `fileURLToPath`; malformed URLs stay ordinary paths so filesystem
        // methods preserve their non-throwing contract (nodejs.ts:65-71).
        if let Ok(url) = url::Url::parse(&normalized) {
            if let Ok(file_path) = url.to_file_path() {
                normalized = path_string(&file_path);
            }
        }
    }
    if Path::new(&normalized).is_absolute() {
        normalize_absolute(Path::new(&normalized))
    } else {
        normalize_absolute(&Path::new(cwd).join(&normalized))
    }
}

/// Concatenate two path fragments with the platform separator (node `join`
/// concatenates; an absolute later segment does not reset the base).
fn join_raw(base: &str, tail: &str) -> String {
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let mut joined = String::from(base);
    if !joined.is_empty() && !joined.ends_with(separator) {
        joined.push(separator);
    }
    joined.push_str(tail);
    joined
}

/// Upstream `joinPath` (`nodejs.ts:454-456`): `join(...parts)`.
fn join_paths(parts: &[String]) -> String {
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let mut joined = String::new();
    for part in parts {
        if !joined.is_empty() {
            joined.push(separator);
        }
        joined.push_str(part);
    }
    let joined = Path::new(&joined);
    if joined.is_absolute() {
        normalize_absolute(joined)
    } else {
        // join() never resolves against cwd; normalize relative input as-is.
        normalize_relative(joined)
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// ---------------------------------------------------------------------------
// Error mapping (nodejs.ts:101-133)
// ---------------------------------------------------------------------------

/// Upstream `toFileError` (`nodejs.ts:105-129`): map a backend error onto
/// the stable [`FileErrorCode`] vocabulary.
fn to_file_error(error: io::Error, fallback_path: Option<&str>) -> FileError {
    let path = fallback_path.map(str::to_string);
    let message = error.to_string();
    let code = match error.kind() {
        io::ErrorKind::NotFound => FileErrorCode::NotFound,
        io::ErrorKind::PermissionDenied => FileErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => FileErrorCode::Invalid,
        _ => match error.raw_os_error() {
            // ENOTDIR
            Some(20) => FileErrorCode::NotDirectory,
            // EISDIR
            Some(21) => FileErrorCode::IsDirectory,
            // Windows ERROR_DIRECTORY: listing a non-directory.
            Some(267) => FileErrorCode::NotDirectory,
            _ => FileErrorCode::Unknown,
        },
    };
    FileError::new(code, message, path).with_cause(Some(Box::new(error)))
}

/// Upstream `abortResult` (`nodejs.ts:131-133`): the pre-operation abort
/// check every cancellable operation opens with.
fn abort_check<T>(context: &Context, path: Option<&str>) -> Option<Result<T, FileError>> {
    if context
        .abort_signal()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Some(Err(FileError::new(
            FileErrorCode::Aborted,
            "aborted",
            path.map(str::to_string),
        )));
    }
    None
}

/// The post-operation abort check (module docs, "Abort mid-IO").
fn abort_check_after<T>(context: &Context, path: Option<&str>, value: T) -> Result<T, FileError> {
    if context
        .abort_signal()
        .is_some_and(|signal| signal.is_cancelled())
    {
        return Err(FileError::new(
            FileErrorCode::Aborted,
            "aborted",
            path.map(str::to_string),
        ));
    }
    Ok(value)
}

fn aborted_error(path: Option<&str>) -> FileError {
    FileError::new(FileErrorCode::Aborted, "aborted", path.map(str::to_string))
}

fn mtime_ms(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| {
            duration.as_secs() as f64 * 1000.0 + f64::from(duration.subsec_micros()) / 1000.0
        })
        .unwrap_or(0.0)
}

/// Upstream `fileInfoFromStats` (`nodejs.ts:86-99`) over an `lstat` result.
fn file_info_from_metadata(
    path: &str,
    metadata: &std::fs::Metadata,
) -> Result<FileInfo, FileError> {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        FileKind::Symlink
    } else if metadata.is_file() {
        FileKind::File
    } else if metadata.is_dir() {
        FileKind::Directory
    } else {
        return Err(FileError::new(
            FileErrorCode::Invalid,
            "Unsupported file type",
            Some(path.to_string()),
        ));
    };
    Ok(FileInfo {
        name: basename(path),
        path: path.to_string(),
        kind,
        size: metadata.len(),
        mtime_ms: mtime_ms(metadata),
    })
}

async fn lstat(path: &str) -> io::Result<std::fs::Metadata> {
    tokio::fs::symlink_metadata(path).await
}

/// Upstream `pathExists` (`nodejs.ts:135-142`).
async fn path_exists(path: &str) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

// ---------------------------------------------------------------------------
// Text line reader (nodejs.ts:373-436)
// ---------------------------------------------------------------------------

struct TextLineReaderState {
    file: Option<tokio::fs::File>,
    byte_offset: u64,
    buffered: Vec<u8>,
    ended: bool,
    closed: bool,
}

/// Upstream `NodeTextLineReader` (`nodejs.ts:374-436`): a strict-LF reader
/// that reports whether the final line was newline-terminated. Explicit
/// positions allow an aborted read to be retried without skipping bytes.
struct NodeTextLineReader {
    path: String,
    state: tokio::sync::Mutex<TextLineReaderState>,
}

impl NodeTextLineReader {
    fn new(file: tokio::fs::File, path: String) -> Self {
        NodeTextLineReader {
            path,
            state: tokio::sync::Mutex::new(TextLineReaderState {
                file: Some(file),
                byte_offset: 0,
                buffered: Vec::new(),
                ended: false,
                closed: false,
            }),
        }
    }
}

impl TextLineReader for NodeTextLineReader {
    fn read_line<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, Result<Option<TextLine>, FileError>> {
        Box::pin(async move {
            if let Some(aborted) = abort_check::<Option<TextLine>>(&context, Some(&self.path)) {
                return aborted;
            }
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(FileError::new(
                    FileErrorCode::Invalid,
                    "Text line reader is closed",
                    Some(self.path.clone()),
                ));
            }
            loop {
                if let Some(newline) = state.buffered.iter().position(|byte| *byte == b'\n') {
                    let text = String::from_utf8_lossy(&state.buffered[..newline]).into_owned();
                    state.buffered.drain(..=newline);
                    return Ok(Some(TextLine {
                        text,
                        terminated: true,
                    }));
                }
                if state.ended {
                    if state.buffered.is_empty() {
                        return Ok(None);
                    }
                    let text = String::from_utf8_lossy(&state.buffered).into_owned();
                    state.buffered.clear();
                    return Ok(Some(TextLine {
                        text,
                        terminated: false,
                    }));
                }

                // Explicit positions allow an aborted read to be retried
                // without skipping bytes (nodejs.ts:409-419).
                let mut file = state.file.take().expect("open file held until close");
                use tokio::io::AsyncSeekExt;
                let read = async {
                    file.seek(std::io::SeekFrom::Start(state.byte_offset))
                        .await?;
                    let mut chunk = vec![0u8; READ_CHUNK];
                    let bytes_read = file.read(&mut chunk).await?;
                    io::Result::Ok((chunk, bytes_read))
                };
                match read.await {
                    Ok((chunk, bytes_read)) => {
                        // The read raced the abort; report it so the retry
                        // restarts at the same explicit position.
                        if context
                            .abort_signal()
                            .is_some_and(|signal| signal.is_cancelled())
                        {
                            state.file = Some(file);
                            return Err(aborted_error(Some(&self.path)));
                        }
                        state.file = Some(file);
                        state.byte_offset += bytes_read as u64;
                        if bytes_read == 0 {
                            state.ended = true;
                        } else {
                            state.buffered.extend_from_slice(&chunk[..bytes_read]);
                        }
                    }
                    Err(error) => {
                        state.file = Some(file);
                        return Err(to_file_error(error, Some(&self.path)));
                    }
                }
            }
        })
    }

    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if state.closed {
                return;
            }
            state.closed = true;
            state.buffered.clear();
            // Closing is best-effort, including after cancellation or an
            // earlier I/O failure (nodejs.ts:430-435).
            state.file = None;
        })
    }
}

// ---------------------------------------------------------------------------
// Shell discovery and process tree (nodejs.ts:144-290)
// ---------------------------------------------------------------------------

/// Upstream `ShellConfig` (`nodejs.ts:189-193`).
struct ShellConfig {
    shell: String,
    args: Vec<String>,
    /// Upstream `commandTransport: "argv" | "stdin"`; `true` is stdin.
    stdin_transport: bool,
}

/// Upstream `killProcessTree` (`nodejs.ts:261-290`).
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let taskkill = Path::new(&system_root)
            .join("System32")
            .join("taskkill.exe");
        let mut command = std::process::Command::new(taskkill);
        command.args(["/F", "/T", "/PID", &pid.to_string()]);
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());
        command.stdin(std::process::Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        // A failed spawn emits an error asynchronously upstream; the port's
        // spawn result is consumed the same best-effort way.
        let _ = command.spawn();
    }
    #[cfg(unix)]
    {
        // `process.kill(-pid, "SIGKILL")`: the child was spawned as its own
        // process-group leader, so the negative pid kills the whole tree.
        // The `--` end-of-options marker is required: procps kill otherwise
        // parses the negative pid as an option and exits 0 having signalled
        // nothing. Fall back to the single process (`nodejs.ts:281-289`).
        let group = std::process::Command::new("kill")
            .args(["-9", "--", &format!("-{pid}")])
            .status();
        if !group.is_ok_and(|status| status.success()) {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
    }
}

/// Upstream `runCommand` (`nodejs.ts:144-177`): spawn, collect stdout, kill
/// after the timeout. Returns `{ stdout, status }`; a spawn failure returns
/// the same `{ stdout: "", status: null }` shape.
async fn run_command(command: &str, args: &[&str], timeout_ms: u64) -> (String, Option<i32>) {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let Ok(mut child) = cmd.spawn() else {
        return (String::new(), None);
    };
    let Some(stdout) = child.stdout.take() else {
        return (String::new(), None);
    };
    let read = tokio::spawn(async move {
        let mut stdout = stdout;
        let mut buffer = Vec::new();
        loop {
            let mut chunk = vec![0u8; READ_CHUNK];
            match stdout.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => buffer.extend_from_slice(&chunk[..bytes_read]),
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    });
    let wait = async {
        let stdout = read.await.unwrap_or_default();
        let status = child.wait().await.ok().and_then(|status| status.code());
        (stdout, status)
    };
    tokio::select! {
        result = wait => result,
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
            if let Some(pid) = child.id() {
                kill_process_tree(pid);
            }
            (String::new(), None)
        }
    }
}

/// Upstream `findBashOnPath` (`nodejs.ts:179-187`).
async fn find_bash_on_path() -> Option<String> {
    let (stdout, status) = if cfg!(windows) {
        run_command("where", &["bash.exe"], 5000).await
    } else {
        run_command("which", &["bash"], 5000).await
    };
    if status != Some(0) {
        return None;
    }
    let first_match = stdout.trim().lines().next()?.trim().to_string();
    if first_match.is_empty() {
        return None;
    }
    if path_exists(&first_match).await {
        Some(first_match)
    } else {
        None
    }
}

/// Upstream `isLegacyWslBashPath` (`nodejs.ts:195-198`).
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let mut chars = normalized.chars();
    let Some(drive) = chars.next() else {
        return false;
    };
    if !drive.is_ascii_lowercase() || !normalized[1..].starts_with(r":\windows\") {
        return false;
    }
    let rest = &normalized[r":\windows\".len() + 1..];
    let Some(rest) = rest
        .strip_prefix(r"system32\")
        .or_else(|| rest.strip_prefix(r"sysnative\"))
    else {
        return false;
    };
    rest == "bash.exe"
}

/// Upstream `getBashShellConfig` (`nodejs.ts:200-202`).
fn get_bash_shell_config(shell: String) -> ShellConfig {
    if is_legacy_wsl_bash_path(&shell) {
        ShellConfig {
            shell,
            args: vec!["-s".into()],
            stdin_transport: true,
        }
    } else {
        ShellConfig {
            shell,
            args: vec!["-c".into()],
            stdin_transport: false,
        }
    }
}

/// Upstream `getShellConfig` (`nodejs.ts:204-246`).
async fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, ExecutionError> {
    if let Some(custom) = custom_shell_path {
        if path_exists(custom).await {
            return Ok(get_bash_shell_config(custom.to_string()));
        }
        return Err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            format!("Custom shell path not found: {custom}"),
        ));
    }
    if cfg!(windows) {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(format!("{program_files}\\Git\\bin\\bash.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(format!("{program_files_x86}\\Git\\bin\\bash.exe"));
        }
        for candidate in &candidates {
            if path_exists(candidate).await {
                return Ok(get_bash_shell_config(candidate.clone()));
            }
        }
        if let Some(bash_on_path) = find_bash_on_path().await {
            return Ok(get_bash_shell_config(bash_on_path));
        }
        let listed = candidates
            .iter()
            .map(|path| format!("  {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            "No bash shell found. Options:\n\
             1. Install Git for Windows: https://git-scm.com/download/win\n\
             2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n\
             3. Configure an explicit shellPath\n\n\
             Searched Git Bash in:\n"
                .to_string()
                + &listed,
        ));
    }

    if path_exists("/bin/bash").await {
        return Ok(get_bash_shell_config("/bin/bash".to_string()));
    }
    if let Some(bash_on_path) = find_bash_on_path().await {
        return Ok(get_bash_shell_config(bash_on_path));
    }
    Ok(ShellConfig {
        shell: "sh".into(),
        args: vec!["-c".into()],
        stdin_transport: false,
    })
}

/// Upstream `getShellEnv` (`nodejs.ts:248-259`): process env, then the
/// configured base env, then the per-command overrides.
fn get_shell_env(
    base_env: Option<&BTreeMap<String, String>>,
    extra_env: Option<&BTreeMap<String, String>>,
    inherit_env: bool,
) -> BTreeMap<String, String> {
    if !inherit_env {
        return extra_env.cloned().unwrap_or_default();
    }
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    if let Some(base_env) = base_env {
        for (key, value) in base_env {
            env.insert(key.clone(), value.clone());
        }
    }
    if let Some(extra_env) = extra_env {
        for (key, value) in extra_env {
            env.insert(key.clone(), value.clone());
        }
    }
    env
}

/// Upstream `resolveTimeoutMs` (`nodejs.ts:46-57`) over the port's
/// floating-point seconds, including fractional durations.
fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<u64>, ExecutionError> {
    let Some(timeout) = timeout else {
        return Ok(None);
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            "Invalid timeout: must be a finite number of seconds",
        ));
    }
    let timeout_ms = timeout * 1000.0;
    if timeout_ms > MAX_TIMEOUT_MS as f64 {
        return Err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            "Invalid timeout: maximum is 2147483.647 seconds",
        ));
    }
    // Node timers truncate fractional milliseconds and clamp positive sub-ms
    // delays to one millisecond.
    Ok(Some((timeout_ms as u64).max(1)))
}

// ---------------------------------------------------------------------------
// NodeExecutionEnv (nodejs.ts:438-924)
// ---------------------------------------------------------------------------

/// The shell-discovery and process-tracking state one implementor carries
/// (upstream instance fields `shellPath`/`shellEnv`/`activeChildPids`);
/// passed to [`exec_via`] explicitly so the spawn machinery runs against
/// `Arc<dyn ExecutionEnv>` and `createTempFile` dispatch stays virtual.
#[derive(Clone, Default)]
pub struct ShellRuntime {
    pub shell_path: Option<String>,
    pub shell_env: Option<BTreeMap<String, String>>,
    pub active_child_pids: Arc<Mutex<BTreeSet<u32>>>,
}

/// Upstream `NodeExecutionEnv` (`nodejs.ts:438-924`). Clone to share one
/// environment (the handle is cheap; the process set is shared).
#[derive(Clone)]
pub struct NodeExecutionEnv {
    cwd: String,
    runtime: ShellRuntime,
}

impl NodeExecutionEnv {
    /// Upstream `new NodeExecutionEnv({ cwd, shellPath, shellEnv })`
    /// (`nodejs.ts:444-448`).
    pub fn new(cwd: impl Into<String>) -> Self {
        NodeExecutionEnv {
            cwd: cwd.into(),
            runtime: ShellRuntime::default(),
        }
    }

    /// Upstream `shellPath`.
    pub fn with_shell_path(mut self, shell_path: impl Into<String>) -> Self {
        self.runtime.shell_path = Some(shell_path.into());
        self
    }

    /// Upstream `shellEnv`.
    pub fn with_shell_env(mut self, shell_env: BTreeMap<String, String>) -> Self {
        self.runtime.shell_env = Some(shell_env);
        self
    }

    fn kill_active_pids(&self) {
        let pids: BTreeSet<u32> =
            std::mem::take(&mut *self.runtime.active_child_pids.lock().unwrap());
        for pid in pids {
            kill_process_tree(pid);
        }
    }
}

impl FileSystem for NodeExecutionEnv {
    fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Upstream `absolutePath` (`nodejs.ts:450-452`).
    fn absolute_path<'a>(
        &'a self,
        path: &str,
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let path = path.to_string();
        Box::pin(async move { Ok(resolve_path(&self.cwd, &path)) })
    }

    /// Upstream `joinPath` (`nodejs.ts:454-456`).
    fn join_path<'a>(
        &'a self,
        parts: &[String],
        _context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let parts = parts.to_vec();
        Box::pin(async move { Ok(join_paths(&parts)) })
    }

    /// Upstream `readTextFile` (`nodejs.ts:716-726`).
    fn read_text_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<String>(&context, Some(&resolved)) {
                return aborted;
            }
            match tokio::fs::read(&resolved).await {
                Ok(bytes) => abort_check_after(
                    &context,
                    Some(&resolved),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `openTextLineReader` (`nodejs.ts:699-714`).
    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn TextLineReader>, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<Arc<dyn TextLineReader>>(&context, Some(&resolved))
            {
                return aborted;
            }
            match tokio::fs::File::open(&resolved).await {
                Ok(file) => {
                    if let Some(aborted) =
                        abort_check::<Arc<dyn TextLineReader>>(&context, Some(&resolved))
                    {
                        // Close the opened file best-effort before reporting.
                        drop(file);
                        return aborted;
                    }
                    Ok(Arc::new(NodeTextLineReader::new(file, resolved)))
                }
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `readTextLines` (`nodejs.ts:728-748`).
    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&ReadTextLinesOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        let path = path.to_string();
        let options = options.cloned();
        Box::pin(async move {
            let max_lines = options.as_ref().and_then(|options| options.max_lines);
            if max_lines == Some(0) {
                return Ok(Vec::new());
            }
            let opened = self.open_text_line_reader(&path, context.clone()).await?;
            let mut lines: Vec<String> = Vec::new();
            while max_lines.is_none() || lines.len() < max_lines.unwrap_or(0) as usize {
                let line = opened.read_line(context.clone()).await?;
                let Some(line) = line else { break };
                lines.push(line.text);
            }
            opened.close(context).await;
            Ok(lines)
        })
    }

    /// Upstream `readBinaryFile` (`nodejs.ts:750-760`).
    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<Vec<u8>>(&context, Some(&resolved)) {
                return aborted;
            }
            match tokio::fs::read(&resolved).await {
                Ok(bytes) => abort_check_after(&context, Some(&resolved), bytes),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `writeFile` (`nodejs.ts:762-776`): create parent directories
    /// first, like upstream's `mkdir(resolve(resolved, ".."))`.
    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<()>(&context, Some(&resolved)) {
                return aborted;
            }
            let bytes = match content {
                FileContent::Text(text) => text.into_bytes(),
                FileContent::Binary(bytes) => bytes,
            };
            if let Err(error) = write_with_parents(&resolved, &bytes).await {
                return Err(to_file_error(error, Some(&resolved)));
            }
            abort_check_after(&context, Some(&resolved), ())
        })
    }

    /// Upstream `appendFile` (`nodejs.ts:778-793`).
    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<()>(&context, Some(&resolved)) {
                return aborted;
            }
            let bytes = match content {
                FileContent::Text(text) => text.into_bytes(),
                FileContent::Binary(bytes) => bytes,
            };
            // `mkdir(parent)` then append (nodejs.ts:784-787): the node
            // `appendFile` shape — create when missing, never truncate.
            let appended = async {
                let parent = Path::new(&resolved).parent().unwrap_or(Path::new("."));
                tokio::fs::create_dir_all(parent).await?;
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&resolved)
                    .await?;
                file.write_all(&bytes).await
            }
            .await;
            if let Err(error) = appended {
                return Err(to_file_error(error, Some(&resolved)));
            }
            abort_check_after(&context, Some(&resolved), ())
        })
    }

    /// Upstream `renameFile` (`nodejs.ts:795-806`): atomically rename,
    /// replacing the destination; failures report the source path.
    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let source_path = source_path.to_string();
        let destination_path = destination_path.to_string();
        Box::pin(async move {
            let source = resolve_path(&self.cwd, &source_path);
            let destination = resolve_path(&self.cwd, &destination_path);
            if let Some(aborted) = abort_check::<()>(&context, Some(&destination)) {
                return aborted;
            }
            match tokio::fs::rename(&source, &destination).await {
                Ok(()) => Ok(()),
                Err(error) => Err(to_file_error(error, Some(&source))),
            }
        })
    }

    /// Upstream `fileInfo` (`nodejs.ts:808-817`): `lstat` — symlinks are not
    /// followed.
    fn file_info<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<FileInfo>(&context, Some(&resolved)) {
                return aborted;
            }
            match lstat(&resolved).await {
                Ok(metadata) => file_info_from_metadata(&resolved, &metadata),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `listDir` (`nodejs.ts:819-842`): direct children via
    /// `lstat`, so symlinks list as symlinks. An unsupported child kind is
    /// skipped; a child stat failure fails the listing (upstream returns the
    /// error with the child path).
    fn list_dir<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<Vec<FileInfo>>(&context, Some(&resolved)) {
                return aborted;
            }
            let mut entries = match tokio::fs::read_dir(&resolved).await {
                Ok(entries) => entries,
                Err(error) => return Err(to_file_error(error, Some(&resolved))),
            };
            let mut infos: Vec<FileInfo> = Vec::new();
            loop {
                if let Some(aborted) = abort_check::<Vec<FileInfo>>(&context, Some(&resolved)) {
                    return aborted;
                }
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(error) => return Err(to_file_error(error, Some(&resolved))),
                };
                let entry_path = path_string(&Path::new(&resolved).join(entry.file_name()));
                match lstat(&entry_path).await {
                    Ok(metadata) => {
                        if let Ok(info) = file_info_from_metadata(&entry_path, &metadata) {
                            infos.push(info);
                        }
                    }
                    Err(error) => return Err(to_file_error(error, Some(&entry_path))),
                }
            }
            Ok(infos)
        })
    }

    /// Upstream `canonicalPath` (`nodejs.ts:844-853`).
    fn canonical_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<String>(&context, Some(&resolved)) {
                return aborted;
            }
            match tokio::fs::canonicalize(&resolved).await {
                Ok(canonical) => Ok(path_string(&canonical)),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `exists` (`nodejs.ts:855-860`).
    fn exists<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        let path = path.to_string();
        Box::pin(async move {
            match self.file_info(&path, context.clone()).await {
                Ok(_) => Ok(true),
                Err(error) if error.code == FileErrorCode::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        })
    }

    /// Upstream `createDir` (`nodejs.ts:862-876`): defaults to recursive.
    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&CreateDirOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let path = path.to_string();
        let options = options.cloned();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<()>(&context, Some(&resolved)) {
                return aborted;
            }
            let recursive = options
                .and_then(|options| options.recursive)
                .unwrap_or(true);
            let created = if recursive {
                tokio::fs::create_dir_all(&resolved).await
            } else {
                tokio::fs::create_dir(&resolved).await
            };
            match created {
                Ok(()) => Ok(()),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `remove` (`nodejs.ts:878-892`): defaults `recursive: false`,
    /// `force: false`. Symlinks are removed as links, never traversed.
    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&RemoveOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let path = path.to_string();
        let options = options.cloned();
        Box::pin(async move {
            let resolved = resolve_path(&self.cwd, &path);
            if let Some(aborted) = abort_check::<()>(&context, Some(&resolved)) {
                return aborted;
            }
            let recursive = options
                .and_then(|options| options.recursive)
                .unwrap_or(false);
            let force = options.and_then(|options| options.force).unwrap_or(false);
            let metadata = lstat(&resolved).await;
            let result = match metadata {
                Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        tokio::fs::remove_file(&resolved).await
                    } else if recursive {
                        tokio::fs::remove_dir_all(&resolved).await
                    } else {
                        tokio::fs::remove_dir(&resolved).await
                    }
                }
            };
            // `force` ignores missing paths like node's `rm(..., { force })`.
            let result = match result {
                Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
            match result {
                Ok(()) => Ok(()),
                Err(error) => Err(to_file_error(error, Some(&resolved))),
            }
        })
    }

    /// Upstream `createTempDir` (`nodejs.ts:894-903`): `mkdtemp(tmpdir() +
    /// prefix)`; defaults to prefix `"tmp-"`.
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let prefix = prefix.unwrap_or("tmp-").to_string();
        Box::pin(async move {
            if let Some(aborted) = abort_check::<String>(&context, None) {
                return aborted;
            }
            let base = std::env::temp_dir().join(format!("{prefix}{}", uuid_v7()));
            match tokio::fs::create_dir_all(&base).await {
                Ok(()) => Ok(path_string(&base)),
                Err(error) => Err(to_file_error(error, None)),
            }
        })
    }

    /// Upstream `createTempFile` (`nodejs.ts:905-918`): a fresh temp dir with
    /// one empty file inside.
    fn create_temp_file<'a>(
        &'a self,
        options: Option<&TempFileOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        let options = options.cloned();
        Box::pin(async move {
            let dir = self.create_temp_dir(Some("tmp-"), context.clone()).await?;
            let options = options.unwrap_or_default();
            let file_path = join_raw(
                &dir,
                &format!(
                    "{}{}{}",
                    options.prefix.unwrap_or_default(),
                    uuid_v7(),
                    options.suffix.unwrap_or_default()
                ),
            );
            match tokio::fs::File::create(&file_path).await {
                Ok(_) => Ok(file_path),
                Err(error) => Err(to_file_error(error, Some(&file_path))),
            }
        })
    }

    /// Upstream `cleanup` (`nodejs.ts:920-923`).
    fn cleanup<'a>(&'a self, _context: Context) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.kill_active_pids();
        })
    }
}

/// `mkdir -p` the parent, then write (`nodejs.ts:768-771`).
async fn write_with_parents(resolved: &str, bytes: &[u8]) -> io::Result<()> {
    let parent = Path::new(resolved).parent().unwrap_or(Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    tokio::fs::write(resolved, bytes).await
}

impl Shell for NodeExecutionEnv {
    /// Upstream `exec` (`nodejs.ts:458-697`). The spawn machinery runs
    /// against `Arc<dyn ExecutionEnv>` (module docs, "Virtual dispatch").
    fn exec<'a>(
        &'a self,
        command: &str,
        options: Option<&ShellExecOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<ShellExecResult, ExecutionError>> {
        let env: Arc<dyn ExecutionEnv> = Arc::new(self.clone());
        let command = command.to_string();
        let options = options.cloned();
        Box::pin(async move {
            exec_via(
                env,
                self.runtime.clone(),
                &command,
                options.as_ref(),
                context,
            )
            .await
        })
    }

    /// Upstream `cleanup` (`nodejs.ts:920-923`), shared with
    /// [`FileSystem::cleanup`].
    fn cleanup<'a>(&'a self, _context: Context) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.kill_active_pids();
        })
    }
}

impl ExecutionEnv for NodeExecutionEnv {}

// ---------------------------------------------------------------------------
// exec internals (nodejs.ts:458-697)
// ---------------------------------------------------------------------------

/// The first truncation crossers held back until the spill exists (upstream
/// `spillPrefix`, `nodejs.ts:492`).
struct SpillShared {
    queue: Vec<Vec<u8>>,
    path: Option<String>,
    sender: Option<mpsc::Sender<Vec<u8>>>,
    start_started: bool,
    done: bool,
    error: Option<ExecutionError>,
}

type SharedSpill = Arc<tokio::sync::Mutex<SpillShared>>;
type FailFn = Arc<dyn Fn(String) + Send + Sync>;

/// Kill the running child (upstream `onAbort`, `nodejs.ts:499-501`): the pid
/// is set once the child exists.
struct ChildKill {
    pid: Mutex<Option<u32>>,
}

impl ChildKill {
    fn kill(&self) {
        if let Some(pid) = *self.pid.lock().unwrap() {
            kill_process_tree(pid);
        }
    }
}

fn fail_locked(shared: &mut SpillShared, kill: &ChildKill, message: String) {
    if shared.error.is_some() {
        return;
    }
    shared.error = Some(ExecutionError::new(
        ExecutionErrorCode::Unknown,
        format!("Failed to preserve complete shell output: {message}"),
    ));
    shared.done = true;
    kill.kill();
}

/// For callers that do not hold the spill lock (the writer task).
async fn fail_spill(spill: &SharedSpill, kill: &ChildKill, message: String) {
    let mut shared = spill.lock().await;
    fail_locked(&mut shared, kill, message);
}

/// Open the spill file, start the writer task, and hand the queued chunks to
/// it as its ordered backlog (upstream `startSpill`, `nodejs.ts:559-579`:
/// `for (const queued of spillQueue) writeSpill(queued)`). Called by the one
/// feed that drove the start. The spill lock is held only while the file is
/// created and the writer installed — never across a channel send — so the
/// writer's failure path can always acquire it (no drain/writer cycle), and
/// chunks routed while the temp file was being created keep their arrival
/// order ahead of everything routed after the sender was installed.
async fn start_spill(
    spill: &SharedSpill,
    capture: &Arc<OutputCapture>,
    env: &Arc<dyn ExecutionEnv>,
    context: &Context,
    kill: &Arc<ChildKill>,
) {
    let mut shared = spill.lock().await;
    let created = env
        .create_temp_file(
            Some(&TempFileOptions {
                prefix: Some("pi-output-".into()),
                suffix: Some(".log".into()),
            }),
            context.clone(),
        )
        .await;
    let path = match created {
        Ok(path) => path,
        Err(error) => {
            fail_locked(&mut shared, kill, error.to_string());
            return;
        }
    };
    capture.set_spill_path(&path);
    let opened = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await;
    let mut file = match opened {
        Ok(file) => file,
        Err(error) => {
            fail_locked(&mut shared, kill, error.to_string());
            return;
        }
    };
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(8);
    // The queued chunks become the writer's ordered backlog: it writes them
    // before anything routed after the sender was installed, so arrival
    // order survives without holding the lock across channel sends — the
    // writer's failure path can always acquire the lock and never cycles
    // against a parked drain.
    let backlog = std::mem::take(&mut shared.queue);
    shared.path = Some(path);
    shared.sender = Some(sender);
    let writer_spill = Arc::clone(spill);
    let writer_kill = Arc::clone(kill);
    tokio::spawn(async move {
        for chunk in backlog {
            if let Err(error) = file.write_all(&chunk).await {
                fail_spill(&writer_spill, &writer_kill, error.to_string()).await;
                break;
            }
        }
        while let Some(chunk) = receiver.recv().await {
            if let Err(error) = file.write_all(&chunk).await {
                fail_spill(&writer_spill, &writer_kill, error.to_string()).await;
                break;
            }
        }
        let _ = file.flush().await;
        writer_spill.lock().await.done = true;
    });
}

/// Shared reader-task state for one exec run.
struct RunState {
    env: Arc<dyn ExecutionEnv>,
    context: Context,
    capture: Arc<OutputCapture>,
    spill: SharedSpill,
    spill_enabled: bool,
    fail: FailFn,
    kill: Arc<ChildKill>,
    /// Data arrivals since the last grace check (upstream `onData` re-arms
    /// the idle timer, `nodejs.ts:333-335`).
    data_seen: AtomicU64,
}

impl RunState {
    /// Upstream `feed` (`nodejs.ts:628-645`): push into the capture, then
    /// route the chunk for spilling. A chunk arriving while the spill is
    /// starting (was already truncated, spill not open) queues behind the
    /// start — upstream's `startSpill` pushes it into `spillQueue` — and the
    /// start drains the queue in arrival order. Panics from the capture's
    /// callback path surface through `fail` (upstream try/catch feeding
    /// `failCallback`).
    async fn feed(&self, chunk: Vec<u8>) {
        let now_truncated = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.capture.push(&chunk);
            self.capture.truncated()
        })) {
            Ok(now_truncated) => now_truncated,
            Err(error) => {
                (self.fail)(panic_message(error.as_ref()));
                return;
            }
        };
        if !self.spill_enabled || chunk.is_empty() {
            return;
        }
        let mut shared = self.spill.lock().await;
        if shared.error.is_some() {
            return;
        }
        if shared.path.is_some() {
            // Spill open: the bounded channel applies the backpressure
            // upstream got from `write() === false` + pause/resume. A
            // missing sender means the streams were already destroyed during
            // settle (upstream `child.stdout?.destroy()`); discard.
            if let Some(sender) = shared.sender.clone() {
                drop(shared);
                let _ = sender.send(chunk).await;
            }
            return;
        }
        // Spill not open yet (starting, or crossing now): queue the chunk.
        shared.queue.push(chunk);
        let drive_start = !shared.start_started && now_truncated;
        if drive_start {
            shared.start_started = true;
        }
        drop(shared);
        if drive_start {
            start_spill(
                &self.spill,
                &self.capture,
                &self.env,
                &self.context,
                &self.kill,
            )
            .await;
        }
    }
}

fn spawn_reader(
    mut pipe: impl AsyncRead + Unpin + Send + 'static,
    state: Arc<RunState>,
    ended_flag: Arc<AtomicBool>,
    kill_readers: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut chunk = vec![0u8; READ_CHUNK];
            let bytes_read = tokio::select! {
                read = pipe.read(&mut chunk) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(bytes_read) => bytes_read,
                },
                _ = kill_readers.cancelled() => break,
            };
            state.data_seen.fetch_add(1, Ordering::SeqCst);
            chunk.truncate(bytes_read);
            state.feed(chunk).await;
        }
        ended_flag.store(true, Ordering::SeqCst);
        // Ending also counts as an arrival for the grace logic (upstream
        // `maybeFinalizeAfterExit`).
        state.data_seen.fetch_add(1, Ordering::SeqCst);
    })
}

/// Why the wait loop stopped (upstream: the exit + timeout + abort
/// interplay in `exec`'s promise body).
enum StopReason {
    Exit(std::process::ExitStatus),
    TimedOut,
    Aborted,
}

/// Upstream `exec`'s promise body (`nodejs.ts:458-697`), shared by every
/// `ExecutionEnv` implementor so the spill's `createTempFile` dispatch stays
/// virtual.
pub async fn exec_via(
    env: Arc<dyn ExecutionEnv>,
    runtime: ShellRuntime,
    command: &str,
    options: Option<&ShellExecOptions>,
    context: Context,
) -> Result<ShellExecResult, ExecutionError> {
    let signal = context.abort_signal();
    if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
        return Err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"));
    }
    let timeout_ms = resolve_timeout_ms(options.and_then(|options| options.timeout))?;

    let cwd = match options.and_then(|options| options.cwd.as_deref()) {
        Some(cwd) => resolve_path(env.cwd(), cwd),
        None => env.cwd().to_string(),
    };
    let shell_config = get_shell_config(runtime.shell_path.as_deref()).await?;
    if let Err(error) = tokio::fs::metadata(&cwd).await {
        let cause = to_file_error(error, None);
        return Err(ExecutionError::new(
            ExecutionErrorCode::SpawnError,
            format!("Working directory does not exist: {cwd}\nCannot execute bash commands."),
        )
        .with_cause(Some(Box::new(cause))));
    }

    let kill = Arc::new(ChildKill {
        pid: Mutex::new(None),
    });
    let callback_error: Arc<Mutex<Option<ExecutionError>>> = Arc::new(Mutex::new(None));
    let fail: FailFn = {
        let callback_error = Arc::clone(&callback_error);
        let kill = Arc::clone(&kill);
        Arc::new(move |message: String| {
            let mut slot = callback_error.lock().unwrap();
            if slot.is_some() {
                return;
            }
            *slot = Some(ExecutionError::new(
                ExecutionErrorCode::CallbackError,
                message,
            ));
            drop(slot);
            kill.kill();
        })
    };
    let capture = Arc::new(
        OutputCapture::new(
            options.and_then(|options| options.capture.as_ref()),
            context.clone(),
            OutputCaptureHandlers {
                on_update: options.and_then(|options| options.on_update.clone()),
                on_error: {
                    let fail = Arc::clone(&fail);
                    Arc::new(move |message: String| fail(message))
                },
            },
        )
        .map_err(|message| ExecutionError::new(ExecutionErrorCode::Unknown, message))?,
    );
    let spill_enabled = options
        .and_then(|options| options.capture.as_ref())
        .and_then(|capture| capture.spill)
        .unwrap_or(false);
    let spill: SharedSpill = Arc::new(tokio::sync::Mutex::new(SpillShared {
        queue: Vec::new(),
        path: None,
        sender: None,
        start_started: false,
        done: false,
        error: None,
    }));

    let spawn_result = spawn_child(&shell_config, &cwd, command, options, &runtime);
    let mut child = match spawn_result {
        Ok(child) => child,
        Err(error) => {
            return Err(
                ExecutionError::new(ExecutionErrorCode::SpawnError, error.0.to_string())
                    .with_cause(Some(Box::new(error.0))),
            );
        }
    };
    let pid = child.id();
    if let Some(pid) = pid {
        *kill.pid.lock().unwrap() = Some(pid);
        runtime.active_child_pids.lock().unwrap().insert(pid);
    }
    if shell_config.stdin_transport {
        // Upstream attaches a no-op error listener and ends stdin with the
        // command (`nodejs.ts:605-608`); write errors are best-effort.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(command.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }
    }

    // Reader tasks (upstream `child.stdout?.on("data", feed)` etc.).
    let kill_readers = CancellationToken::new();
    let stdout_ended = Arc::new(AtomicBool::new(false));
    let stderr_ended = Arc::new(AtomicBool::new(false));
    let state = Arc::new(RunState {
        env,
        context: context.clone(),
        capture: Arc::clone(&capture),
        spill: Arc::clone(&spill),
        spill_enabled,
        fail: Arc::clone(&fail),
        kill: Arc::clone(&kill),
        data_seen: std::sync::atomic::AtomicU64::new(0),
    });
    let mut reader_handles = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        reader_handles.push(spawn_reader(
            stdout,
            Arc::clone(&state),
            Arc::clone(&stdout_ended),
            kill_readers.clone(),
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        reader_handles.push(spawn_reader(
            stderr,
            Arc::clone(&state),
            Arc::clone(&stderr_ended),
            kill_readers.clone(),
        ));
    }

    // Wait for exit, timeout, or abort (upstream: the timeout timer, the
    // abort listener, and `waitForChildProcess`).
    let timed_out = AtomicBool::new(false);
    let timeout_pending = async {
        match timeout_ms {
            Some(timeout_ms) => tokio::time::sleep(Duration::from_millis(timeout_ms)).await,
            None => std::future::pending().await,
        }
    };
    let abort_pending = async {
        match &signal {
            Some(signal) => signal.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let stop = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => StopReason::Exit(status),
            Err(error) => {
                return Err(ExecutionError::new(ExecutionErrorCode::SpawnError, error.to_string()));
            }
        },
        _ = timeout_pending => {
            timed_out.store(true, Ordering::SeqCst);
            kill.kill();
            StopReason::TimedOut
        }
        _ = abort_pending => {
            kill.kill();
            StopReason::Aborted
        }
    };
    // Reap the killed child for the timeout/abort paths (upstream resolves
    // on `close` after the kill).
    let status = match stop {
        StopReason::Exit(status) => Some(status),
        StopReason::TimedOut | StopReason::Aborted => child.wait().await.ok(),
    };
    // Post-exit stdio grace (upstream `waitForChildProcess` +
    // `EXIT_STDIO_GRACE_MS`): wait for EOF, extending while data arrives or
    // the spill is draining, then destroy the pipes.
    let grace_deadline = Instant::now() + Duration::from_millis(EXIT_STDIO_GRACE_MS);
    let mut last_seen = state.data_seen.load(Ordering::SeqCst);
    let mut deadline = grace_deadline;
    loop {
        if stdout_ended.load(Ordering::SeqCst) && stderr_ended.load(Ordering::SeqCst) {
            break;
        }
        let now = Instant::now();
        if now >= deadline {
            let spill_draining = {
                let shared = spill.lock().await;
                shared.error.is_none() && shared.start_started && !shared.done
            };
            let seen = state.data_seen.load(Ordering::SeqCst);
            if spill_draining || seen != last_seen {
                last_seen = seen;
                deadline = now + Duration::from_millis(EXIT_STDIO_GRACE_MS);
            } else {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(GRACE_TICK_MS)).await;
    }
    // Destroy the streams (upstream `child.stdout?.destroy()`).
    kill_readers.cancel();
    for handle in reader_handles {
        let _ = tokio::time::timeout(Duration::from_millis(500), handle).await;
    }
    // Drop the spill channel's last sender so the writer task drains and
    // finishes (the stored clone would otherwise keep the channel open
    // forever).

    spill.lock().await.sender = None;

    // `finishSpill` (nodejs.ts:580-589): wait for the spill writer to drain.
    loop {
        let settled = {
            let shared = spill.lock().await;
            !shared.start_started || shared.done || shared.error.is_some()
        };
        if settled {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // `capture.finish(); capture.flush()` (nodejs.ts:659-661): a publish
    // panic here is a callback error (upstream try/catch feeds
    // `failCallback`).
    let flushed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture.finish();
        capture.flush();
    }));
    if let Err(error) = flushed {
        fail(panic_message(error.as_ref()));
    }

    // Settle (nodejs.ts:663-695): priority order callback > timeout >
    // aborted > spill > result.
    // Hoisted so the std guard cannot live across the spill lock's await
    // below (the future must stay Send).
    let callback_failure = callback_error.lock().unwrap().take();
    let settled = if let Some(error) = callback_failure {
        Err(error)
    } else if timed_out.load(Ordering::SeqCst) {
        let timeout = options.and_then(|options| options.timeout);
        Err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            format!("timeout:{timeout:?}"),
        ))
    } else if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
        Err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"))
    } else if let Some(error) = spill.lock().await.error.take() {
        Err(error)
    } else {
        let output = capture.snapshot();
        // A process killed by a signal (e.g. OOM killer) has no exit code;
        // map it to the conventional 128 + signal number so callers do not
        // mistake it for a successful exit (nodejs.ts:682-684).
        let exit_code = status.and_then(|status| exit_code_of(&status)).unwrap_or(1);
        Ok(ShellExecResult {
            metadata: output.metadata,
            exit_code,
        })
    };

    if let Some(pid) = pid {
        runtime.active_child_pids.lock().unwrap().remove(&pid);
    }
    capture.dispose();
    settled
}

/// The spawn failure carries the backend error so the `spawn_error` keeps
/// its cause (upstream `toError(error)`).
struct SpawnFailure(io::Error);

fn spawn_child(
    shell_config: &ShellConfig,
    cwd: &str,
    command: &str,
    options: Option<&ShellExecOptions>,
    runtime: &ShellRuntime,
) -> Result<tokio::process::Child, SpawnFailure> {
    let env_map = get_shell_env(
        runtime.shell_env.as_ref(),
        options.and_then(|options| options.env.as_ref()),
        options
            .and_then(|options| options.inherit_env)
            .unwrap_or(true),
    );
    let mut cmd = tokio::process::Command::new(&shell_config.shell);
    // commandFromStdin ? args : [...args, command] (nodejs.ts:592-595).
    if shell_config.stdin_transport {
        cmd.args(&shell_config.args);
    } else {
        cmd.args(
            shell_config
                .args
                .iter()
                .cloned()
                .chain([command.to_string()]),
        );
    }
    cmd.current_dir(cwd)
        .env_clear()
        .envs(env_map)
        .stdin(if shell_config.stdin_transport {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // `detached: process.platform !== "win32"` (nodejs.ts:598): the child
    // becomes its own process-group leader so tree kills reach descendants.
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.spawn().map_err(SpawnFailure)
}

/// Upstream `code ?? (exitSignal ? 128 + signals[exitSignal] : 1)`
/// (`nodejs.ts:684`).
#[cfg(unix)]
fn exit_code_of(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

#[cfg(not(unix))]
fn exit_code_of(status: &std::process::ExitStatus) -> Option<i32> {
    status.code()
}

#[cfg(test)]
mod tests;
