//! Port of `src/env/index.ts`: the portable filesystem and shell capability
//! seam (`ExecutionEnv`) every IO flows through, plus its failure taxonomy.
//!
//! Divergences (structural, disclosed, continuing the phase-1 numbering):
//!
//! - **D3 (sync storage; extended to the env seam).** Upstream every
//!   `FileSystem` / `Shell` / `TextLineReader` method is `async` and takes the
//!   chord `Context` for cancellation; the port's methods are synchronous and
//!   take `&Context`, whose abort signal interruptible operations honor at
//!   their entry (the port has no interleaved async IO to cancel mid-flight).
//!   The `Result<T, E>` envelope maps onto [`std::result::Result`] directly,
//!   so `ok` / `err` / `getOrThrow` collapse into ordinary `?` flow.
//! - **D10 (error text source).** Upstream `FileError.message` carries Node's
//!   errno text (`ENOENT: no such file or directory, open '...'`); the port's
//!   [`FileError`] carries `std::io::Error`'s `Display` text instead, with the
//!   [`FileErrorCode`] mapped from `io::ErrorKind` exactly like
//!   `env/node.ts` `toFileError` maps errno codes. Message bytes of the
//!   hand-raised failures (invalid size, closed reader, unavailable shell,
//!   timeouts) are byte-identical.
//!
//! `jsonl` (phase 1) still reads the filesystem through `std::fs` directly
//! because its IO predates this seam; re-pointing it is deferred (disclosed
//! in the module plan).

pub mod node;

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::agent_core::chord_support::context::Context;

/// How a [`FileError`] classifies its failure (`env/index.ts`
/// `FileErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileErrorCode {
    Aborted,
    NotFound,
    PermissionDenied,
    NotDirectory,
    IsDirectory,
    Invalid,
    NotSupported,
    Unknown,
}

impl FileErrorCode {
    /// The upstream discriminant string.
    pub fn as_str(&self) -> &'static str {
        match self {
            FileErrorCode::Aborted => "aborted",
            FileErrorCode::NotFound => "not_found",
            FileErrorCode::PermissionDenied => "permission_denied",
            FileErrorCode::NotDirectory => "not_directory",
            FileErrorCode::IsDirectory => "is_directory",
            FileErrorCode::Invalid => "invalid",
            FileErrorCode::NotSupported => "not_supported",
            FileErrorCode::Unknown => "unknown",
        }
    }
}

/// Filesystem failure returned instead of thrown (`env/index.ts`
/// `FileError`). `Display` is the upstream `error.message`.
#[derive(Debug, Clone)]
pub struct FileError {
    pub code: FileErrorCode,
    pub message: String,
    pub path: Option<String>,
}

impl FileError {
    pub fn new(code: FileErrorCode, message: impl Into<String>, path: Option<String>) -> Self {
        FileError {
            code,
            message: message.into(),
            path,
        }
    }

    /// `new FileError("aborted", "aborted", path)` (`env/node.ts`
    /// `abortResult`).
    pub fn aborted(path: Option<String>) -> Self {
        FileError::new(FileErrorCode::Aborted, "aborted", path)
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FileError {}

/// How an [`ExecutionError`] classifies its failure (`env/index.ts`
/// `ExecutionErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionErrorCode {
    Aborted,
    Timeout,
    ShellUnavailable,
    SpawnError,
    CallbackError,
    Unknown,
}

impl ExecutionErrorCode {
    /// The upstream discriminant string.
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionErrorCode::Aborted => "aborted",
            ExecutionErrorCode::Timeout => "timeout",
            ExecutionErrorCode::ShellUnavailable => "shell_unavailable",
            ExecutionErrorCode::SpawnError => "spawn_error",
            ExecutionErrorCode::CallbackError => "callback_error",
            ExecutionErrorCode::Unknown => "unknown",
        }
    }
}

/// Command execution failure returned instead of thrown (`env/index.ts`
/// `ExecutionError`).
#[derive(Debug, Clone)]
pub struct ExecutionError {
    pub code: ExecutionErrorCode,
    pub message: String,
    /// Spill file of a command that timed out or was aborted after its output
    /// crossed the spill thresholds.
    pub spill_path: Option<String>,
}

impl ExecutionError {
    pub fn new(code: ExecutionErrorCode, message: impl Into<String>) -> Self {
        ExecutionError {
            code,
            message: message.into(),
            spill_path: None,
        }
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExecutionError {}

/// One directory entry with its stat snapshot (`env/index.ts` `FileInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub mtime_ms: f64,
}

/// Entry kind of a [`FileInfo`] (`env/index.ts` `FileKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

/// One line read by a [`TextLineReader`] (`env/index.ts` `TextLine`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    pub text: String,
    pub terminated: bool,
}

/// Strict LF line reader (`env/index.ts` `TextLineReader`); reports whether
/// the final line was newline-terminated.
pub trait TextLineReader: Send + Sync {
    /// Next line, or `None` at end of file.
    fn read_line(&self, context: &Context) -> Result<Option<TextLine>, FileError>;
    /// Best-effort close, including after cancellation or an earlier failure.
    fn close(&self);
}

/// Portable filesystem capability (`env/index.ts` `FileSystem`). Operations
/// return failures rather than throwing.
pub trait FileSystem: Send + Sync {
    /// The file namespace (v1.0.0): equal ids see the same files at the same
    /// paths, whatever their `cwd`. Every local Node environment shares one
    /// id; each container or remote host has its own.
    fn id(&self) -> &str {
        "local"
    }
    fn cwd(&self) -> &str;
    fn absolute_path(&self, path: &str, context: &Context) -> Result<String, FileError>;
    fn join_path(&self, parts: &[&str], context: &Context) -> Result<String, FileError>;
    fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError>;
    fn open_text_line_reader(
        &self,
        path: &str,
        context: &Context,
    ) -> Result<Box<dyn TextLineReader>, FileError>;
    fn read_text_lines(
        &self,
        path: &str,
        options: Option<TextLinesOptions>,
        context: &Context,
    ) -> Result<Vec<String>, FileError>;
    fn read_binary_file(&self, path: &str, context: &Context) -> Result<Vec<u8>, FileError>;
    fn write_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError>;
    fn append_file(
        &self,
        path: &str,
        content: FileContent<'_>,
        context: &Context,
    ) -> Result<(), FileError>;
    /// Truncate or extend a file to exactly `size` bytes.
    fn truncate_file(&self, path: &str, size: u64, context: &Context) -> Result<(), FileError>;
    /// Flush file contents and metadata needed to retrieve them.
    fn flush_file(&self, path: &str, context: &Context) -> Result<(), FileError>;
    fn rename_file(
        &self,
        source_path: &str,
        destination_path: &str,
        context: &Context,
    ) -> Result<(), FileError>;
    fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError>;
    fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError>;
    fn canonical_path(&self, path: &str, context: &Context) -> Result<String, FileError>;
    fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError>;
    fn create_dir(
        &self,
        path: &str,
        options: Option<CreateDirOptions>,
        context: &Context,
    ) -> Result<(), FileError>;
    fn remove(
        &self,
        path: &str,
        options: Option<RemoveOptions>,
        context: &Context,
    ) -> Result<(), FileError>;
    fn create_temp_dir(&self, prefix: Option<&str>, context: &Context)
        -> Result<String, FileError>;
    fn create_temp_file(
        &self,
        options: Option<TempFileOptions>,
        context: &Context,
    ) -> Result<String, FileError>;
    fn cleanup(&self);
}

/// `readTextLines` options (`env/index.ts` `{ maxLines?: number }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLinesOptions {
    pub max_lines: Option<usize>,
}

/// `createDir` options (`env/index.ts` `{ recursive?: boolean }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateDirOptions {
    pub recursive: Option<bool>,
}

/// `remove` options (`env/index.ts` `{ recursive?: boolean; force?: boolean }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveOptions {
    pub recursive: Option<bool>,
    pub force: Option<bool>,
}

/// `createTempFile` options (`env/index.ts` `{ prefix?, suffix? }`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TempFileOptions {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// Text or binary file content (`env/index.ts` `string | Uint8Array`).
#[derive(Debug, Clone)]
pub enum FileContent<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
}

impl FileContent<'_> {
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            FileContent::Text(text) => text.as_bytes().to_vec(),
            FileContent::Binary(bytes) => bytes.to_vec(),
        }
    }
}

/// Spill the complete output to a temporary file once it exceeds either
/// threshold (`env/index.ts` `ShellSpillOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellSpillOptions {
    pub after_bytes: usize,
    /// Complete or partial lines.
    pub after_lines: usize,
}

/// Result of one [`Shell::exec`] (`env/index.ts` `ShellExecResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellExecResult {
    pub exit_code: i32,
    /// Temporary file holding the complete raw output, when the spill
    /// thresholds were exceeded.
    pub spill_path: Option<String>,
}

/// Output callback of [`ShellExecOptions`] (`env/index.ts` `onOutput`).
/// Shared so the stdout and stderr readers can both emit.
pub type OnOutput = Arc<Mutex<dyn FnMut(&str) + Send>>;

/// Options of [`Shell::exec`] (`env/index.ts` `ShellExecOptions`).
#[derive(Default)]
pub struct ShellExecOptions {
    pub cwd: Option<String>,
    pub env: Option<Vec<(String, String)>>,
    pub inherit_env: Option<bool>,
    /// Timeout in seconds.
    pub timeout: Option<f64>,
    /// Every decoded chunk of combined stdout and stderr as it arrives: raw,
    /// unbounded, and unthrottled.
    pub on_output: Option<OnOutput>,
    pub spill: Option<ShellSpillOptions>,
}

/// Shell command capability (`env/index.ts` `Shell`).
pub trait Shell: Send + Sync {
    fn exec(
        &self,
        command: &str,
        options: Option<ShellExecOptions>,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError>;
    fn cleanup(&self);
}

/// The environment seam offered to tools as `api.env` (`env/index.ts`
/// `ExecutionEnv`).
pub trait ExecutionEnv: FileSystem + Shell {}
