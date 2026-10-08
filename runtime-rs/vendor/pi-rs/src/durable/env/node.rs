//! Port of `src/env/node.ts`: the std-filesystem / child-process
//! [`ExecutionEnv`] implementation (`NodeExecutionEnv`).
//!
//! Divergences (structural, disclosed):
//!
//! - **D3 extension (sync).** The upstream implementation drives Node's
//!   promise IO with stream backpressure; the port is synchronous
//!   (`std::fs`, `std::process`, reader threads). Spill writes therefore
//!   never backlog, so the `WriteStream` high-water-mark machinery collapses
//!   into direct appends; chunk boundaries, thresholds, prefix replay, and
//!   every resulting byte are unchanged.
//! - **D11 (process tree).** Upstream kills children through a detached
//!   process group (`SIGKILL` to `-pid`); the port cannot call `pre_exec`
//!   (`no unsafe`), so on Windows it uses `taskkill /F /T /PID` exactly like
//!   upstream and on Unix it signals the direct child PID only. A descendant
//!   that survives its parent holding stdio open is ended by the
//!   post-exit 100 ms idle grace, as upstream.
//! - **D10 inheritance.** IO failure message text comes from
//!   `std::io::Error`'s `Display` rather than Node's errno strings (see
//!   [`super`] D10); the `FileErrorCode` mapping mirrors `toFileError`'s
//!   errno switch (`NotFound`→`not_found`, `PermissionDenied`→
//!   `permission_denied`, `NotADirectory`→`not_directory`,
//!   `IsADirectory`→`is_directory`, `InvalidInput`→`invalid`).
//! - **D12 (realpath shape).** `std::fs::canonicalize` returns extended-length
//!   paths on Windows; the port strips the `\\?\` / `\\?\UNC\` prefix so the
//!   result matches Node's `realpath` shape.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rand::RngExt;

use crate::agent_core::chord_support::context::Context;

use super::{
    ExecutionEnv, ExecutionError, ExecutionErrorCode, FileContent, FileError, FileErrorCode,
    FileInfo, FileKind, FileSystem, Shell, ShellExecOptions, ShellExecResult, ShellSpillOptions,
    TempFileOptions, TextLine, TextLineReader,
};

const MAX_TIMEOUT_MS: f64 = 2_147_483_647.0;
const MAX_TIMEOUT_SECONDS: f64 = MAX_TIMEOUT_MS / 1000.0;
const EXIT_STDIO_GRACE_MS: u64 = 100;
const SPILL_CHUNK: usize = 64 * 1024;
const CHUNK_SIZE: usize = 64 * 1024;

fn is_windows() -> bool {
    cfg!(windows)
}

/// `resolveTimeoutMs` (`env/node.ts:45-56`).
fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<f64>, ExecutionError> {
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
    if timeout_ms > MAX_TIMEOUT_MS {
        return Err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            format!(
                "Invalid timeout: maximum is {}",
                js_number(MAX_TIMEOUT_SECONDS)
            ),
        ));
    }
    Ok(Some(timeout_ms))
}

/// JavaScript's default number-to-string for the timeout / retry texts.
pub(crate) fn js_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `fileURLToPath` for the `file://` forms `resolvePath` accepts; malformed
/// URLs keep their ordinary-path treatment.
fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Strip the authority (empty or localhost), then percent-decode.
    let index = rest.find('/')?;
    let (authority, path) = (&rest[..index], &rest[index..]);
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let decoded = percent_decode(path);
    // `file:///C:/x` → `C:/x`; a POSIX file URL keeps its leading slash.
    #[cfg(windows)]
    {
        let trimmed = decoded.trim_start_matches('/');
        if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
            return Some(PathBuf::from(trimmed.replace('/', "\\")));
        }
        Some(PathBuf::from(decoded))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Node `path.resolve(base, path)` for the forms this seam accepts.
fn node_resolve(base: &str, path: &str) -> String {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        Path::new(base).join(path)
    };
    node_normalize(&joined)
}

/// Node `path.normalize`: collapse `.` / `..`, keep the root or prefix.
fn node_normalize(path: &Path) -> String {
    let text = path.to_string_lossy();
    let starts_with_sep = text.starts_with('\\') || text.starts_with('/');
    // Windows drive or UNC prefix, or a POSIX root.
    let (prefix, rest) = match text.find(':') {
        Some(index)
            if index >= 1 && matches!(text.as_bytes().get(index + 1), Some(b'\\') | Some(b'/')) =>
        {
            let mut prefix = text[..=index].to_string();
            prefix.push(text.as_bytes()[index + 1] as char);
            (prefix, &text[index + 2..])
        }
        _ if starts_with_sep => (text[..1].to_string(), &text[1..]),
        _ => (String::new(), &text[..]),
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in rest
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty())
    {
        match segment {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut joined = prefix.clone();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            joined.push(if prefix.ends_with('\\') || prefix.contains(':') {
                '\\'
            } else {
                '/'
            });
        }
        joined.push_str(part);
    }
    if joined.is_empty() {
        joined.push_str(if prefix.is_empty() {
            "."
        } else {
            prefix.trim_end_matches(['\\', '/'])
        });
        if joined.is_empty() {
            joined.push('.');
        }
    }
    joined
}

fn path_separator(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// Node `path.join(...parts)`.
fn node_join(parts: &[&str]) -> String {
    let mut joined = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if Path::new(part).is_absolute() || joined.is_empty() {
            joined = (*part).to_string();
        } else {
            let separator = if joined.is_empty() {
                '/'
            } else {
                path_separator(&joined)
            };
            joined.push(separator);
            joined.push_str(part);
        }
    }
    node_normalize(Path::new(&joined))
}

fn path_exists(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

/// `toFileError` (`env/node.ts:104-128`) over `std::io::Error`.
fn to_file_error(error: &std::io::Error, fallback_path: Option<String>) -> FileError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => FileErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => FileErrorCode::PermissionDenied,
        std::io::ErrorKind::NotADirectory => FileErrorCode::NotDirectory,
        std::io::ErrorKind::IsADirectory => FileErrorCode::IsDirectory,
        std::io::ErrorKind::InvalidInput => FileErrorCode::Invalid,
        _ => FileErrorCode::Unknown,
    };
    FileError::new(code, error.to_string(), fallback_path)
}

fn abort_result<T>(context: &Context, path: Option<String>) -> Option<Result<T, FileError>> {
    if let Some(signal) = context.abort_signal() {
        if signal.is_cancelled() {
            return Some(Err(FileError::aborted(path)));
        }
    }
    None
}

/// Either piped stream of a spawned child, read uniformly.
enum ReaderStream {
    Stdout(std::process::ChildStdout),
    Stderr(std::process::ChildStderr),
}

impl Read for ReaderStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            ReaderStream::Stdout(stream) => stream.read(buf),
            ReaderStream::Stderr(stream) => stream.read(buf),
        }
    }
}

fn file_info_from_stats(path: &str, metadata: &std::fs::Metadata) -> Result<FileInfo, FileError> {
    let kind = if metadata.is_file() {
        FileKind::File
    } else if metadata.is_dir() {
        FileKind::Directory
    } else if metadata.is_symlink() {
        FileKind::Symlink
    } else {
        return Err(FileError::new(
            FileErrorCode::Invalid,
            "Unsupported file type",
            Some(path.to_string()),
        ));
    };
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    Ok(FileInfo {
        name: Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string(),
        kind,
        size: metadata.len(),
        mtime_ms,
    })
}

/// `resolvePath` (`env/node.ts:58-72`).
fn resolve_path(cwd: &str, path: &str) -> String {
    if path == "~" {
        return node_normalize(&home_dir());
    }
    if path.starts_with("~/") || (is_windows() && path.starts_with("~\\")) {
        let expanded = home_dir().join(&path[2..]);
        return node_normalize(&expanded);
    }
    let mut normalized = path.to_string();
    if path.starts_with("file://") {
        if let Some(converted) = file_url_to_path(path) {
            normalized = converted.to_string_lossy().into_owned();
        }
    }
    if Path::new(&normalized).is_absolute() {
        node_normalize(Path::new(&normalized))
    } else {
        node_resolve(cwd, &normalized)
    }
}

/// Incremental UTF-8 decoder matching `TextDecoder({stream: true})`: chunks
/// may split characters; the trailing incomplete sequence stays buffered and
/// the final flush replaces it with U+FFFD.
#[derive(Default)]
struct StreamDecoder {
    pending: Vec<u8>,
}

impl StreamDecoder {
    fn decode(&mut self, chunk: &[u8], stream: bool) -> String {
        self.pending.extend_from_slice(chunk);
        let text = match std::str::from_utf8(&self.pending) {
            Ok(_) => {
                let text = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                text
            }
            Err(error) => {
                let valid = error.valid_up_to();
                let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                self.pending.drain(..valid);
                text
            }
        };
        if !stream && !self.pending.is_empty() {
            let tail = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            return format!("{text}{tail}");
        }
        text
    }

    /// Final flush (`decoder.decode()`): the incomplete tail becomes U+FFFD.
    fn finish(&mut self) -> String {
        self.decode(&[], false)
    }
}

/// Strict LF reader (`NodeTextLineReader`); explicit byte offsets let an
/// aborted read be retried without skipping bytes.
struct StdTextLineReader {
    file: Mutex<Option<std::fs::File>>,
    path: String,
    decoder: Mutex<StreamDecoder>,
    buffered: Mutex<String>,
    byte_offset: AtomicU64,
    ended: Mutex<bool>,
    closed: Mutex<bool>,
}

impl StdTextLineReader {
    fn new(file: std::fs::File, path: String) -> Self {
        StdTextLineReader {
            file: Mutex::new(Some(file)),
            path,
            decoder: Mutex::new(StreamDecoder::default()),
            buffered: Mutex::new(String::new()),
            byte_offset: AtomicU64::new(0),
            ended: Mutex::new(false),
            closed: Mutex::new(false),
        }
    }
}

impl TextLineReader for StdTextLineReader {
    fn read_line(&self, context: &Context) -> Result<Option<TextLine>, FileError> {
        if let Some(aborted) = abort_result::<Option<TextLine>>(context, Some(self.path.clone())) {
            return aborted;
        }
        if *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err(FileError::new(
                FileErrorCode::Invalid,
                "Text line reader is closed",
                Some(self.path.clone()),
            ));
        }
        loop {
            {
                let mut buffered = self
                    .buffered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(newline) = buffered.find('\n') {
                    let text = buffered[..newline].to_string();
                    buffered.drain(..newline + 1);
                    return Ok(Some(TextLine {
                        text,
                        terminated: true,
                    }));
                }
                if *self
                    .ended
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                {
                    if buffered.is_empty() {
                        return Ok(None);
                    }
                    let text = std::mem::take(&mut *buffered);
                    return Ok(Some(TextLine {
                        text,
                        terminated: false,
                    }));
                }
            }
            let mut chunk = vec![0u8; CHUNK_SIZE];
            let bytes_read = {
                let mut guard = self
                    .file
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let file = guard.as_mut().ok_or_else(|| {
                    FileError::new(
                        FileErrorCode::Invalid,
                        "Text line reader is closed",
                        Some(self.path.clone()),
                    )
                })?;
                file.seek(SeekFrom::Start(self.byte_offset.load(Ordering::SeqCst)))
                    .map_err(|error| to_file_error(&error, Some(self.path.clone())))?;
                file.read(&mut chunk)
                    .map_err(|error| to_file_error(&error, Some(self.path.clone())))?
            };
            if let Some(aborted) =
                abort_result::<Option<TextLine>>(context, Some(self.path.clone()))
            {
                return aborted;
            }
            self.byte_offset
                .fetch_add(bytes_read as u64, Ordering::SeqCst);
            let text = self
                .decoder
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .decode(&chunk[..bytes_read], true);
            self.buffered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_str(&text);
            if bytes_read == 0 {
                let finish = self
                    .decoder
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .finish();
                let mut buffered = self
                    .buffered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                buffered.push_str(&finish);
                *self
                    .ended
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            }
        }
    }

    fn close(&self) {
        if *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return;
        }
        *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.buffered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        // Closing is best-effort, including after cancellation or an earlier I/O failure.
        *self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// `ShellConfig` (`env/node.ts:188-192`).
#[derive(Debug, Clone)]
struct ShellConfig {
    shell: String,
    args: Vec<&'static str>,
    command_transport_stdin: bool,
}

/// `isLegacyWslBashPath` (`env/node.ts:194-197`): matches
/// `[a-z]:\windows\(system32|sysnative)\bash.exe` after slash normalization.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let bytes = normalized.as_bytes();
    let matches = |suffix: &str| {
        let full = format!("x:{}", suffix);
        bytes.len() == full.len()
            && bytes.first().is_some_and(|byte| byte.is_ascii_lowercase())
            && normalized.ends_with(suffix)
    };
    matches(r":\windows\system32\bash.exe") || matches(r":\windows\sysnative\bash.exe")
}

fn get_bash_shell_config(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-s"],
            command_transport_stdin: true,
        }
    } else {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-c"],
            command_transport_stdin: false,
        }
    }
}

/// `runCommand` (`env/node.ts:143-176`): spawn and capture stdout, `status`
/// `None` when the command could not run.
fn run_command(command: &str, args: &[&str], timeout_ms: u64) -> (String, Option<i32>) {
    let child = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => return (String::new(), None),
    };
    let mut stdout = String::new();
    let mut handle = child.stdout.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = stream.read_to_end(&mut buffer);
            String::from_utf8_lossy(&buffer).into_owned()
        })
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                if let Some(handle) = handle.take() {
                    let _ = handle.join();
                }
                return (String::new(), None);
            }
        }
    };
    if let Some(handle) = handle.take() {
        if let Ok(text) = handle.join() {
            stdout = text;
        }
    }
    (stdout, status)
}

/// `findBashOnPath` (`env/node.ts:178-186`).
fn find_bash_on_path() -> Option<String> {
    let (stdout, status) = if is_windows() {
        run_command("where", &["bash.exe"], 5000)
    } else {
        run_command("which", &["bash"], 5000)
    };
    if status != Some(0) || stdout.is_empty() {
        return None;
    }
    let first_match = stdout.trim().split(['\r', '\n']).next()?.to_string();
    if first_match.is_empty() || !path_exists(&first_match) {
        return None;
    }
    Some(first_match)
}

/// `getShellConfig` (`env/node.ts:203-245`).
fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, ExecutionError> {
    if let Some(custom) = custom_shell_path {
        if path_exists(custom) {
            return Ok(get_bash_shell_config(custom));
        }
        return Err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            format!("Custom shell path not found: {custom}"),
        ));
    }
    if is_windows() {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(program_files_x86)
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        for candidate in &candidates {
            if path_exists(candidate) {
                return Ok(get_bash_shell_config(candidate));
            }
        }
        if let Some(bash_on_path) = find_bash_on_path() {
            return Ok(get_bash_shell_config(&bash_on_path));
        }
        let searched = candidates
            .iter()
            .map(|path| format!("  {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            "No bash shell found. Options:\n".to_string()
                + "  1. Install Git for Windows: https://git-scm.com/download/win\n"
                + "  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n"
                + "  3. Configure an explicit shellPath\n\n"
                + &format!("Searched Git Bash in:\n{searched}"),
        ));
    }
    if path_exists("/bin/bash") {
        return Ok(get_bash_shell_config("/bin/bash"));
    }
    if let Some(bash_on_path) = find_bash_on_path() {
        return Ok(get_bash_shell_config(&bash_on_path));
    }
    Ok(ShellConfig {
        shell: String::from("sh"),
        args: vec!["-c"],
        command_transport_stdin: false,
    })
}

/// `getShellEnv` (`env/node.ts:247-258`): base process env, then the
/// instance's `shellEnv`, then the call's extra env.
fn get_shell_env(
    shell_env: Option<&Vec<(String, String)>>,
    extra_env: Option<&Vec<(String, String)>>,
    inherit_env: bool,
) -> Vec<(String, String)> {
    if !inherit_env {
        return extra_env.cloned().unwrap_or_default();
    }
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    if let Some(shell_env) = shell_env {
        for (name, value) in shell_env {
            upsert(&mut env, name, value);
        }
    }
    if let Some(extra) = extra_env {
        for (name, value) in extra {
            upsert(&mut env, name, value);
        }
    }
    env
}

fn upsert(env: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = env.iter_mut().find(|(key, _)| key == name) {
        slot.1 = value.to_string();
    } else {
        env.push((name.to_string(), value.to_string()));
    }
}

/// `killProcessTree` (`env/node.ts:260-289`): `taskkill /F /T` on Windows, a
/// direct child `SIGKILL` on Unix (D11).
pub(crate) fn kill_process_tree(pid: u32) {
    if is_windows() {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let taskkill = PathBuf::from(&system_root)
            .join("System32")
            .join("taskkill.exe");
        let _ = std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    } else {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// `randomUUID` (`node:crypto`): a random RFC 4122 version 4 UUID.
fn random_uuid() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `mkdtemp`: prefix plus six random characters, like Node's implementation.
fn make_temp_dir(prefix: &str) -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir();
    for _ in 0..32 {
        let suffix: String = (0..6)
            .map(|_| {
                let index = rand::rng().random_range(0..26);
                (b'a' + index) as char
            })
            .collect();
        let candidate = base.join(format!("{prefix}{suffix}"));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not create a temporary directory",
    ))
}

/// The shared state of one `exec` run: what the reader threads report and how
/// the run settles.
struct ExecRun {
    on_output: Option<super::OnOutput>,
    callback_error: Mutex<Option<ExecutionError>>,
    spill_error: Mutex<Option<ExecutionError>>,
    spill_path: Mutex<Option<String>>,
    spill: Mutex<SpillState>,
    progress: AtomicU64,
    settled: Mutex<bool>,
}

struct SpillState {
    options: Option<ShellSpillOptions>,
    prefix: Vec<String>,
    seen_bytes: usize,
    seen_newlines: usize,
    started: Option<std::fs::File>,
}

impl ExecRun {
    /// `emit` (`env/node.ts:511-518`): no output reaches the caller after
    /// exec settled, and a throwing callback ends the run.
    fn emit(&self, text: &str) {
        if *self
            .settled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            || text.is_empty()
        {
            return;
        }
        let Some(on_output) = &self.on_output else {
            return;
        };
        if self
            .callback_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            return;
        }
        let mut callback = on_output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let invoked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(text)));
        if let Err(error) = invoked {
            // A throwing callback ends the run (`failCallback`).
            let message = error
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| error.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| String::from("output callback failed"));
            self.fail_callback(&message);
        }
    }

    fn fail_callback(&self, error: &str) {
        let mut guard = self
            .callback_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_some() {
            return;
        }
        *guard = Some(ExecutionError::new(
            ExecutionErrorCode::CallbackError,
            error.to_string(),
        ));
    }

    /// `feed` (`env/node.ts:629-647`): decode, emit, and route to the spill.
    fn feed(&self, decoder: &Mutex<StreamDecoder>, chunk: &[u8]) {
        let text = decoder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decode(chunk, true);
        self.emit(&text);
        let mut spill = self
            .spill
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(options) = spill.options else {
            return;
        };
        if chunk.is_empty() {
            return;
        }
        if spill.started.is_some() {
            self.write_spill(&mut spill, &text);
            return;
        }
        spill.seen_bytes += chunk.len();
        for byte in chunk {
            if *byte == 0x0a {
                spill.seen_newlines += 1;
            }
        }
        let lines = spill.seen_newlines + usize::from(*chunk.last().unwrap_or(&0) != 0x0a);
        if spill.seen_bytes <= options.after_bytes && lines <= options.after_lines {
            spill.prefix.push(text);
            return;
        }
        let prefix = std::mem::take(&mut spill.prefix);
        for part in &prefix {
            self.start_spill(&mut spill, part);
        }
        self.start_spill(&mut spill, &text);
    }

    /// `startSpill`: create the spill file lazily, then write the queued
    /// prefix and every later chunk (D3 extension: no backpressure).
    fn start_spill(&self, spill: &mut SpillState, chunk: &str) {
        if spill.started.is_some() {
            self.write_spill(spill, chunk);
            return;
        }
        spill.prefix.push(chunk.to_string());
        // The first crossing creates the file; a failure ends the run's spill.
        let dir = match make_temp_dir("tmp-") {
            Ok(dir) => dir,
            Err(error) => {
                self.fail_spill(&error.to_string());
                return;
            }
        };
        let path = dir.join(format!("pi-output-{}.log", random_uuid()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        match file {
            Ok(mut file) => {
                let queued = std::mem::take(&mut spill.prefix);
                for part in &queued {
                    let _ = file.write_all(part.as_bytes());
                }
                spill.started = Some(file);
                *self
                    .spill_path
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(path.to_string_lossy().into_owned());
            }
            Err(error) => self.fail_spill(&error.to_string()),
        }
    }

    fn write_spill(&self, spill: &mut SpillState, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if let Some(file) = spill.started.as_mut() {
            if let Err(error) = file.write_all(chunk.as_bytes()) {
                self.fail_spill(&error.to_string());
            }
        }
    }

    fn fail_spill(&self, cause: &str) {
        let mut guard = self
            .spill_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_some() {
            return;
        }
        *guard = Some(ExecutionError::new(
            ExecutionErrorCode::Unknown,
            format!("Failed to preserve complete shell output: {cause}"),
        ));
    }
}

/// The std-filesystem / bash implementation of [`ExecutionEnv`] (upstream
/// `NodeExecutionEnv`).
pub struct NodeExecutionEnv {
    cwd: String,
    shell_path: Option<String>,
    shell_env: Option<Vec<(String, String)>>,
    active_child_pids: Mutex<HashSet<u32>>,
}

impl NodeExecutionEnv {
    pub fn new(options: NodeExecutionEnvOptions) -> Self {
        NodeExecutionEnv {
            cwd: options.cwd,
            shell_path: options.shell_path,
            shell_env: options.shell_env,
            active_child_pids: Mutex::new(HashSet::new()),
        }
    }

    fn resolve(&self, path: &str) -> String {
        resolve_path(&self.cwd, path)
    }

    /// `openTextLineReader` on an already-resolved path.
    fn open_reader(
        &self,
        resolved: &str,
        context: &Context,
    ) -> Result<Box<dyn TextLineReader>, FileError> {
        if let Some(aborted) =
            abort_result::<Box<dyn TextLineReader>>(context, Some(resolved.to_string()))
        {
            return aborted;
        }
        match std::fs::File::open(resolved) {
            Ok(file) => {
                if let Some(aborted) =
                    abort_result::<Box<dyn TextLineReader>>(context, Some(resolved.to_string()))
                {
                    return aborted;
                }
                Ok(Box::new(StdTextLineReader::new(file, resolved.to_string())))
            }
            Err(error) => Err(to_file_error(&error, Some(resolved.to_string()))),
        }
    }
}

/// Constructor options of [`NodeExecutionEnv`] (`env/node.ts:443-447`).
pub struct NodeExecutionEnvOptions {
    pub cwd: String,
    pub shell_path: Option<String>,
    pub shell_env: Option<Vec<(String, String)>>,
}

impl FileSystem for NodeExecutionEnv {
    /// Every local environment sees the same files (v1.0.0).
    fn id(&self) -> &str {
        "node:local"
    }

    fn cwd(&self) -> &str {
        &self.cwd
    }

    fn absolute_path(&self, path: &str, _context: &Context) -> Result<String, FileError> {
        Ok(self.resolve(path))
    }

    fn join_path(&self, parts: &[&str], _context: &Context) -> Result<String, FileError> {
        Ok(node_join(parts))
    }

    fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<String>(context, Some(resolved.clone())) {
            return aborted;
        }
        std::fs::read_to_string(&resolved).map_err(|error| to_file_error(&error, Some(resolved)))
    }

    fn open_text_line_reader(
        &self,
        path: &str,
        context: &Context,
    ) -> Result<Box<dyn TextLineReader>, FileError> {
        let resolved = self.resolve(path);
        self.open_reader(&resolved, context)
    }

    fn read_text_lines(
        &self,
        path: &str,
        options: Option<super::TextLinesOptions>,
        context: &Context,
    ) -> Result<Vec<String>, FileError> {
        if let Some(max_lines) = options.and_then(|options| options.max_lines) {
            if max_lines == 0 {
                return Ok(Vec::new());
            }
        }
        let resolved = self.resolve(path);
        let reader = self.open_reader(&resolved, context)?;
        let mut lines = Vec::new();
        let max_lines = options.and_then(|options| options.max_lines);
        while max_lines.is_none() || lines.len() < max_lines.unwrap() {
            let line = reader.read_line(context)?;
            let Some(line) = line else {
                break;
            };
            lines.push(line.text);
        }
        reader.close();
        Ok(lines)
    }

    fn read_binary_file(&self, path: &str, context: &Context) -> Result<Vec<u8>, FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<Vec<u8>>(context, Some(resolved.clone())) {
            return aborted;
        }
        std::fs::read(&resolved).map_err(|error| to_file_error(&error, Some(resolved)))
    }

    fn write_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let parent = node_resolve(&resolved, "..");
        std::fs::create_dir_all(&parent)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        std::fs::write(&resolved, content.as_bytes())
            .map_err(|error| to_file_error(&error, Some(resolved)))
    }

    fn append_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let parent = node_resolve(&resolved, "..");
        std::fs::create_dir_all(&parent)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        file.write_all(&content.as_bytes())
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        Ok(())
    }

    fn truncate_file(&self, path: &str, size: u64, context: &Context) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        // `size` is unsigned, so the "non-negative safe integer" rejection of
        // `env/node.ts:792-794` is unreachable here.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        file.set_len(size)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        Ok(())
    }

    fn flush_file(&self, path: &str, context: &Context) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        file.sync_all()
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        Ok(())
    }

    fn rename_file(
        &self,
        source_path: &str,
        destination_path: &str,
        context: &Context,
    ) -> Result<(), FileError> {
        let source = self.resolve(source_path);
        let destination = self.resolve(destination_path);
        if let Some(aborted) = abort_result::<()>(context, Some(destination.clone())) {
            return aborted;
        }
        std::fs::rename(&source, &destination).map_err(|error| to_file_error(&error, Some(source)))
    }

    fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<FileInfo>(context, Some(resolved.clone())) {
            return aborted;
        }
        let metadata = std::fs::symlink_metadata(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        file_info_from_stats(&resolved, &metadata)
    }

    fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<Vec<FileInfo>>(context, Some(resolved.clone())) {
            return aborted;
        }
        let entries = std::fs::read_dir(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        let mut infos = Vec::new();
        for entry in entries {
            if let Some(aborted) = abort_result::<Vec<FileInfo>>(context, Some(resolved.clone())) {
                return aborted;
            }
            let entry = entry.map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
            let entry_path = node_resolve(&resolved, &entry.file_name().to_string_lossy());
            let metadata = std::fs::symlink_metadata(&entry_path)
                .map_err(|error| to_file_error(&error, Some(entry_path.clone())))?;
            infos.push(file_info_from_stats(&entry_path, &metadata)?);
        }
        Ok(infos)
    }

    fn canonical_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<String>(context, Some(resolved.clone())) {
            return aborted;
        }
        let canonical = std::fs::canonicalize(&resolved)
            .map_err(|error| to_file_error(&error, Some(resolved.clone())))?;
        Ok(strip_extended_prefix(&canonical.to_string_lossy()))
    }

    fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError> {
        match self.file_info(path, context) {
            Ok(_) => Ok(true),
            Err(error) if error.code == FileErrorCode::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn create_dir(
        &self,
        path: &str,
        options: Option<super::CreateDirOptions>,
        context: &Context,
    ) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let recursive = options
            .and_then(|options| options.recursive)
            .unwrap_or(true);
        let result = if recursive {
            std::fs::create_dir_all(&resolved)
        } else {
            std::fs::create_dir(&resolved)
        };
        result.map_err(|error| to_file_error(&error, Some(resolved)))
    }

    fn remove(
        &self,
        path: &str,
        options: Option<super::RemoveOptions>,
        context: &Context,
    ) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        if let Some(aborted) = abort_result::<()>(context, Some(resolved.clone())) {
            return aborted;
        }
        let recursive = options
            .and_then(|options| options.recursive)
            .unwrap_or(false);
        let force = options.and_then(|options| options.force).unwrap_or(false);
        let metadata = match std::fs::symlink_metadata(&resolved) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(to_file_error(&error, Some(resolved))),
        };
        let Some(metadata) = metadata else {
            return if force {
                Ok(())
            } else {
                Err(to_file_error(
                    &std::io::Error::from(std::io::ErrorKind::NotFound),
                    Some(resolved),
                ))
            };
        };
        let result = if metadata.is_dir() && !metadata.is_symlink() && recursive {
            std::fs::remove_dir_all(&resolved)
        } else if metadata.is_dir() && !metadata.is_symlink() {
            std::fs::remove_dir(&resolved)
        } else {
            std::fs::remove_file(&resolved)
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) if force && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(to_file_error(&error, Some(resolved))),
        }
    }

    fn create_temp_dir(
        &self,
        prefix: Option<&str>,
        context: &Context,
    ) -> Result<String, FileError> {
        if let Some(aborted) = abort_result::<String>(context, None) {
            return aborted;
        }
        let prefix = prefix.unwrap_or("tmp-");
        make_temp_dir(prefix)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| to_file_error(&error, None))
    }

    fn create_temp_file(
        &self,
        options: Option<TempFileOptions>,
        context: &Context,
    ) -> Result<String, FileError> {
        let dir = self.create_temp_dir(Some("tmp-"), context)?;
        let options = options.unwrap_or_default();
        let file_path = node_join(&[
            &dir,
            &format!(
                "{}{}{}",
                options.prefix.unwrap_or_default(),
                random_uuid(),
                options.suffix.unwrap_or_default()
            ),
        ]);
        std::fs::write(&file_path, "")
            .map_err(|error| to_file_error(&error, Some(file_path.clone())))?;
        Ok(file_path)
    }

    fn cleanup(&self) {
        let pids: HashSet<u32> = self
            .active_child_pids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for pid in pids {
            kill_process_tree(pid);
        }
        self.active_child_pids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

/// Strip the Windows extended-length prefix so canonical paths match Node's
/// `realpath` shape (D12).
fn strip_extended_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

impl Shell for NodeExecutionEnv {
    fn exec(
        &self,
        command: &str,
        options: Option<ShellExecOptions>,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError> {
        let options = options.unwrap_or_default();
        if let Some(signal) = context.abort_signal() {
            if signal.is_cancelled() {
                return Err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"));
            }
        }
        let timeout_ms = resolve_timeout_ms(options.timeout)?;
        let cwd = match &options.cwd {
            Some(cwd) => resolve_path(&self.cwd, cwd),
            None => self.cwd.clone(),
        };
        let shell_config = get_shell_config(self.shell_path.as_deref())?;
        if let Err(error) = std::fs::metadata(&cwd) {
            let _cause = error.to_string();
            return Err(ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                format!("Working directory does not exist: {cwd}\nCannot execute bash commands."),
            ));
        }

        let run = Arc::new(ExecRun {
            on_output: options.on_output.clone(),
            callback_error: Mutex::new(None),
            spill_error: Mutex::new(None),
            spill_path: Mutex::new(None),
            spill: Mutex::new(SpillState {
                options: options.spill,
                prefix: Vec::new(),
                seen_bytes: 0,
                seen_newlines: 0,
                started: None,
            }),
            progress: AtomicU64::new(0),
            settled: Mutex::new(false),
        });

        let mut shell_command = std::process::Command::new(&shell_config.shell);
        shell_command
            .args(&shell_config.args)
            .current_dir(&cwd)
            .env_clear()
            .envs(get_shell_env(
                self.shell_env.as_ref(),
                options.env.as_ref(),
                options.inherit_env.unwrap_or(true),
            ))
            .stdin(if shell_config.command_transport_stdin {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = match shell_command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(ExecutionError::new(
                    ExecutionErrorCode::SpawnError,
                    error.to_string(),
                ));
            }
        };
        let pid = child.id();
        self.active_child_pids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(pid);
        if shell_config.command_transport_stdin {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(command.as_bytes());
                // Dropping closes stdin, so the `-s` shell runs the command.
            }
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut readers = Vec::new();
        // One decoder per stream, so a character split across chunks of one
        // stream survives interleaving.
        for stream in [
            stdout.map(ReaderStream::Stdout),
            stderr.map(ReaderStream::Stderr),
        ]
        .into_iter()
        .flatten()
        {
            let run = Arc::clone(&run);
            readers.push(std::thread::spawn(move || {
                let decoder = Mutex::new(StreamDecoder::default());
                let mut buffer = [0u8; SPILL_CHUNK];
                let mut reader = stream;
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            run.progress.fetch_add(1, Ordering::SeqCst);
                            run.feed(&decoder, &buffer[..read]);
                        }
                        Err(_) => break,
                    }
                }
                run.emit(
                    &decoder
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .finish(),
                );
            }));
        }

        // Timeout / abort: kill the tree, then collect the exit status.
        let timed_out = AtomicU64::new(0);
        let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms as u64));
        let status_code: Option<i32> = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    #[cfg_attr(not(unix), allow(unused_mut))]
                    let mut code = status.code();
                    #[cfg(unix)]
                    if code.is_none() {
                        use std::os::unix::process::ExitStatusExt;
                        if let Some(signal) = status.signal() {
                            // A killed process has no exit code; map it to the
                            // conventional 128 + signal number.
                            code = Some(128 + signal);
                        }
                    }
                    break code;
                }
                Ok(None) => {
                    if let Some(signal) = context.abort_signal() {
                        if signal.is_cancelled() {
                            kill_process_tree(pid);
                        }
                    }
                    if let Some(deadline) = deadline {
                        if Instant::now() >= deadline && timed_out.swap(1, Ordering::SeqCst) == 0 {
                            kill_process_tree(pid);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    self.active_child_pids
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&pid);
                    return Err(ExecutionError::new(
                        ExecutionErrorCode::SpawnError,
                        error.to_string(),
                    ));
                }
            }
        };

        // Post-exit stdio grace: after the child is gone, wait for the
        // readers while they make progress, but at most 100 ms of idleness
        // before finalizing (a descendant may hold the pipes open; D11).
        let mut last_progress = run.progress.load(Ordering::SeqCst);
        let mut idle = Instant::now();
        let mut joined_all = false;
        while idle.elapsed() < Duration::from_millis(EXIT_STDIO_GRACE_MS) {
            let done = readers.iter().all(|reader| reader.is_finished());
            if done {
                joined_all = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            let progress = run.progress.load(Ordering::SeqCst);
            if progress != last_progress {
                last_progress = progress;
                idle = Instant::now();
            }
        }
        // No output reaches the caller after exec settled; unfinished reader
        // threads are dropped from the run's perspective.
        *run.settled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        if joined_all {
            for reader in readers {
                let _ = reader.join();
            }
        }
        self.active_child_pids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&pid);

        if let Some(callback_error) = run
            .callback_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Err(callback_error);
        }
        let aborted = context
            .abort_signal()
            .is_some_and(|signal| signal.is_cancelled());
        let timed_out = timed_out.load(Ordering::SeqCst) == 1;
        let spill_path = run
            .spill_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let spill_error = run
            .spill_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        drop(run);
        if timed_out || aborted {
            let mut error = if timed_out {
                let seconds = options.timeout.unwrap_or(0.0);
                ExecutionError::new(
                    ExecutionErrorCode::Timeout,
                    format!("timeout:{}", js_number(seconds)),
                )
            } else {
                ExecutionError::new(ExecutionErrorCode::Aborted, "aborted")
            };
            error.spill_path = spill_path;
            return Err(error);
        }
        if let Some(spill_error) = spill_error {
            return Err(spill_error);
        }
        Ok(ShellExecResult {
            exit_code: status_code.unwrap_or(1),
            spill_path,
        })
    }

    fn cleanup(&self) {
        FileSystem::cleanup(self);
    }
}

impl ExecutionEnv for NodeExecutionEnv {}
