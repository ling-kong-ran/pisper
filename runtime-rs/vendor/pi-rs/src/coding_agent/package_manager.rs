//! Port of upstream `coding-agent` package management — slice M5 W3.9.
//!
//! Provenance map (upstream file → submodule), upstream SHA256 at migration
//! time:
//!
//! | upstream                        | submodule        | sha256 |
//! |---|---|---|
//! | `src/core/package-manager.ts`   | this module      | `e2fdd18093dc69cb8fe82bb6bef674f20f8a5093d0d13fd1b7180f49dbe751fd` |
//! | `src/package-manager-cli.ts`    | [`cli`]          | `8f75c01f9c2c2e07d2144eaef43a0739435844cacf28a6fd6b316692e510a1c0` |
//!
//! Both upstream sources are vendored byte-identical under
//! `tests/fixtures/pm_oracle/src/` and driven under node (type stripping) by the
//! capture scripts next to them; the captured determinism surfaces
//! (`core.oracle.json`, `cli.oracle.json`) are pinned by the tests here.
//! Vendored library behavior (npm `semver` 7.8.5 / `minimatch` 10.2.6 /
//! `ignore` 7.0.8 / node `fs.globSync`) lives in [`vendor`].
//!
//! ## Seams
//!
//! - **SettingsManager** (`core/settings-manager.ts`, not yet ported): the
//!   [`SettingsManagerHandle`] trait mirrors the consumed surface
//!   (`getGlobalSettings`/`getProjectSettings`/`setPackages`/
//!   `setProjectPackages`/`isProjectTrusted`/`getNpmCommand`); snapshot
//!   structs carry only the fields this module reads.
//! - **Child processes** (`utils/child-process.ts` + `cross-spawn`): the
//!   [`CommandRunner`] trait with the exact upstream error-message
//!   renderings; [`RealCommandRunner`] spawns argv arrays via
//!   `std::process::Command`. Tests inject scripted runners exactly like the
//!   upstream suite spies on `runCommand`/`runCommandCapture`/`runCommandSync`.
//! - **`readPiManifest`** (core/pi-manifest.ts): reused from the W3.5 port
//!   (`extensions::loader::read_pi_manifest`).
//! - **`isStdoutTakenOver`** (core/output-guard.ts, not ported): treated as
//!   `false` (spawns inherit stdout), matching the non-takeover default.
//! - **CLI externals** (`config.ts` identity/self-update helpers,
//!   `version-check.ts` network, `ModelRuntime`, config TUI, trust store,
//!   `proper-lockfile`, pi-tui `Markdown`): the [`cli::PackageCommandHost`]
//!   trait; defaults and test fakes reproduce the oracle-captured texts.
//!
//! ## Divergences
//!
//! 1. **Async dropped** (as in the W3.3 event-bus port): resolve/install/
//!    update are synchronous. `runWithConcurrency` keeps real concurrency
//!    via scoped threads, so batching/parallelism semantics are preserved.
//! 2. **Windows process launching**: upstream spawns through `cross-spawn`
//!    (which routes `npm` to `npm.cmd` via cmd.exe); the real runner spawns
//!    argv directly, so `.cmd` shims are not auto-resolved. Tests use the
//!    runner seam; argv fidelity (the "argv entries containing spaces" case)
//!    is std `Command`'s array contract.
//! 3. **`getEnv()` /proc fallback**: upstream re-reads `/proc/self/environ`
//!    on linux when the env is empty; std inherits the real environment, so
//!    the fallback is unreachable here.
//! 4. **Node stream ordering**: upstream resolves captured stdout on the
//!    stream `close` event (the "wait for close" suite case); the sync
//!    runner reads pipes to EOF before returning — same observable data.
//! 5. **semver `validRange` canonical string** is not produced (never
//!    observable on the ported surface; ranges feed `satisfies`/
//!    `maxSatisfying` directly).
//! 6. **Progress errors**: upstream propagates `error.message`; non-Error
//!    throws render via `String(error)` — the port's [`PmError`] always
//!    carries a message string.

#[cfg(test)]
#[path = "package_manager/pm_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "package_manager/cli_tests.rs"]
mod cli_tests;

pub mod cli;

#[path = "package_manager/vendor.rs"]
pub mod vendor;

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::coding_agent::core::CONFIG_DIR_NAME;
use crate::coding_agent::extensions::loader::read_pi_manifest;
use crate::coding_agent::utils::git::{parse_git_url, GitSource};
use crate::coding_agent::utils::paths::{
    canonicalize_path, is_local_path, mark_path_ignored_by_cloud_sync, resolve_path_with,
    PathInputOptions,
};
use crate::coding_agent::utils::text::strip_bom;

// ===========================================================================
// Constants / env
// ===========================================================================

const NETWORK_TIMEOUT_MS: u64 = 10000;
const UPDATE_CHECK_CONCURRENCY: usize = 4;
const GIT_UPDATE_CONCURRENCY: usize = 4;

/// Upstream `isOfflineModeEnabled()`.
pub fn is_offline_mode_enabled() -> bool {
    let Ok(value) = std::env::var("PI_OFFLINE") else {
        return false;
    };
    value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
}

/// Upstream `getHomeDir()`: `HOME` env, else `os.homedir()`.
fn get_home_dir() -> String {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return home;
        }
    }
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ===========================================================================
// Errors / progress / public data types
// ===========================================================================

/// JS `Error` with a message (rendered via `error.message` upstream).
#[derive(Debug, Clone)]
pub struct PmError {
    pub message: String,
}

impl PmError {
    pub fn new(message: impl Into<String>) -> PmError {
        PmError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PmError {}

pub type PmResult<T> = Result<T, PmError>;

/// Upstream `SourceScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

/// Upstream `PathMetadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMetadata {
    pub source: String,
    pub scope: SourceScope,
    /// `"package"` or `"top-level"`.
    pub origin: PathMetadataOrigin,
    pub base_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMetadataOrigin {
    Package,
    TopLevel,
}

/// Upstream `ResolvedResource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedResource {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
}

/// Upstream `ResolvedPaths`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPaths {
    pub extensions: Vec<ResolvedResource>,
    pub skills: Vec<ResolvedResource>,
    pub prompts: Vec<ResolvedResource>,
    pub themes: Vec<ResolvedResource>,
}

/// Upstream `MissingSourceAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSourceAction {
    Install,
    Skip,
    Error,
}

/// Upstream `ProgressEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    /// `"start" | "progress" | "complete" | "error"`.
    pub event_type: &'static str,
    /// `"install" | "remove" | "update" | "clone" | "pull"`.
    pub action: &'static str,
    pub source: String,
    pub message: Option<String>,
}

pub type ProgressCallback = Arc<dyn Fn(&ProgressEvent) + Send + Sync>;

/// Upstream `PackageUpdate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUpdate {
    pub source: String,
    pub display_name: String,
    /// `"npm" | "git"`.
    pub update_type: &'static str,
    pub scope: InstalledSourceScope,
}

/// Upstream `ConfiguredPackage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredPackage {
    pub source: String,
    pub scope: InstalledSourceScope,
    pub filtered: bool,
    pub installed_path: Option<String>,
}

pub type InstalledSourceScope = SourceScope; // "user" | "project"

// ===========================================================================
// Settings seam
// ===========================================================================

/// One entry of the settings `packages` array (upstream `PackageSource`:
/// string or filter object). Unknown object fields round-trip through
/// [`PackageFilterSpec::extra`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSourceEntry {
    Plain(String),
    Object(PackageFilterSpec),
}

impl PackageSourceEntry {
    /// Upstream `getPackageSourceString`.
    pub fn source(&self) -> &str {
        match self {
            PackageSourceEntry::Plain(source) => source,
            PackageSourceEntry::Object(spec) => &spec.source,
        }
    }
}

/// Upstream `PackageSource` object form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFilterSpec {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoload: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
    /// Preserved unknown fields (upstream spreads `{ ...existing, source }`).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PackageFilterSpec {
    /// Upstream `PackageFilter` view (the resource patterns of a package).
    fn as_filter(&self) -> PackageFilter {
        PackageFilter {
            autoload: self.autoload,
            extensions: self.extensions.clone(),
            skills: self.skills.clone(),
            prompts: self.prompts.clone(),
            themes: self.themes.clone(),
        }
    }
}

/// Upstream `PackageFilter`.
#[derive(Debug, Clone, Default)]
pub struct PackageFilter {
    pub autoload: Option<bool>,
    pub extensions: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
}

/// The settings fields this module consumes (snapshot of upstream `Settings`).
#[derive(Debug, Clone, Default)]
pub struct SettingsData {
    pub packages: Vec<PackageSourceEntry>,
    pub extensions: Vec<String>,
    pub skills: Vec<String>,
    pub prompts: Vec<String>,
    pub themes: Vec<String>,
    pub npm_command: Option<Vec<String>>,
}

/// Seam over upstream `SettingsManager` (the consumed surface only).
pub trait SettingsManagerHandle: Send + Sync {
    fn global_settings(&self) -> SettingsData;
    fn project_settings(&self) -> SettingsData;
    fn is_project_trusted(&self) -> bool;
    /// Upstream `setProjectTrusted` (CLI command context).
    fn set_project_trusted(&self, trusted: bool);
    fn npm_command(&self) -> Option<Vec<String>>;
    fn set_packages(&self, packages: Vec<PackageSourceEntry>);
    fn set_project_packages(&self, packages: Vec<PackageSourceEntry>);
}

// ===========================================================================
// Command-runner seam
// ===========================================================================

/// Child-process failure (carries the exact upstream message text).
#[derive(Debug, Clone)]
pub struct CommandError {
    pub message: String,
}

impl CommandError {
    pub fn new(message: impl Into<String>) -> CommandError {
        CommandError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Seam over upstream `spawnProcess`/`spawnProcessSync` (utils/child-process.ts).
pub trait CommandRunner: Send + Sync {
    /// `runCommand`: inherit stdio, resolve on exit 0.
    fn run(&self, command: &str, args: &[String], cwd: Option<&str>) -> Result<(), CommandError>;

    /// `runCommandCapture`: pipe stdout/stderr, resolve trimmed stdout.
    fn run_capture(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
        timeout_ms: Option<u64>,
        extra_env: &[(String, String)],
    ) -> Result<String, CommandError>;

    /// `runCommandSync`: synchronous, trimmed stdout (or stderr).
    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, CommandError>;
}

fn join_args_for_message(command: &str, args: &[String]) -> String {
    let mut out = String::from(command);
    for arg in args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// Default [`CommandRunner`] over `std::process::Command` (divergence 2:
/// no cross-spawn `.cmd` shim resolution).
pub struct RealCommandRunner;

impl RealCommandRunner {
    fn build(command: &str, args: &[String], cwd: Option<&str>) -> std::process::Command {
        let mut cmd = std::process::Command::new(command);
        cmd.args(args);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    fn read_stream(mut stream: impl Read, sink: &Arc<Mutex<String>>) {
        let mut buffer = String::new();
        let _ = stream.read_to_string(&mut buffer);
        if let Ok(mut guard) = sink.lock() {
            guard.push_str(&buffer);
        }
    }
}

impl CommandRunner for RealCommandRunner {
    fn run(&self, command: &str, args: &[String], cwd: Option<&str>) -> Result<(), CommandError> {
        let mut cmd = Self::build(command, args, cwd);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());
        match cmd.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(CommandError::new(format!(
                "{} failed with code {}",
                join_args_for_message(command, args),
                status.code().unwrap_or(-1)
            ))),
            Err(error) => Err(CommandError::new(error.to_string())),
        }
    }

    fn run_capture(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
        timeout_ms: Option<u64>,
        extra_env: &[(String, String)],
    ) -> Result<String, CommandError> {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let mut cmd = Self::build(command, args, cwd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => return Err(CommandError::new(error.to_string())),
        };
        let mut reader_handles = Vec::new();
        let stdout_sink = child.stdout.take().map(|stream| {
            let sink = Arc::new(Mutex::new(String::new()));
            let writer = Arc::clone(&sink);
            reader_handles.push(std::thread::spawn(move || {
                Self::read_stream(stream, &writer)
            }));
            sink
        });
        let stderr_sink = child.stderr.take().map(|stream| {
            let sink = Arc::new(Mutex::new(String::new()));
            let writer = Arc::clone(&sink);
            reader_handles.push(std::thread::spawn(move || {
                Self::read_stream(stream, &writer)
            }));
            sink
        });

        // Wait for exit, honoring the timeout (upstream kills the child and
        // rejects with `{command} {args} timed out after {timeoutMs}ms`).
        let deadline = timeout_ms.map(|timeout| Instant::now() + Duration::from_millis(timeout));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if let Some(deadline) = deadline {
                        if Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            for handle in reader_handles {
                                let _ = handle.join();
                            }
                            return Err(CommandError::new(format!(
                                "{} timed out after {}ms",
                                join_args_for_message(command, args),
                                timeout_ms.unwrap_or_default()
                            )));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(CommandError::new(error.to_string())),
            }
        };
        for handle in reader_handles {
            let _ = handle.join();
        }

        let stdout = stdout_sink
            .and_then(|sink| sink.lock().ok().map(|guard| guard.clone()))
            .unwrap_or_default();
        let stderr = stderr_sink
            .and_then(|sink| sink.lock().ok().map(|guard| guard.clone()))
            .unwrap_or_default();
        if status.success() {
            return Ok(stdout.trim().to_string());
        }
        let exit_status = match status.code() {
            Some(code) => format!("code {code}"),
            None => "signal unknown".to_string(),
        };
        Err(CommandError::new(format!(
            "{} failed with {}: {}",
            join_args_for_message(command, args),
            exit_status,
            if stderr.is_empty() { stdout } else { stderr }
        )))
    }

    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, CommandError> {
        let output = Self::build(command, args, None)
            .stdin(std::process::Stdio::null())
            .output();
        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                if !output.status.success() {
                    return Err(CommandError::new(format!(
                        "Failed to run {}: {}",
                        join_args_for_message(command, args),
                        if stderr.is_empty() { stdout } else { stderr }
                    )));
                }
                Ok(if stdout.is_empty() { stderr } else { stdout }
                    .trim()
                    .to_string())
            }
            Err(error) => Err(CommandError::new(format!(
                "Failed to run {}: {}",
                join_args_for_message(command, args),
                error
            ))),
        }
    }
}

// ===========================================================================
// Pure helpers (upstream order preserved)
// ===========================================================================

/// Upstream `resourcePrecedenceRank` (lower = higher precedence).
fn resource_precedence_rank(metadata: &PathMetadata) -> u8 {
    if metadata.origin == PathMetadataOrigin::Package {
        return 4;
    }
    let scope_base = match metadata.scope {
        SourceScope::Project => 0,
        _ => 2,
    };
    scope_base + u8::from(metadata.source != "local")
}

/// Upstream `isExactNpmVersion`.
fn is_exact_npm_version(version: Option<&str>) -> bool {
    version.is_some_and(|version| vendor::semver_valid(version).is_some())
}

/// Upstream `getNpmVersionRange`.
fn get_npm_version_range(version: Option<&str>) -> Option<vendor::Range> {
    let version = version?;
    if version.is_empty() {
        return None;
    }
    vendor::parse_range(version)
}

/// Upstream `toPosixPath`.
fn to_posix_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}

const RESOURCE_TYPES: [&str; 4] = ["extensions", "skills", "prompts", "themes"];

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// Upstream `getExtensionTempFolder`.
pub fn get_extension_temp_folder(agent_dir: &str) -> String {
    let temp_folder = node_join(agent_dir, &["tmp", "extensions"]);
    let _ = std::fs::create_dir_all(&temp_folder);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp_folder, std::fs::Permissions::from_mode(0o700));
    }
    temp_folder
}

/// Upstream `isPattern`.
fn is_pattern(entry: &str) -> bool {
    entry.starts_with('!')
        || entry.starts_with('+')
        || entry.starts_with('-')
        || entry.contains('*')
        || entry.contains('?')
}

/// Upstream `isOverridePattern`.
fn is_override_pattern(entry: &str) -> bool {
    entry.starts_with('!') || entry.starts_with('+') || entry.starts_with('-')
}

/// Upstream `hasGlobPattern`.
fn has_glob_pattern(entry: &str) -> bool {
    entry.contains('*') || entry.contains('?')
}

/// Upstream `expandPackageGlob`: glob entries discover visible paths; exact
/// entries can target dot paths or symlinked trees.
fn expand_package_glob(pattern: &str, root: &str) -> Vec<String> {
    vendor::glob_sync(pattern, root)
        .into_iter()
        .filter(|path| {
            node_relative(root, path)
                .split(node_sep())
                .all(|segment| segment == ".." || !segment.starts_with('.'))
        })
        .collect()
}

/// Upstream `splitPatterns`.
fn split_patterns(entries: &[String]) -> (Vec<String>, Vec<String>) {
    let mut plain = Vec::new();
    let mut patterns = Vec::new();
    for entry in entries {
        if is_pattern(entry) {
            patterns.push(entry.clone());
        } else {
            plain.push(entry.clone());
        }
    }
    (plain, patterns)
}

// ---------------------------------------------------------------------------
// node:path shims on the host platform
// ---------------------------------------------------------------------------

fn node_sep() -> &'static str {
    if cfg!(windows) {
        "\\"
    } else {
        "/"
    }
}

fn node_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

/// `path.join(...parts)` on the host platform.
pub(crate) fn node_join(base: &str, parts: &[&str]) -> String {
    let mut args: Vec<&str> = vec![base];
    args.extend_from_slice(parts);
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&args)
    } else {
        crate::coding_agent::utils::node_path::posix_join(&args)
    }
}

/// `path.resolve(...parts)` on the host platform.
pub(crate) fn node_resolve(parts: &[&str]) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_resolve(parts, &node_cwd())
    } else {
        crate::coding_agent::utils::node_path::posix_resolve(parts, &node_cwd())
    }
}

/// `path.resolve(base, ...parts)`.
pub(crate) fn node_resolve_under(base: &str, parts: &[&str]) -> String {
    let mut args: Vec<&str> = vec![base];
    args.extend_from_slice(parts);
    node_resolve(&args)
}

/// `path.dirname` on the host platform.
fn node_dirname(path: &str) -> String {
    node_resolve(&[path, ".."])
}

/// `path.basename` on the host platform.
fn node_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(node_sep());
    match trimmed.rsplit(node_sep()).next() {
        Some(base) => base.to_string(),
        None => trimmed.to_string(),
    }
}

/// `path.relative(from, to)` on the host platform.
pub(crate) fn node_relative(from: &str, to: &str) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_relative(from, to, &node_cwd())
    } else {
        crate::coding_agent::utils::node_path::posix_relative(from, to, &node_cwd())
    }
}

fn resolve_path_options() -> PathInputOptions {
    PathInputOptions {
        trim: true,
        expand_tilde: None,
        home_dir: Some(get_home_dir()),
        strip_at_prefix: false,
        normalize_unicode_spaces: false,
    }
}

// ===========================================================================
// Pattern engine (upstream applyPatterns family)
// ===========================================================================

/// Upstream `matchesAnyPattern`.
fn matches_any_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let rel = to_posix_path(&node_relative(base_dir, file_path));
    let name = node_basename(file_path);
    let file_path_posix = to_posix_path(file_path);
    let is_skill_file = name == "SKILL.md";
    let parent_dir = is_skill_file.then(|| node_dirname(file_path));
    let parent_rel = parent_dir
        .as_ref()
        .map(|parent| to_posix_path(&node_relative(base_dir, parent)));
    let parent_name = parent_dir.as_ref().map(|parent| node_basename(parent));
    let parent_dir_posix = parent_dir.as_ref().map(|parent| to_posix_path(parent));

    patterns.iter().any(|pattern| {
        let normalized_pattern = to_posix_path(pattern);
        if vendor::minimatch(&rel, &normalized_pattern)
            || vendor::minimatch(&name, &normalized_pattern)
            || vendor::minimatch(&file_path_posix, &normalized_pattern)
        {
            return true;
        }
        if !is_skill_file {
            return false;
        }
        vendor::minimatch(
            parent_rel.as_deref().unwrap_or_default(),
            &normalized_pattern,
        ) || vendor::minimatch(
            parent_name.as_deref().unwrap_or_default(),
            &normalized_pattern,
        ) || vendor::minimatch(
            parent_dir_posix.as_deref().unwrap_or_default(),
            &normalized_pattern,
        )
    })
}

/// Upstream `normalizeExactPattern`.
fn normalize_exact_pattern(pattern: &str) -> String {
    let normalized = pattern
        .strip_prefix("./")
        .or_else(|| pattern.strip_prefix(".\\"))
        .unwrap_or(pattern);
    to_posix_path(normalized)
}

/// Upstream `matchesAnyExactPattern`.
fn matches_any_exact_pattern(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let rel = to_posix_path(&node_relative(base_dir, file_path));
    let name = node_basename(file_path);
    let file_path_posix = to_posix_path(file_path);
    let is_skill_file = name == "SKILL.md";
    let parent_dir = is_skill_file.then(|| node_dirname(file_path));
    let parent_rel = parent_dir
        .as_ref()
        .map(|parent| to_posix_path(&node_relative(base_dir, parent)));
    let parent_dir_posix = parent_dir.as_ref().map(|parent| to_posix_path(parent));

    patterns.iter().any(|pattern| {
        let normalized = normalize_exact_pattern(pattern);
        if normalized == rel || normalized == file_path_posix {
            return true;
        }
        if !is_skill_file {
            return false;
        }
        normalized == parent_rel.as_deref().unwrap_or_default()
            || normalized == parent_dir_posix.as_deref().unwrap_or_default()
    })
}

/// Upstream `getOverridePatterns`.
fn get_override_patterns(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter(|pattern| {
            pattern.starts_with('!') || pattern.starts_with('+') || pattern.starts_with('-')
        })
        .cloned()
        .collect()
}

/// Upstream `isEnabledByOverrides`.
fn is_enabled_by_overrides(file_path: &str, patterns: &[String], base_dir: &str) -> bool {
    let overrides = get_override_patterns(patterns);
    let strip = |value: &str| value[1..].to_string();
    let excludes: Vec<String> = overrides
        .iter()
        .filter(|pattern| pattern.starts_with('!'))
        .map(|pattern| strip(pattern))
        .collect();
    let force_includes: Vec<String> = overrides
        .iter()
        .filter(|pattern| pattern.starts_with('+'))
        .map(|pattern| strip(pattern))
        .collect();
    let force_excludes: Vec<String> = overrides
        .iter()
        .filter(|pattern| pattern.starts_with('-'))
        .map(|pattern| strip(pattern))
        .collect();

    let mut enabled = true;
    if !excludes.is_empty() && matches_any_pattern(file_path, &excludes, base_dir) {
        enabled = false;
    }
    if !force_includes.is_empty() && matches_any_exact_pattern(file_path, &force_includes, base_dir)
    {
        enabled = true;
    }
    if !force_excludes.is_empty() && matches_any_exact_pattern(file_path, &force_excludes, base_dir)
    {
        enabled = false;
    }
    enabled
}

/// Upstream `applyPatterns`: plain patterns include, `!` excludes, `+`
/// force-includes (exact, overrides exclusions), `-` force-excludes (exact).
/// Returns the enabled set as an ordered list (upstream: `Set`).
fn apply_patterns(all_paths: &[String], patterns: &[String], base_dir: &str) -> Vec<String> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut force_includes = Vec::new();
    let mut force_excludes = Vec::new();

    for pattern in patterns {
        if let Some(rest) = pattern.strip_prefix('+') {
            force_includes.push(rest.to_string());
        } else if let Some(rest) = pattern.strip_prefix('-') {
            force_excludes.push(rest.to_string());
        } else if let Some(rest) = pattern.strip_prefix('!') {
            excludes.push(rest.to_string());
        } else {
            includes.push(pattern.clone());
        }
    }

    // Step 1: includes (or all when none).
    let mut result: Vec<String> = if includes.is_empty() {
        all_paths.to_vec()
    } else {
        all_paths
            .iter()
            .filter(|path| matches_any_pattern(path, &includes, base_dir))
            .cloned()
            .collect()
    };

    // Step 2: excludes.
    if !excludes.is_empty() {
        result.retain(|path| !matches_any_pattern(path, &excludes, base_dir));
    }

    // Step 3: force-include (add back from all_paths).
    if !force_includes.is_empty() {
        for path in all_paths {
            if !result.contains(path) && matches_any_exact_pattern(path, &force_includes, base_dir)
            {
                result.push(path.clone());
            }
        }
    }

    // Step 4: force-exclude.
    if !force_excludes.is_empty() {
        result.retain(|path| !matches_any_exact_pattern(path, &force_excludes, base_dir));
    }

    result
}

/// Upstream `applyAutoloadDisabledPatterns`.
fn apply_autoload_disabled_patterns(
    all_paths: &[String],
    patterns: &[String],
    base_dir: &str,
) -> Vec<(String, bool)> {
    let mut result: Vec<(String, bool)> = Vec::new();
    for pattern in patterns {
        let target = pattern
            .strip_prefix('+')
            .or_else(|| pattern.strip_prefix('-'))
            .or_else(|| pattern.strip_prefix('!'))
            .unwrap_or(pattern)
            .to_string();
        let enabled = !pattern.starts_with('-') && !pattern.starts_with('!');
        let exact = pattern.starts_with('+') || pattern.starts_with('-');
        for file_path in all_paths {
            let matched = if exact {
                matches_any_exact_pattern(file_path, std::slice::from_ref(&target), base_dir)
            } else {
                matches_any_pattern(file_path, std::slice::from_ref(&target), base_dir)
            };
            if matched {
                // Map.set semantics: overwrite the value, keep position.
                if let Some(existing) = result.iter_mut().find(|(path, _)| path == file_path) {
                    existing.1 = enabled;
                } else {
                    result.push((file_path.clone(), enabled));
                }
            }
        }
    }
    result
}

// ===========================================================================
// Filesystem discovery (upstream collectors)
// ===========================================================================

/// Gitignore rules collected pre-order over a tree (upstream builds the
/// matcher incrementally during its DFS; the collected rule sequence is
/// identical, and gitignore matching is order-only).
struct IgnoreRules {
    root: String,
    matcher: vendor::IgnoreMatcher,
}

impl IgnoreRules {
    /// Scan `root` (existing directory) collecting `.gitignore`/`.ignore`/
    /// `.fdignore` rules with upstream prefixing.
    fn scan(root: &str) -> IgnoreRules {
        let mut rules = Vec::new();
        collect_ignore_rules_preorder(Path::new(root), root, &mut rules);
        IgnoreRules {
            root: root.to_string(),
            matcher: vendor::IgnoreMatcher::from_rules(root, &rules),
        }
    }

    fn ignores(&self, relative_path: &str, is_dir: bool) -> bool {
        let absolute = node_resolve_under(&self.root, &[relative_path]);
        self.matcher.ignores(&absolute, is_dir)
    }
}

fn collect_ignore_rules_preorder(dir: &Path, root: &str, rules: &mut Vec<String>) {
    let relative_dir = node_relative(root, &dir.to_string_lossy());
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{}/", to_posix_path(&relative_dir))
    };
    for filename in IGNORE_FILE_NAMES {
        let ignore_path = dir.join(filename);
        if !ignore_path.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&ignore_path) {
            for line in content.split('\n') {
                let line = line.trim_end_matches('\r');
                if let Some(pattern) = vendor::prefix_ignore_pattern(line, &prefix) {
                    rules.push(pattern);
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let child = dir.join(&name);
        if entry_kind_of(&child) == Some(EntryKind::Dir) {
            collect_ignore_rules_preorder(&child, root, rules);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Dir,
}

fn entry_kind_of(path: &Path) -> Option<EntryKind> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Some(EntryKind::Dir),
        Ok(_) => Some(EntryKind::File),
        Err(_) => None,
    }
}

/// One `readdirSync(dir, { withFileTypes: true })` entry with symlink
/// resolution (upstream `statSync` follows symlinks).
struct DirEntryInfo {
    name: String,
    path: String,
    kind: Option<EntryKind>,
}

fn read_dir_entries(dir: &str) -> Vec<DirEntryInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = node_join(dir, &[&name]);
            let symlink_kind = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.file_type().is_symlink());
            let kind = match symlink_kind {
                Some(_) => entry_kind_of(Path::new(&path)),
                None => match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => Some(EntryKind::Dir),
                    Ok(file_type) if file_type.is_file() => Some(EntryKind::File),
                    _ => None,
                },
            };
            DirEntryInfo { name, path, kind }
        })
        .collect()
}

/// Upstream `collectFiles`.
fn collect_files(
    dir: &str,
    resource_type: &str,
    skip_node_modules: bool,
    rules: Option<&IgnoreRules>,
    root_dir: &str,
) -> Vec<String> {
    let mut files = Vec::new();
    if !std::path::Path::new(dir).exists() {
        return files;
    }
    let owned_rules;
    let rules = match rules {
        Some(rules) => rules,
        None => {
            owned_rules = IgnoreRules::scan(root_dir);
            &owned_rules
        }
    };

    for entry in read_dir_entries(dir) {
        if entry.name.starts_with('.') {
            continue;
        }
        if skip_node_modules && entry.name == "node_modules" {
            continue;
        }
        let Some(kind) = entry.kind else {
            continue;
        };
        let rel_path = to_posix_path(&node_relative(root_dir, &entry.path));
        let ignore_path = if kind == EntryKind::Dir {
            format!("{rel_path}/")
        } else {
            rel_path
        };
        if rules.ignores(&ignore_path, kind == EntryKind::Dir) {
            continue;
        }
        if kind == EntryKind::Dir {
            files.extend(collect_files(
                &entry.path,
                resource_type,
                skip_node_modules,
                Some(rules),
                root_dir,
            ));
        } else if matches_extension_pattern(&entry.name, resource_type) {
            files.push(entry.path);
        }
    }
    files
}

/// The upstream `FILE_PATTERNS` regexes restricted to the resource corpus:
/// extensions `\.(ts|js)$`, skills/prompts `\.md$`, themes `\.json$`.
fn matches_extension_pattern(name: &str, resource_type: &str) -> bool {
    match resource_type {
        "extensions" => name.ends_with(".ts") || name.ends_with(".js"),
        "skills" | "prompts" => name.ends_with(".md"),
        "themes" => name.ends_with(".json"),
        _ => false,
    }
}

type SkillDiscoveryMode = &'static str; // "pi" | "agents"

/// Upstream `collectSkillEntries`.
fn collect_skill_entries(
    dir: &str,
    mode: SkillDiscoveryMode,
    rules: Option<&IgnoreRules>,
    root: &str,
) -> Vec<String> {
    let mut entries = Vec::new();
    if !std::path::Path::new(dir).exists() {
        return entries;
    }
    let owned_rules;
    let rules = match rules {
        Some(rules) => rules,
        None => {
            owned_rules = IgnoreRules::scan(root);
            &owned_rules
        }
    };

    let dir_entries = read_dir_entries(dir);
    for entry in &dir_entries {
        if entry.name != "SKILL.md" {
            continue;
        }
        if entry.kind == Some(EntryKind::File) {
            let rel_path = to_posix_path(&node_relative(root, &entry.path));
            if !rules.ignores(&rel_path, false) {
                entries.push(entry.path.clone());
                return entries;
            }
        }
    }

    for entry in &dir_entries {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let Some(kind) = entry.kind else {
            continue;
        };
        let rel_path = to_posix_path(&node_relative(root, &entry.path));
        let should_include_markdown_file = kind == EntryKind::File
            && entry.name.ends_with(".md")
            && !rules.ignores(&rel_path, false)
            && ((mode == "pi" && dir == root) || (mode == "agents" && dir != root));
        if should_include_markdown_file {
            entries.push(entry.path.clone());
            continue;
        }
        if kind != EntryKind::Dir {
            continue;
        }
        if rules.ignores(&format!("{rel_path}/"), true) {
            continue;
        }
        entries.extend(collect_skill_entries(&entry.path, mode, Some(rules), root));
    }
    entries
}

fn collect_auto_skill_entries(dir: &str, mode: SkillDiscoveryMode) -> Vec<String> {
    collect_skill_entries(dir, mode, None, dir)
}

/// Upstream `findGitRepoRoot`.
fn find_git_repo_root(start_dir: &str) -> Option<String> {
    let mut dir = node_resolve(&[start_dir]);
    loop {
        if std::path::Path::new(&node_join(&dir, &[".git"])).exists() {
            return Some(dir);
        }
        let parent = node_dirname(&dir);
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// Upstream `collectAncestorAgentsSkillDirs`.
fn collect_ancestor_agents_skill_dirs(start_dir: &str) -> Vec<String> {
    let mut skill_dirs = Vec::new();
    let resolved_start_dir = node_resolve(&[start_dir]);
    let git_repo_root = find_git_repo_root(&resolved_start_dir);

    let mut dir = resolved_start_dir;
    loop {
        skill_dirs.push(node_join(&dir, &[".agents", "skills"]));
        if Some(&dir) == git_repo_root.as_ref() {
            break;
        }
        let parent = node_dirname(&dir);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    skill_dirs
}

/// Upstream `collectAutoPromptEntries`.
fn collect_auto_prompt_entries(dir: &str) -> Vec<String> {
    let mut entries = Vec::new();
    if !std::path::Path::new(dir).exists() {
        return entries;
    }
    let rules = IgnoreRules::scan(dir);
    for entry in read_dir_entries(dir) {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let Some(kind) = entry.kind else {
            continue;
        };
        let rel_path = to_posix_path(&node_relative(dir, &entry.path));
        if rules.ignores(&rel_path, kind == EntryKind::Dir) {
            continue;
        }
        if kind == EntryKind::File && entry.name.ends_with(".md") {
            entries.push(entry.path);
        }
    }
    entries
}

/// Upstream `collectAutoThemeEntries`.
fn collect_auto_theme_entries(dir: &str) -> Vec<String> {
    let mut entries = Vec::new();
    if !std::path::Path::new(dir).exists() {
        return entries;
    }
    let rules = IgnoreRules::scan(dir);
    for entry in read_dir_entries(dir) {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let Some(kind) = entry.kind else {
            continue;
        };
        let rel_path = to_posix_path(&node_relative(dir, &entry.path));
        if rules.ignores(&rel_path, kind == EntryKind::Dir) {
            continue;
        }
        if kind == EntryKind::File && entry.name.ends_with(".json") {
            entries.push(entry.path);
        }
    }
    entries
}

/// Upstream `resolveExtensionEntries`: `package.json` `pi.extensions`, else
/// `index.ts`, else `index.js`. `None` when nothing applies.
fn resolve_extension_entries(dir: &str) -> Option<Vec<String>> {
    let package_json_path = node_join(dir, &["package.json"]);
    if std::path::Path::new(&package_json_path).exists() {
        let manifest = read_pi_manifest(&package_json_path);
        if let Some(manifest) = manifest {
            let declared = manifest.extensions.clone().unwrap_or_default();
            if !declared.is_empty() {
                let mut entries = Vec::new();
                for ext_path in &declared {
                    let resolved_ext_path = node_resolve_under(dir, &[ext_path]);
                    if std::path::Path::new(&resolved_ext_path).exists() {
                        entries.push(resolved_ext_path);
                    }
                }
                if !entries.is_empty() {
                    return Some(entries);
                }
            }
        }
    }

    let index_ts = node_join(dir, &["index.ts"]);
    let index_js = node_join(dir, &["index.js"]);
    if std::path::Path::new(&index_ts).exists() {
        return Some(vec![index_ts]);
    }
    if std::path::Path::new(&index_js).exists() {
        return Some(vec![index_js]);
    }

    None
}

/// Upstream `collectAutoExtensionEntries`.
fn collect_auto_extension_entries(dir: &str) -> Vec<String> {
    let mut entries = Vec::new();
    if !std::path::Path::new(dir).exists() {
        return entries;
    }

    // The directory itself may declare explicit entries.
    if let Some(root_entries) = resolve_extension_entries(dir) {
        return root_entries;
    }

    let rules = IgnoreRules::scan(dir);
    for entry in read_dir_entries(dir) {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let Some(kind) = entry.kind else {
            continue;
        };
        let rel_path = to_posix_path(&node_relative(dir, &entry.path));
        let ignore_path = if kind == EntryKind::Dir {
            format!("{rel_path}/")
        } else {
            rel_path
        };
        if rules.ignores(&ignore_path, kind == EntryKind::Dir) {
            continue;
        }
        if kind == EntryKind::File && (entry.name.ends_with(".ts") || entry.name.ends_with(".js")) {
            entries.push(entry.path);
        } else if kind == EntryKind::Dir {
            if let Some(resolved_entries) = resolve_extension_entries(&entry.path) {
                entries.extend(resolved_entries);
            }
        }
    }
    entries
}

/// Upstream `collectResourceFiles`.
fn collect_resource_files(dir: &str, resource_type: &str) -> Vec<String> {
    if resource_type == "skills" {
        return collect_skill_entries(dir, "pi", None, dir);
    }
    if resource_type == "extensions" {
        return collect_auto_extension_entries(dir);
    }
    collect_files(dir, resource_type, true, None, dir)
}

// ===========================================================================
// Parsed sources
// ===========================================================================

/// Upstream `ParsedSource`.
#[derive(Debug, Clone)]
pub enum ParsedSource {
    Npm(NpmSource),
    Git(GitSource),
    Local(LocalSource),
}

/// Upstream `NpmSource`.
#[derive(Debug, Clone)]
pub struct NpmSource {
    pub spec: String,
    pub name: String,
    pub version: Option<String>,
    pub range: Option<vendor::Range>,
    pub pinned: bool,
}

/// Upstream `LocalSource`.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalSource {
    pub path: String,
}

/// Upstream `parseNpmSpec` (`/^(@?[^@]+(?:\/[^@]+)?)(?:@(.+))?$/`).
fn parse_npm_spec(spec: &str) -> (String, Option<String>) {
    let (prefix_len, body) = match spec.strip_prefix('@') {
        Some(body) => (1usize, body),
        None => (0usize, spec),
    };
    let Some(at_index) = body.find('@') else {
        // No version separator: the whole spec is the name (greedy `[^@]+`
        // covers slashes).
        return (spec.to_string(), None);
    };
    let name_part = &body[..at_index];
    let version = &body[at_index + 1..];
    if name_part.is_empty() || version.is_empty() {
        // No regex match: upstream falls back to `{ name: spec }`.
        return (spec.to_string(), None);
    }
    (
        format!("{}{}", &spec[..prefix_len], name_part),
        Some(version.to_string()),
    )
}

// ===========================================================================
// Ordered accumulator
// ===========================================================================

struct AccumulatorEntry {
    metadata: PathMetadata,
    enabled: bool,
}

/// Insertion-ordered map (upstream `Map`).
#[derive(Default)]
struct OrderedMap {
    entries: Vec<(String, AccumulatorEntry)>,
    index: HashMap<String, usize>,
}

impl OrderedMap {
    fn has(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    fn add(&mut self, path: &str, metadata: PathMetadata, enabled: bool) {
        if path.is_empty() || self.has(path) {
            return;
        }
        self.index.insert(path.to_string(), self.entries.len());
        self.entries
            .push((path.to_string(), AccumulatorEntry { metadata, enabled }));
    }
}

#[derive(Default)]
struct ResourceAccumulator {
    extensions: OrderedMap,
    skills: OrderedMap,
    prompts: OrderedMap,
    themes: OrderedMap,
}

impl ResourceAccumulator {
    fn target(&mut self, resource_type: &str) -> &mut OrderedMap {
        match resource_type {
            "extensions" => &mut self.extensions,
            "skills" => &mut self.skills,
            "prompts" => &mut self.prompts,
            "themes" => &mut self.themes,
            _ => unreachable!("unknown resource type"),
        }
    }
}

// ===========================================================================
// DefaultPackageManager
// ===========================================================================

/// Upstream `PackageManagerOptions`.
pub struct PackageManagerOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub settings_manager: Arc<dyn SettingsManagerHandle>,
    pub command_runner: Option<Arc<dyn CommandRunner>>,
}

/// Upstream `DefaultPackageManager`.
pub struct DefaultPackageManager {
    cwd: String,
    agent_dir: String,
    settings_manager: Arc<dyn SettingsManagerHandle>,
    runner: Arc<dyn CommandRunner>,
    global_npm_root: Mutex<Option<(String, String)>>,
    progress_callback: Mutex<Option<ProgressCallback>>,
}

/// `runWithConcurrency` worker pool (scoped threads; divergence 1 keeps the
/// upstream batching/parallelism semantics with synchronous tasks).
fn run_with_concurrency<'a, T: Send + 'a>(
    tasks: Vec<Box<dyn FnOnce() -> T + Send + 'a>>,
    limit: usize,
) -> Vec<T> {
    if tasks.is_empty() {
        return Vec::new();
    }
    let task_count = tasks.len();
    type TaskSlot<'a, T> = Option<Box<dyn FnOnce() -> T + Send + 'a>>;
    let tasks: Mutex<Vec<TaskSlot<'a, T>>> = Mutex::new(tasks.into_iter().map(Some).collect());
    let tasks_ref = &tasks;
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..task_count).map(|_| None).collect());
    let results_ref = &results;
    let next_index = AtomicUsize::new(0);
    let next_index_ref = &next_index;
    let worker_count = limit.max(1).min(task_count);
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(move || loop {
                let index = next_index_ref.fetch_add(1, AtomicOrdering::SeqCst);
                if index >= task_count {
                    return;
                }
                let task = tasks_ref
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard[index].take());
                let Some(task) = task else {
                    return;
                };
                let value = task();
                if let Ok(mut guard) = results_ref.lock() {
                    guard[index] = Some(value);
                }
            });
        }
    });
    let mut guard = results.into_inner().unwrap_or_default();
    guard
        .drain(..)
        .map(|slot| slot.unwrap_or_else(|| panic!("missing task result")))
        .collect()
}

impl DefaultPackageManager {
    pub fn new(options: PackageManagerOptions) -> DefaultPackageManager {
        let cwd = resolve_path_with(
            &options.cwd,
            &node_cwd(),
            &PathInputOptions::default(),
            cfg!(windows),
        )
        .unwrap_or_else(|_| options.cwd.clone());
        let agent_dir = resolve_path_with(
            &options.agent_dir,
            &node_cwd(),
            &PathInputOptions::default(),
            cfg!(windows),
        )
        .unwrap_or_else(|_| options.agent_dir.clone());
        DefaultPackageManager {
            cwd,
            agent_dir,
            settings_manager: options.settings_manager,
            runner: options
                .command_runner
                .unwrap_or_else(|| Arc::new(RealCommandRunner)),
            global_npm_root: Mutex::new(None),
            progress_callback: Mutex::new(None),
        }
    }

    /// Upstream `setProgressCallback`.
    pub fn set_progress_callback(&self, callback: Option<ProgressCallback>) {
        *self
            .progress_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback;
    }

    fn emit_progress(&self, event: ProgressEvent) {
        if let Some(callback) = self
            .progress_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            callback(&event);
        }
    }

    fn with_progress<T>(
        &self,
        action: &'static str,
        source: &str,
        message: &str,
        operation: impl FnOnce() -> PmResult<T>,
    ) -> PmResult<T> {
        self.emit_progress(ProgressEvent {
            event_type: "start",
            action,
            source: source.to_string(),
            message: Some(message.to_string()),
        });
        match operation() {
            Ok(value) => {
                self.emit_progress(ProgressEvent {
                    event_type: "complete",
                    action,
                    source: source.to_string(),
                    message: None,
                });
                Ok(value)
            }
            Err(error) => {
                self.emit_progress(ProgressEvent {
                    event_type: "error",
                    action,
                    source: source.to_string(),
                    message: Some(error.message.clone()),
                });
                Err(error)
            }
        }
    }

    pub fn parse_source(&self, source: &str) -> ParsedSource {
        if let Some(spec) = source.strip_prefix("npm:") {
            let spec = spec.trim().to_string();
            let (name, version) = parse_npm_spec(&spec);
            let range = get_npm_version_range(version.as_deref());
            let pinned = is_exact_npm_version(version.as_deref());
            return ParsedSource::Npm(NpmSource {
                spec,
                name,
                version,
                range,
                pinned,
            });
        }

        if is_local_path(source) {
            return ParsedSource::Local(LocalSource {
                path: source.to_string(),
            });
        }

        // Try parsing as git URL.
        if let Some(git_parsed) = parse_git_url(source) {
            return ParsedSource::Git(git_parsed);
        }

        ParsedSource::Local(LocalSource {
            path: source.to_string(),
        })
    }

    // -----------------------------------------------------------------
    // Settings helpers
    // -----------------------------------------------------------------

    fn get_package_source_string(pkg: &PackageSourceEntry) -> String {
        pkg.source().to_string()
    }

    fn get_source_match_key_for_input(&self, source: &str) -> String {
        match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(local) => format!("local:{}", self.resolve_path(&local.path)),
        }
    }

    fn get_source_match_key_for_settings(&self, source: &str, scope: SourceScope) -> String {
        match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(local) => {
                let base_dir = self.get_base_dir_for_scope(scope);
                format!(
                    "local:{}",
                    self.resolve_path_from_base(&local.path, &base_dir)
                )
            }
        }
    }

    fn build_no_matching_package_message(
        &self,
        source: &str,
        configured_packages: &[PackageSourceEntry],
    ) -> String {
        match self.find_suggested_configured_source(source, configured_packages) {
            Some(suggestion) => {
                format!("No matching package found for {source}. Did you mean {suggestion}?")
            }
            None => format!("No matching package found for {source}"),
        }
    }

    fn find_suggested_configured_source(
        &self,
        source: &str,
        configured_packages: &[PackageSourceEntry],
    ) -> Option<String> {
        let trimmed_source = source.trim();
        let mut suggestions: Vec<String> = Vec::new();

        for pkg in configured_packages {
            let source_str = Self::get_package_source_string(pkg);
            match self.parse_source(&source_str) {
                ParsedSource::Npm(npm) => {
                    if (trimmed_source == npm.name || trimmed_source == npm.spec)
                        && !suggestions.contains(&source_str)
                    {
                        suggestions.push(source_str);
                    }
                }
                ParsedSource::Git(git) => {
                    let shorthand = format!("{}/{}", git.host, git.path);
                    let shorthand_with_ref =
                        git.ref_.as_ref().map(|ref_| format!("{shorthand}@{ref_}"));
                    if (trimmed_source == shorthand
                        || shorthand_with_ref.as_deref() == Some(trimmed_source))
                        && !suggestions.contains(&source_str)
                    {
                        suggestions.push(source_str);
                    }
                }
                ParsedSource::Local(_) => {}
            }
        }

        suggestions.into_iter().next()
    }

    fn package_sources_match(
        &self,
        existing: &PackageSourceEntry,
        input_source: &str,
        scope: SourceScope,
    ) -> bool {
        let left = self
            .get_source_match_key_for_settings(&Self::get_package_source_string(existing), scope);
        let right = self.get_source_match_key_for_input(input_source);
        left == right
    }

    fn normalize_package_source_for_settings(&self, source: &str, scope: SourceScope) -> String {
        let parsed = self.parse_source(source);
        let ParsedSource::Local(local) = parsed else {
            return source.to_string();
        };
        let base_dir = self.get_base_dir_for_scope(scope);
        let resolved = self.resolve_path(&local.path);
        let rel = node_relative(&base_dir, &resolved);
        if rel.is_empty() {
            ".".to_string()
        } else {
            rel
        }
    }

    /// Upstream `addSourceToSettings`.
    pub fn add_source_to_settings(&self, source: &str, local: bool) -> bool {
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        let current_settings = if scope == SourceScope::Project {
            self.settings_manager.project_settings()
        } else {
            self.settings_manager.global_settings()
        };
        let current_packages = current_settings.packages;
        let normalized_source = self.normalize_package_source_for_settings(source, scope);
        let match_index = current_packages
            .iter()
            .position(|existing| self.package_sources_match(existing, source, scope));
        let next_packages = match match_index {
            Some(index) => {
                let existing = &current_packages[index];
                if Self::get_package_source_string(existing) == normalized_source {
                    return false;
                }
                let mut next = current_packages.clone();
                next[index] = match existing {
                    PackageSourceEntry::Plain(_) => PackageSourceEntry::Plain(normalized_source),
                    PackageSourceEntry::Object(spec) => {
                        let mut updated = spec.clone();
                        updated.source = normalized_source;
                        PackageSourceEntry::Object(updated)
                    }
                };
                next
            }
            None => {
                let mut next = current_packages.clone();
                next.push(PackageSourceEntry::Plain(normalized_source));
                next
            }
        };
        if scope == SourceScope::Project {
            self.settings_manager.set_project_packages(next_packages);
        } else {
            self.settings_manager.set_packages(next_packages);
        }
        true
    }

    /// Upstream `removeSourceFromSettings`.
    pub fn remove_source_from_settings(&self, source: &str, local: bool) -> bool {
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        let current_settings = if scope == SourceScope::Project {
            self.settings_manager.project_settings()
        } else {
            self.settings_manager.global_settings()
        };
        let current_packages = current_settings.packages;
        let next_packages: Vec<PackageSourceEntry> = current_packages
            .iter()
            .filter(|existing| !self.package_sources_match(existing, source, scope))
            .cloned()
            .collect();
        let changed = next_packages.len() != current_packages.len();
        if !changed {
            return false;
        }
        if scope == SourceScope::Project {
            self.settings_manager.set_project_packages(next_packages);
        } else {
            self.settings_manager.set_packages(next_packages);
        }
        true
    }

    /// Upstream `getInstalledPath`.
    pub fn get_installed_path(&self, source: &str, scope: SourceScope) -> Option<String> {
        match self.parse_source(source) {
            ParsedSource::Npm(npm) => {
                let path = self.get_npm_install_path(&npm, scope).ok()?;
                std::path::Path::new(&path).exists().then_some(path)
            }
            ParsedSource::Git(git) => {
                let path = self.get_git_install_path(&git, scope).ok()?;
                std::path::Path::new(&path).exists().then_some(path)
            }
            ParsedSource::Local(local) => {
                let base_dir = self.get_base_dir_for_scope(scope);
                let path = self.resolve_path_from_base(&local.path, &base_dir);
                std::path::Path::new(&path).exists().then_some(path)
            }
        }
    }

    // -----------------------------------------------------------------
    // Resolve
    // -----------------------------------------------------------------

    /// Upstream `resolve`.
    pub fn resolve(
        &self,
        on_missing: Option<&dyn Fn(&str) -> MissingSourceAction>,
    ) -> PmResult<ResolvedPaths> {
        let mut accumulator = ResourceAccumulator::default();
        let global_settings = self.settings_manager.global_settings();
        let project_settings = self.settings_manager.project_settings();

        // Project first so cwd resources win collisions.
        let mut all_packages: Vec<(&PackageSourceEntry, SourceScope)> = Vec::new();
        for pkg in &project_settings.packages {
            all_packages.push((pkg, SourceScope::Project));
        }
        for pkg in &global_settings.packages {
            all_packages.push((pkg, SourceScope::User));
        }

        let package_sources = self.dedupe_packages(all_packages);
        self.resolve_package_sources(&package_sources, &mut accumulator, on_missing)?;

        let global_base_dir = self.agent_dir.clone();
        let project_base_dir = node_join(&self.cwd, &[CONFIG_DIR_NAME]);

        for resource_type in RESOURCE_TYPES {
            let project_entries = project_resource_entries(&project_settings, resource_type);
            self.resolve_local_entries(
                &project_entries,
                resource_type,
                &mut accumulator,
                &PathMetadata {
                    source: "local".to_string(),
                    scope: SourceScope::Project,
                    origin: PathMetadataOrigin::TopLevel,
                    base_dir: None,
                },
                &project_base_dir,
            );
            let global_entries = project_resource_entries(&global_settings, resource_type);
            self.resolve_local_entries(
                &global_entries,
                resource_type,
                &mut accumulator,
                &PathMetadata {
                    source: "local".to_string(),
                    scope: SourceScope::User,
                    origin: PathMetadataOrigin::TopLevel,
                    base_dir: None,
                },
                &global_base_dir,
            );
        }

        self.add_auto_discovered_resources(
            &mut accumulator,
            &global_settings,
            &project_settings,
            &global_base_dir,
            &project_base_dir,
        );

        Ok(self.to_resolved_paths(accumulator))
    }

    /// Upstream `resolveExtensionSources`.
    pub fn resolve_extension_sources(
        &self,
        sources: &[String],
        local: bool,
        temporary: bool,
    ) -> PmResult<ResolvedPaths> {
        let mut accumulator = ResourceAccumulator::default();
        let scope = if temporary {
            SourceScope::Temporary
        } else if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        let package_sources: Vec<(PackageSourceEntry, SourceScope)> = sources
            .iter()
            .map(|source| (PackageSourceEntry::Plain(source.clone()), scope))
            .collect();
        self.resolve_package_sources(&package_sources, &mut accumulator, None)?;
        Ok(self.to_resolved_paths(accumulator))
    }

    /// Upstream `listConfiguredPackages`.
    pub fn list_configured_packages(&self) -> Vec<ConfiguredPackage> {
        let global_settings = self.settings_manager.global_settings();
        let project_settings = self.settings_manager.project_settings();
        let mut configured_packages = Vec::new();

        for pkg in &global_settings.packages {
            let source = Self::get_package_source_string(pkg);
            let installed_path = self.get_installed_path(&source, SourceScope::User);
            configured_packages.push(ConfiguredPackage {
                source,
                scope: SourceScope::User,
                filtered: matches!(pkg, PackageSourceEntry::Object(_)),
                installed_path,
            });
        }

        for pkg in &project_settings.packages {
            let source = Self::get_package_source_string(pkg);
            let installed_path = self.get_installed_path(&source, SourceScope::Project);
            configured_packages.push(ConfiguredPackage {
                source,
                scope: SourceScope::Project,
                filtered: matches!(pkg, PackageSourceEntry::Object(_)),
                installed_path,
            });
        }

        configured_packages
    }

    // -----------------------------------------------------------------
    // Install / remove / update
    // -----------------------------------------------------------------

    /// Upstream `install`.
    pub fn install(&self, source: &str, local: bool) -> PmResult<()> {
        let parsed = self.parse_source(source);
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        self.assert_project_trusted_for_scope(scope)?;
        self.with_progress(
            "install",
            source,
            &format!("Installing {source}..."),
            || match parsed {
                ParsedSource::Npm(npm) => self.install_npm(&npm, scope, false),
                ParsedSource::Git(git) => self.install_git(&git, scope),
                ParsedSource::Local(local_source) => {
                    let resolved = self.resolve_path(&local_source.path);
                    if !std::path::Path::new(&resolved).exists() {
                        return Err(PmError::new(format!("Path does not exist: {resolved}")));
                    }
                    Ok(())
                }
            },
        )
    }

    /// Upstream `installAndPersist`.
    pub fn install_and_persist(&self, source: &str, local: bool) -> PmResult<()> {
        self.install(source, local)?;
        self.add_source_to_settings(source, local);
        Ok(())
    }

    /// Upstream `remove`.
    pub fn remove(&self, source: &str, local: bool) -> PmResult<()> {
        let parsed = self.parse_source(source);
        let scope = if local {
            SourceScope::Project
        } else {
            SourceScope::User
        };
        self.assert_project_trusted_for_scope(scope)?;
        self.with_progress(
            "remove",
            source,
            &format!("Removing {source}..."),
            || match parsed {
                ParsedSource::Npm(npm) => self.uninstall_npm(&npm, scope),
                ParsedSource::Git(git) => self.remove_git(&git, scope),
                ParsedSource::Local(_) => Ok(()),
            },
        )
    }

    /// Upstream `removeAndPersist`.
    pub fn remove_and_persist(&self, source: &str, local: bool) -> PmResult<bool> {
        self.remove(source, local)?;
        Ok(self.remove_source_from_settings(source, local))
    }

    /// Upstream `update`.
    pub fn update(&self, source: Option<&str>) -> PmResult<()> {
        let global_settings = self.settings_manager.global_settings();
        let project_settings = self.settings_manager.project_settings();
        let identity = source.map(|source| self.get_package_identity(source, None));
        let mut matched = false;
        let mut update_sources: Vec<(PackageSourceEntry, SourceScope)> = Vec::new();

        for pkg in &global_settings.packages {
            let source_str = Self::get_package_source_string(pkg);
            if let Some(identity) = &identity {
                if self.get_package_identity(&source_str, Some(SourceScope::User)) != *identity {
                    continue;
                }
            }
            matched = true;
            update_sources.push((pkg.clone(), SourceScope::User));
        }
        for pkg in &project_settings.packages {
            let source_str = Self::get_package_source_string(pkg);
            if let Some(identity) = &identity {
                if self.get_package_identity(&source_str, Some(SourceScope::Project)) != *identity {
                    continue;
                }
            }
            matched = true;
            update_sources.push((pkg.clone(), SourceScope::Project));
        }

        if source.is_some() && !matched {
            let mut configured = global_settings.packages.clone();
            configured.extend(project_settings.packages.clone());
            return Err(PmError::new(self.build_no_matching_package_message(
                source.unwrap_or_default(),
                &configured,
            )));
        }

        self.update_configured_sources(update_sources)
    }

    fn update_configured_sources(
        &self,
        sources: Vec<(PackageSourceEntry, SourceScope)>,
    ) -> PmResult<()> {
        if is_offline_mode_enabled() || sources.is_empty() {
            return Ok(());
        }

        let mut npm_candidates: Vec<(NpmSource, PackageSourceEntry, SourceScope)> = Vec::new();
        let mut git_candidates: Vec<(GitSource, PackageSourceEntry, SourceScope)> = Vec::new();

        for (entry, scope) in &sources {
            match self.parse_source(&Self::get_package_source_string(entry)) {
                // Pinned npm versions are fixed. Pinned git refs are configured
                // checkout targets, so include them to reconcile an existing
                // clone when the configured ref changes.
                ParsedSource::Npm(parsed) => {
                    if !parsed.pinned {
                        npm_candidates.push((parsed, entry.clone(), *scope));
                    }
                }
                ParsedSource::Git(parsed) => {
                    git_candidates.push((parsed, entry.clone(), *scope));
                }
                ParsedSource::Local(_) => {}
            }
        }

        // npm update checks run with bounded concurrency.
        let npm_check_results: Vec<bool> = {
            let tasks: Vec<Box<dyn FnOnce() -> PmResult<bool> + Send + '_>> = npm_candidates
                .iter()
                .map(|(parsed, _entry, scope)| {
                    let parsed = parsed.clone();
                    let scope = *scope;
                    Box::new(move || self.should_update_npm_source(&parsed, scope))
                        as Box<dyn FnOnce() -> PmResult<bool> + Send + '_>
                })
                .collect();
            let results = run_with_concurrency(tasks, UPDATE_CHECK_CONCURRENCY);
            let mut checked = Vec::with_capacity(results.len());
            for result in results {
                checked.push(result?);
            }
            checked
        };

        let user_npm_updates: Vec<&(NpmSource, PackageSourceEntry, SourceScope)> = npm_candidates
            .iter()
            .zip(npm_check_results.iter())
            .filter(|(_, should_update)| **should_update)
            .map(|(candidate, _)| candidate)
            .filter(|(_, _, scope)| *scope == SourceScope::User)
            .collect();
        let project_npm_updates: Vec<&(NpmSource, PackageSourceEntry, SourceScope)> =
            npm_candidates
                .iter()
                .zip(npm_check_results.iter())
                .filter(|(_, should_update)| **should_update)
                .map(|(candidate, _)| candidate)
                .filter(|(_, _, scope)| *scope == SourceScope::Project)
                .collect();

        // Batches and git updates run concurrently (upstream `Promise.all`).
        let user_error: Mutex<Option<PmError>> = Mutex::new(None);
        let project_error: Mutex<Option<PmError>> = Mutex::new(None);
        let git_error: Mutex<Option<PmError>> = Mutex::new(None);
        std::thread::scope(|scope| {
            if !user_npm_updates.is_empty() {
                let user_error_ref = &user_error;
                scope.spawn(move || {
                    let updates: Vec<(NpmSource, PackageSourceEntry, SourceScope)> =
                        user_npm_updates
                            .iter()
                            .map(|(parsed, entry, scope)| (parsed.clone(), entry.clone(), *scope))
                            .collect();
                    if let Err(error) = self.update_npm_batch(&updates, SourceScope::User) {
                        *user_error_ref.lock().unwrap() = Some(error);
                    }
                });
            }
            if !project_npm_updates.is_empty() {
                let project_error_ref = &project_error;
                scope.spawn(move || {
                    let updates: Vec<(NpmSource, PackageSourceEntry, SourceScope)> =
                        project_npm_updates
                            .iter()
                            .map(|(parsed, entry, scope)| (parsed.clone(), entry.clone(), *scope))
                            .collect();
                    if let Err(error) = self.update_npm_batch(&updates, SourceScope::Project) {
                        *project_error_ref.lock().unwrap() = Some(error);
                    }
                });
            }
            if !git_candidates.is_empty() {
                let git_error_ref = &git_error;
                let git_tasks: Vec<Box<dyn FnOnce() + Send + '_>> = git_candidates
                    .iter()
                    .map(|(parsed, entry, scope)| {
                        let parsed = parsed.clone();
                        let entry_source = Self::get_package_source_string(entry);
                        let scope = *scope;
                        let error_ref = &git_error;
                        Box::new(move || {
                            let result = self.with_progress(
                                "update",
                                &entry_source,
                                &format!("Updating {entry_source}..."),
                                || self.update_git(&parsed, scope),
                            );
                            if let Err(error) = result {
                                *error_ref.lock().unwrap() = Some(error);
                            }
                        }) as Box<dyn FnOnce() + Send + '_>
                    })
                    .collect();
                let _unused = run_with_concurrency(git_tasks, GIT_UPDATE_CONCURRENCY);
                let _ = git_error_ref;
            }
        });

        if let Some(error) = user_error.into_inner().unwrap_or(None) {
            return Err(error);
        }
        if let Some(error) = project_error.into_inner().unwrap_or(None) {
            return Err(error);
        }
        if let Some(error) = git_error.into_inner().unwrap_or(None) {
            return Err(error);
        }
        Ok(())
    }

    fn should_update_npm_source(&self, source: &NpmSource, scope: SourceScope) -> PmResult<bool> {
        let installed_path = self.get_managed_npm_install_path(source, scope)?;
        let installed_version = if std::path::Path::new(&installed_path).exists() {
            self.get_installed_npm_version(&installed_path)
        } else {
            None
        };
        let Some(installed_version) = installed_version else {
            return Ok(true);
        };

        let spec = source
            .version
            .as_deref()
            .map_or(source.name.as_str(), |_| source.spec.as_str());
        match self.get_latest_npm_version(spec, source.range.as_ref()) {
            Ok(target_version) => Ok(vendor::semver_gt(&target_version, &installed_version)),
            // Preserve existing update behavior when version lookup fails.
            Err(_) => Ok(true),
        }
    }

    fn update_npm_batch(
        &self,
        sources: &[(NpmSource, PackageSourceEntry, SourceScope)],
        scope: SourceScope,
    ) -> PmResult<()> {
        if sources.is_empty() {
            return Ok(());
        }

        let source_label = if sources.len() == 1 {
            Self::get_package_source_string(&sources[0].1)
        } else {
            match scope {
                SourceScope::User => "user npm packages".to_string(),
                _ => "project npm packages".to_string(),
            }
        };
        let message = if sources.len() == 1 {
            format!(
                "Updating {}...",
                Self::get_package_source_string(&sources[0].1)
            )
        } else {
            format!(
                "Updating {} npm packages...",
                if scope == SourceScope::User {
                    "user"
                } else {
                    "project"
                }
            )
        };
        let specs: Vec<String> = sources
            .iter()
            .map(|(parsed, _, _)| {
                parsed.version.as_ref().map_or_else(
                    || format!("{}@latest", parsed.name),
                    |_| parsed.spec.clone(),
                )
            })
            .collect();

        self.with_progress("update", &source_label, &message, || {
            self.install_npm_batch(&specs, scope)
        })
    }

    fn install_npm_batch(&self, specs: &[String], scope: SourceScope) -> PmResult<()> {
        let install_root = self.get_npm_install_root(scope, false)?;
        self.ensure_npm_project(&install_root);
        let args = self.get_npm_install_args(specs, &install_root);
        self.run_npm_command(&args, None)
    }

    /// Upstream `checkForAvailableUpdates`.
    pub fn check_for_available_updates(&self) -> PmResult<Vec<PackageUpdate>> {
        if is_offline_mode_enabled() {
            return Ok(Vec::new());
        }

        let global_settings = self.settings_manager.global_settings();
        let project_settings = self.settings_manager.project_settings();
        let mut all_packages: Vec<(PackageSourceEntry, SourceScope)> = Vec::new();
        for pkg in &project_settings.packages {
            all_packages.push((pkg.clone(), SourceScope::Project));
        }
        for pkg in &global_settings.packages {
            all_packages.push((pkg.clone(), SourceScope::User));
        }

        let package_sources = self.dedupe_packages_owned(all_packages);
        let checks: Vec<Box<dyn FnOnce() -> PmResult<Option<PackageUpdate>> + Send + '_>> =
            package_sources
                .iter()
                .map(|(pkg, scope)| {
                    let source = Self::get_package_source_string(pkg);
                    let scope = *scope;
                    Box::new(move || {
                        let parsed = self.parse_source(&source);
                        match parsed {
                            ParsedSource::Npm(npm) if !npm.pinned => {
                                let installed_path = self.get_npm_install_path(&npm, scope)?;
                                if !std::path::Path::new(&installed_path).exists() {
                                    return Ok(None);
                                }
                                if !self.npm_has_available_update(&npm, &installed_path)? {
                                    return Ok(None);
                                }
                                Ok(Some(PackageUpdate {
                                    source,
                                    display_name: npm.name,
                                    update_type: "npm",
                                    scope,
                                }))
                            }
                            ParsedSource::Npm(_) => Ok(None),
                            ParsedSource::Git(git) if !git.pinned => {
                                let installed_path = self.get_git_install_path(&git, scope)?;
                                if !std::path::Path::new(&installed_path).exists() {
                                    return Ok(None);
                                }
                                if !self.git_has_available_update(&installed_path)? {
                                    return Ok(None);
                                }
                                Ok(Some(PackageUpdate {
                                    source,
                                    display_name: format!("{}/{}", git.host, git.path),
                                    update_type: "git",
                                    scope,
                                }))
                            }
                            ParsedSource::Git(_) => Ok(None),
                            ParsedSource::Local(_) => Ok(None),
                        }
                    })
                        as Box<dyn FnOnce() -> PmResult<Option<PackageUpdate>> + Send + '_>
                })
                .collect();

        let results = run_with_concurrency(checks, UPDATE_CHECK_CONCURRENCY);
        let mut updates = Vec::new();
        for result in results {
            if let Some(update) = result? {
                updates.push(update);
            }
        }
        Ok(updates)
    }

    // -----------------------------------------------------------------
    // Package source resolution internals
    // -----------------------------------------------------------------

    fn resolve_package_sources(
        &self,
        sources: &[(PackageSourceEntry, SourceScope)],
        accumulator: &mut ResourceAccumulator,
        on_missing: Option<&dyn Fn(&str) -> MissingSourceAction>,
    ) -> PmResult<()> {
        for (pkg, scope) in sources {
            let source_str = Self::get_package_source_string(pkg);
            let filter = match pkg {
                PackageSourceEntry::Object(spec) => Some(spec.as_filter()),
                PackageSourceEntry::Plain(_) => None,
            };
            let delta_base = self.find_autoload_delta_base(pkg, *scope, sources);
            let resolved_source = delta_base
                .as_ref()
                .map_or(source_str.as_str(), |(source, _)| source);
            let resolved_scope = delta_base.as_ref().map_or(*scope, |(_, scope)| *scope);
            let parsed = self.parse_source(resolved_source);
            let mut metadata = PathMetadata {
                source: source_str.clone(),
                scope: *scope,
                origin: PathMetadataOrigin::Package,
                base_dir: None,
            };

            if let ParsedSource::Local(local) = &parsed {
                let base_dir = self.get_base_dir_for_scope(resolved_scope);
                self.resolve_local_extension_source(
                    local,
                    accumulator,
                    filter.as_ref(),
                    metadata,
                    &base_dir,
                );
                continue;
            }

            macro_rules! install_missing {
                ($parsed:expr) => {{
                    if is_offline_mode_enabled() {
                        false
                    } else {
                        match on_missing {
                            None => {
                                self.install_parsed_source(&$parsed, resolved_scope)?;
                                true
                            }
                            Some(on_missing) => match on_missing(resolved_source) {
                                MissingSourceAction::Skip => false,
                                MissingSourceAction::Error => {
                                    return Err(PmError::new(format!(
                                        "Missing source: {resolved_source}"
                                    )));
                                }
                                MissingSourceAction::Install => {
                                    self.install_parsed_source(&$parsed, resolved_scope)?;
                                    true
                                }
                            },
                        }
                    }
                }};
            }

            match &parsed {
                ParsedSource::Npm(npm) => {
                    let mut installed_path = self.get_npm_install_path(npm, resolved_scope)?;
                    let needs_install = !std::path::Path::new(&installed_path).exists()
                        || !self.installed_npm_matches_configured_version(npm, &installed_path);
                    if needs_install {
                        let installed = install_missing!(parsed.clone());
                        if !installed {
                            continue;
                        }
                        installed_path = self.get_npm_install_path(npm, resolved_scope)?;
                    }
                    metadata.base_dir = Some(installed_path.clone());
                    self.collect_package_resources(
                        &installed_path,
                        accumulator,
                        filter.as_ref(),
                        &metadata,
                    );
                }
                ParsedSource::Git(git) => {
                    let installed_path = self.get_git_install_path(git, resolved_scope)?;
                    if !std::path::Path::new(&installed_path).exists() {
                        let installed = install_missing!(parsed.clone());
                        if !installed {
                            continue;
                        }
                    } else if resolved_scope == SourceScope::Temporary
                        && !git.pinned
                        && !is_offline_mode_enabled()
                    {
                        self.refresh_temporary_git_source(git, resolved_source)?;
                    }
                    metadata.base_dir = Some(installed_path.clone());
                    self.collect_package_resources(
                        &installed_path,
                        accumulator,
                        filter.as_ref(),
                        &metadata,
                    );
                }
                ParsedSource::Local(_) => unreachable!("handled above"),
            }
        }
        Ok(())
    }

    fn find_autoload_delta_base(
        &self,
        pkg: &PackageSourceEntry,
        scope: SourceScope,
        sources: &[(PackageSourceEntry, SourceScope)],
    ) -> Option<(String, SourceScope)> {
        let spec = match (pkg, scope) {
            (PackageSourceEntry::Object(spec), SourceScope::Project)
                if spec.autoload == Some(false) =>
            {
                spec
            }
            _ => return None,
        };
        let identity = self.get_package_identity(&spec.source, Some(scope));
        sources.iter().find_map(|(entry, entry_scope)| {
            if *entry_scope != SourceScope::User {
                return None;
            }
            let entry_source = Self::get_package_source_string(entry);
            if self.get_package_identity(&entry_source, Some(SourceScope::User)) == identity {
                Some((entry_source, SourceScope::User))
            } else {
                None
            }
        })
    }

    fn resolve_local_extension_source(
        &self,
        source: &LocalSource,
        accumulator: &mut ResourceAccumulator,
        filter: Option<&PackageFilter>,
        mut metadata: PathMetadata,
        base_dir: &str,
    ) {
        let resolved = self.resolve_path_from_base(&source.path, base_dir);
        if !std::path::Path::new(&resolved).exists() {
            return;
        }

        match entry_kind_of(std::path::Path::new(&resolved)) {
            Some(EntryKind::File) => {
                metadata.base_dir = Some(node_dirname(&resolved));
                accumulator.extensions.add(&resolved, metadata, true);
            }
            Some(EntryKind::Dir) => {
                metadata.base_dir = Some(resolved.clone());
                let resources =
                    self.collect_package_resources(&resolved, accumulator, filter, &metadata);
                if !resources {
                    accumulator.extensions.add(&resolved, metadata, true);
                }
            }
            None => {}
        }
    }

    fn install_parsed_source(&self, parsed: &ParsedSource, scope: SourceScope) -> PmResult<()> {
        match parsed {
            ParsedSource::Npm(npm) => self.install_npm(npm, scope, scope == SourceScope::Temporary),
            ParsedSource::Git(git) => self.install_git(git, scope),
            ParsedSource::Local(_) => Ok(()),
        }
    }

    /// Upstream `getPackageIdentity` (version/ref ignored).
    pub fn get_package_identity(&self, source: &str, scope: Option<SourceScope>) -> String {
        match self.parse_source(source) {
            ParsedSource::Npm(npm) => format!("npm:{}", npm.name),
            ParsedSource::Git(git) => format!("git:{}/{}", git.host, git.path),
            ParsedSource::Local(local) => match scope {
                Some(scope) => {
                    let base_dir = self.get_base_dir_for_scope(scope);
                    format!(
                        "local:{}",
                        self.resolve_path_from_base(&local.path, &base_dir)
                    )
                }
                None => format!("local:{}", self.resolve_path(&local.path)),
            },
        }
    }

    /// Upstream `dedupePackages`: project wins; autoload=false project
    /// entries are deltas over the global entry (both kept, delta first).
    fn dedupe_packages(
        &self,
        packages: Vec<(&PackageSourceEntry, SourceScope)>,
    ) -> Vec<(PackageSourceEntry, SourceScope)> {
        let owned: Vec<(PackageSourceEntry, SourceScope)> = packages
            .into_iter()
            .map(|(pkg, scope)| (pkg.clone(), scope))
            .collect();
        self.dedupe_packages_owned(owned)
    }

    fn dedupe_packages_owned(
        &self,
        packages: Vec<(PackageSourceEntry, SourceScope)>,
    ) -> Vec<(PackageSourceEntry, SourceScope)> {
        let mut result: Vec<(PackageSourceEntry, SourceScope)> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for entry in packages {
            let identity = self
                .get_package_identity(&Self::get_package_source_string(&entry.0), Some(entry.1));
            match seen.get(&identity) {
                None => {
                    seen.insert(identity, result.len());
                    result.push(entry);
                }
                Some(&index) => {
                    let existing = &result[index];
                    if existing.1 == SourceScope::Project && entry.1 == SourceScope::User {
                        if matches!(&existing.0, PackageSourceEntry::Object(spec) if spec.autoload == Some(false))
                        {
                            result.push(entry);
                        }
                    } else if entry.1 == SourceScope::Project {
                        result[index] = entry;
                    }
                }
            }
        }
        result
    }

    // -----------------------------------------------------------------
    // npm plumbing
    // -----------------------------------------------------------------

    fn assert_project_trusted_for_scope(&self, scope: SourceScope) -> PmResult<()> {
        if scope == SourceScope::Project && !self.settings_manager.is_project_trusted() {
            return Err(PmError::new(
                "Project is not trusted; refusing to access project package storage",
            ));
        }
        Ok(())
    }

    fn get_npm_command(&self) -> PmResult<(String, Vec<String>)> {
        match self.settings_manager.npm_command() {
            None => Ok(("npm".to_string(), Vec::new())),
            Some(command) if command.is_empty() => Ok(("npm".to_string(), Vec::new())),
            Some(command) => {
                let (first, rest) = command.split_first().unwrap();
                if first.is_empty() {
                    return Err(PmError::new(
                        "Invalid npmCommand: first array entry must be a non-empty command",
                    ));
                }
                Ok((first.clone(), rest.to_vec()))
            }
        }
    }

    fn get_package_manager_name(&self) -> String {
        let Ok((command, args)) = self.get_npm_command() else {
            return String::new();
        };
        let mut command_parts = vec![command];
        command_parts.extend(args);
        let separator_index = command_parts.iter().rposition(|part| part == "--");
        let package_manager_command = match separator_index {
            Some(index) => command_parts.get(index + 1).cloned().unwrap_or_default(),
            None => command_parts[0].clone(),
        };
        let base = node_basename(&package_manager_command);
        // `basename(command).replace(/\.(cmd|exe)$/i, "")`.
        let lowered = base.to_lowercase();
        if lowered.ends_with(".cmd") || lowered.ends_with(".exe") {
            base[..base.len() - 4].to_string()
        } else {
            base
        }
    }

    fn run_npm_command(&self, args: &[String], cwd: Option<&str>) -> PmResult<()> {
        let (command, mut npm_args) = self.get_npm_command()?;
        npm_args.extend(args.iter().cloned());
        self.runner
            .run(&command, &npm_args, cwd)
            .map_err(|error| PmError::new(error.message))
    }

    fn get_git_dependency_install_args(&self) -> Vec<String> {
        let configured_command = self.settings_manager.npm_command();
        if configured_command
            .as_ref()
            .is_some_and(|command| !command.is_empty())
        {
            return vec!["install".to_string()];
        }
        vec!["install".to_string(), "--omit=dev".to_string()]
    }

    /// Upstream `runCommandSync` surface (public for the test suite).
    pub fn run_command_sync(&self, command: &str, args: &[String]) -> PmResult<String> {
        self.runner
            .run_sync(command, args)
            .map_err(|error| PmError::new(error.message))
    }

    fn run_npm_command_sync(&self, args: &[String]) -> PmResult<String> {
        let (command, mut npm_args) = self.get_npm_command()?;
        npm_args.extend(args.iter().cloned());
        self.runner
            .run_sync(&command, &npm_args)
            .map_err(|error| PmError::new(error.message))
    }

    fn get_npm_install_args(&self, specs: &[String], install_root: &str) -> Vec<String> {
        let package_manager_name = self.get_package_manager_name();
        // Extension packages run inside pi and resolve pi APIs through loader
        // aliases/virtual modules. Disable peer dependency resolution for
        // managed installs so package managers do not install or solve
        // host-provided @earendil-works/pi-* peers.
        if package_manager_name == "bun" {
            let mut args = vec!["install".to_string()];
            args.extend(specs.iter().cloned());
            args.extend([
                "--cwd".to_string(),
                install_root.to_string(),
                "--omit=peer".to_string(),
            ]);
            return args;
        }
        if package_manager_name == "pnpm" {
            let mut args = vec!["install".to_string()];
            args.extend(specs.iter().cloned());
            args.extend([
                "--prefix".to_string(),
                install_root.to_string(),
                "--config.auto-install-peers=false".to_string(),
                "--config.strict-peer-dependencies=false".to_string(),
                "--config.strict-dep-builds=false".to_string(),
            ]);
            return args;
        }
        let mut args = vec!["install".to_string()];
        args.extend(specs.iter().cloned());
        args.extend([
            "--prefix".to_string(),
            install_root.to_string(),
            "--legacy-peer-deps".to_string(),
        ]);
        args
    }

    fn install_npm(&self, source: &NpmSource, scope: SourceScope, temporary: bool) -> PmResult<()> {
        let install_root = self.get_npm_install_root(scope, temporary)?;
        self.ensure_npm_project(&install_root);
        let args = self.get_npm_install_args(std::slice::from_ref(&source.spec), &install_root);
        self.run_npm_command(&args, None)
    }

    fn uninstall_npm(&self, source: &NpmSource, scope: SourceScope) -> PmResult<()> {
        let install_root = self.get_npm_install_root(scope, false)?;
        if !std::path::Path::new(&install_root).exists() {
            return Ok(());
        }
        let package_manager_name = self.get_package_manager_name();
        if package_manager_name == "bun" {
            return self.run_npm_command(
                &[
                    "uninstall".to_string(),
                    source.name.clone(),
                    "--cwd".to_string(),
                    install_root.clone(),
                ],
                None,
            );
        }
        let mut args = vec![
            "uninstall".to_string(),
            source.name.clone(),
            "--prefix".to_string(),
            install_root.clone(),
        ];
        if package_manager_name != "pnpm" {
            args.push("--legacy-peer-deps".to_string());
        }
        self.run_npm_command(&args, None)
    }

    // -----------------------------------------------------------------
    // git plumbing
    // -----------------------------------------------------------------

    fn install_git(&self, source: &GitSource, scope: SourceScope) -> PmResult<()> {
        let target_dir = self.get_git_install_path(source, scope)?;
        if std::path::Path::new(&target_dir).exists() {
            if let Some(ref_) = &source.ref_ {
                return self.ensure_git_ref(
                    &target_dir,
                    &["fetch".to_string(), "origin".to_string(), ref_.clone()],
                    "FETCH_HEAD",
                );
            }
            let target = self.get_local_git_update_target(&target_dir)?;
            let fetch_args = target.fetch_args;
            return self.ensure_git_ref(&target_dir, &fetch_args, &target.ref_);
        }
        let git_root = self.get_git_install_root(scope);
        if let Some(git_root) = &git_root {
            self.ensure_git_ignore(git_root);
        }
        let _ = std::fs::create_dir_all(node_dirname(&target_dir));
        let _ = std::fs::remove_file(self.get_git_update_marker_path(&target_dir));

        let result = (|| -> PmResult<()> {
            self.runner
                .run(
                    "git",
                    &["clone".to_string(), source.repo.clone(), target_dir.clone()],
                    None,
                )
                .map_err(|error| PmError::new(error.message))?;
            if let Some(ref_) = &source.ref_ {
                self.runner
                    .run(
                        "git",
                        &["checkout".to_string(), ref_.clone()],
                        Some(&target_dir),
                    )
                    .map_err(|error| PmError::new(error.message))?;
            }
            let package_json_path = node_join(&target_dir, &["package.json"]);
            if std::path::Path::new(&package_json_path).exists() {
                self.run_npm_command(&self.get_git_dependency_install_args(), Some(&target_dir))?;
            }
            Ok(())
        })();

        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&target_dir);
            self.prune_empty_git_parents(&target_dir, git_root.as_deref());
            return Err(error);
        }
        Ok(())
    }

    fn update_git(&self, source: &GitSource, scope: SourceScope) -> PmResult<()> {
        let target_dir = self.get_git_install_path(source, scope)?;
        if !std::path::Path::new(&target_dir).exists() {
            return self.install_git(source, scope);
        }

        if let Some(ref_) = &source.ref_ {
            return self.ensure_git_ref(
                &target_dir,
                &["fetch".to_string(), "origin".to_string(), ref_.clone()],
                "FETCH_HEAD",
            );
        }

        let target = self.get_local_git_update_target(&target_dir)?;
        let fetch_args = target.fetch_args;
        self.ensure_git_ref(&target_dir, &fetch_args, &target.ref_)
    }

    fn has_missing_git_dependencies(&self, target_dir: &str) -> bool {
        let package_json_path = node_join(target_dir, &["package.json"]);
        if !std::path::Path::new(&package_json_path).exists() {
            return false;
        }

        let Ok(content) = std::fs::read_to_string(&package_json_path) else {
            return false;
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(strip_bom(&content)) else {
            return false;
        };
        let Some(dependencies) = manifest.get("dependencies") else {
            return false;
        };
        let Some(dependencies) = dependencies.as_object() else {
            return false;
        };

        let node_modules_dir = node_resolve_under(target_dir, &["node_modules"]);
        dependencies.keys().any(|name| {
            let dependency_path = node_resolve_under(&node_modules_dir, &[name]);
            if !dependency_path.starts_with(&format!("{}{}", node_modules_dir, node_sep())) {
                return false;
            }
            !std::path::Path::new(&dependency_path).exists()
        })
    }

    fn repair_missing_git_dependencies(&self, target_dir: &str) -> PmResult<()> {
        if !self.has_missing_git_dependencies(target_dir) {
            return Ok(());
        }
        self.run_npm_command(&self.get_git_dependency_install_args(), Some(target_dir))
    }

    fn get_git_update_marker_path(&self, target_dir: &str) -> String {
        node_join(
            &node_dirname(target_dir),
            &[&format!(
                ".{}.pi-update-incomplete",
                node_basename(target_dir)
            )],
        )
    }

    fn clean_and_install_git_dependencies(
        &self,
        target_dir: &str,
        marker_path: &str,
    ) -> PmResult<()> {
        // Clean untracked files (extensions should be pristine). If this
        // fails after deleting dependencies, repair them so the existing
        // extension still loads.
        if let Err(error) = self
            .runner
            .run(
                "git",
                &["clean".to_string(), "-fdx".to_string()],
                Some(target_dir),
            )
            .map_err(|error| PmError::new(error.message))
        {
            let _ = self.repair_missing_git_dependencies(target_dir);
            return Err(error);
        }

        let package_json_path = node_join(target_dir, &["package.json"]);
        if std::path::Path::new(&package_json_path).exists() {
            self.run_npm_command(&self.get_git_dependency_install_args(), Some(target_dir))?;
        }
        let _ = std::fs::remove_file(marker_path);
        Ok(())
    }

    fn ensure_git_ref(&self, target_dir: &str, fetch_args: &[String], ref_: &str) -> PmResult<()> {
        // Fetch only the ref we will reset to, avoiding unrelated branch/tag
        // noise.
        self.runner
            .run("git", fetch_args, Some(target_dir))
            .map_err(|error| PmError::new(error.message))?;

        let local_head = self
            .runner
            .run_capture(
                "git",
                &["rev-parse".to_string(), "HEAD".to_string()],
                Some(target_dir),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let commit_ref = format!("{ref_}^{{commit}}");
        let target_head = self
            .runner
            .run_capture(
                "git",
                &["rev-parse".to_string(), commit_ref.clone()],
                Some(target_dir),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let marker_path = self.get_git_update_marker_path(target_dir);
        if local_head.trim() == target_head.trim() {
            if std::path::Path::new(&marker_path).exists() {
                self.clean_and_install_git_dependencies(target_dir, &marker_path)?;
            } else {
                self.repair_missing_git_dependencies(target_dir)?;
            }
            return Ok(());
        }

        let _ = std::fs::write(&marker_path, "");
        self.runner
            .run(
                "git",
                &["reset".to_string(), "--hard".to_string(), commit_ref],
                Some(target_dir),
            )
            .map_err(|error| PmError::new(error.message))?;
        self.clean_and_install_git_dependencies(target_dir, &marker_path)
    }

    fn refresh_temporary_git_source(&self, source: &GitSource, source_str: &str) -> PmResult<()> {
        if is_offline_mode_enabled() {
            return Ok(());
        }
        // Keep cached temporary checkout if refresh fails.
        let _ = self.with_progress(
            "pull",
            source_str,
            &format!("Refreshing {source_str}..."),
            || self.update_git(source, SourceScope::Temporary),
        );
        Ok(())
    }

    fn remove_git(&self, source: &GitSource, scope: SourceScope) -> PmResult<()> {
        let target_dir = self.get_git_install_path(source, scope)?;
        let _ = std::fs::remove_dir_all(&target_dir);
        let _ = std::fs::remove_file(self.get_git_update_marker_path(&target_dir));
        let install_root = self.get_git_install_root(scope);
        self.prune_empty_git_parents(&target_dir, install_root.as_deref());
        Ok(())
    }

    fn prune_empty_git_parents(&self, target_dir: &str, install_root: Option<&str>) {
        let Some(install_root) = install_root else {
            return;
        };
        let resolved_root = node_resolve(&[install_root]);
        let mut current = node_dirname(target_dir);
        while current.starts_with(&resolved_root) && current != resolved_root {
            if !std::path::Path::new(&current).exists() {
                current = node_dirname(&current);
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&current) else {
                break;
            };
            if entries.count() > 0 {
                break;
            }
            if std::fs::remove_dir_all(&current).is_err() {
                break;
            }
            current = node_dirname(&current);
        }
    }

    // -----------------------------------------------------------------
    // npm install roots / paths
    // -----------------------------------------------------------------

    fn ensure_npm_project(&self, install_root: &str) {
        if !std::path::Path::new(install_root).exists() {
            let _ = std::fs::create_dir_all(install_root);
        }
        mark_path_ignored_by_cloud_sync(install_root);
        self.ensure_git_ignore(install_root);
        let package_json_path = node_join(install_root, &["package.json"]);
        if !std::path::Path::new(&package_json_path).exists() {
            let _ = std::fs::write(
                &package_json_path,
                "{\n  \"name\": \"pi-extensions\",\n  \"private\": true\n}",
            );
        }
    }

    fn ensure_git_ignore(&self, dir: &str) {
        if !std::path::Path::new(dir).exists() {
            let _ = std::fs::create_dir_all(dir);
        }
        let ignore_path = node_join(dir, &[".gitignore"]);
        if !std::path::Path::new(&ignore_path).exists() {
            let _ = std::fs::write(&ignore_path, "*\n!.gitignore\n");
        }
    }

    fn get_npm_install_root(&self, scope: SourceScope, temporary: bool) -> PmResult<String> {
        if temporary {
            return self.get_temporary_dir("npm", None);
        }
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(node_join(&self.cwd, &[CONFIG_DIR_NAME, "npm"]));
        }
        Ok(node_join(&self.agent_dir, &["npm"]))
    }

    fn get_global_npm_root(&self) -> PmResult<String> {
        let (command, args) = self.get_npm_command()?;
        let command_key = {
            let mut parts = vec![command.clone()];
            parts.extend(args.iter().cloned());
            parts.join("\0")
        };
        if let Some((root, cached_key)) = self
            .global_npm_root
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            if cached_key == command_key {
                return Ok(root);
            }
        }
        let root = if self.get_package_manager_name() == "bun" {
            let bin_dir = self.run_npm_command_sync(&[
                "pm".to_string(),
                "bin".to_string(),
                "-g".to_string(),
            ])?;
            let bin_dir = bin_dir.trim();
            node_join(
                &node_dirname(bin_dir),
                &["install", "global", "node_modules"],
            )
        } else {
            self.run_npm_command_sync(&["root".to_string(), "-g".to_string()])?
                .trim()
                .to_string()
        };
        *self
            .global_npm_root
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((root.clone(), command_key));
        Ok(root)
    }

    fn get_pnpm_global_package_path(&self, package_name: &str) -> PmResult<Option<String>> {
        if self.get_package_manager_name() != "pnpm" {
            return Ok(None);
        }

        let output = self.run_npm_command_sync(&[
            "list".to_string(),
            "-g".to_string(),
            "--depth".to_string(),
            "0".to_string(),
            "--json".to_string(),
        ])?;
        let parsed: serde_json::Value =
            serde_json::from_str(&output).map_err(|error| PmError::new(error.to_string()))?;
        let Some(entries) = parsed.as_array() else {
            return Err(PmError::new("not iterable"));
        };
        for entry in entries {
            if let Some(path) = entry
                .get("dependencies")
                .and_then(|dependencies| dependencies.get(package_name))
                .and_then(|package| package.get("path"))
                .and_then(|path| path.as_str())
            {
                return Ok(Some(path.to_string()));
            }
        }
        Ok(None)
    }

    fn get_managed_npm_install_path(
        &self,
        source: &NpmSource,
        scope: SourceScope,
    ) -> PmResult<String> {
        if scope == SourceScope::Temporary {
            return Ok(node_join(
                &self.get_temporary_dir("npm", None)?,
                &["node_modules", &source.name],
            ));
        }
        if scope == SourceScope::Project {
            self.assert_project_trusted_for_scope(scope)?;
            return Ok(node_join(
                &self.cwd,
                &[CONFIG_DIR_NAME, "npm", "node_modules", &source.name],
            ));
        }
        Ok(node_join(
            &self.agent_dir,
            &["npm", "node_modules", &source.name],
        ))
    }

    fn get_legacy_global_npm_install_path(&self, source: &NpmSource) -> Option<String> {
        // `getPnpmGlobalPackagePath(...) ?? join(getGlobalNpmRoot(), name)`
        // inside try/catch: a throw (pnpm list parse failure) yields
        // undefined without running the global-root lookup.
        match self.get_pnpm_global_package_path(&source.name) {
            Ok(Some(path)) => Some(path),
            Ok(None) => self
                .get_global_npm_root()
                .ok()
                .map(|root| node_join(&root, &[&source.name])),
            Err(_) => None,
        }
    }

    pub fn get_npm_install_path(&self, source: &NpmSource, scope: SourceScope) -> PmResult<String> {
        let managed_path = self.get_managed_npm_install_path(source, scope)?;
        if scope != SourceScope::User || std::path::Path::new(&managed_path).exists() {
            return Ok(managed_path);
        }
        match self.get_legacy_global_npm_install_path(source) {
            Some(legacy_path) if std::path::Path::new(&legacy_path).exists() => Ok(legacy_path),
            _ => Ok(managed_path),
        }
    }

    pub fn get_git_install_path(&self, source: &GitSource, scope: SourceScope) -> PmResult<String> {
        if scope == SourceScope::Temporary {
            return self.get_temporary_dir(&format!("git-{}", source.host), Some(&source.path));
        }
        let install_root = self
            .get_git_install_root(scope)
            .ok_or_else(|| PmError::new("Missing git install root"))?;
        self.resolve_managed_path(&install_root, &[&source.host, &source.path])
    }

    fn get_git_install_root(&self, scope: SourceScope) -> Option<String> {
        if scope == SourceScope::Temporary {
            return None;
        }
        if scope == SourceScope::Project {
            if self.assert_project_trusted_for_scope(scope).is_err() {
                return None;
            }
            return Some(node_join(&self.cwd, &[CONFIG_DIR_NAME, "git"]));
        }
        Some(node_join(&self.agent_dir, &["git"]))
    }

    fn get_temporary_dir(&self, prefix: &str, suffix: Option<&str>) -> PmResult<String> {
        let temp_folder = get_extension_temp_folder(&self.agent_dir);
        let root = self.resolve_managed_path(&temp_folder, &[prefix])?;
        let hash = sha2::Sha256::digest(format!("{}-{}", prefix, suffix.unwrap_or("")).as_bytes());
        let hash = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()[..8]
            .to_string();
        self.resolve_managed_path(&root, &[&hash, suffix.unwrap_or("")])
    }

    fn resolve_managed_path(&self, root: &str, parts: &[&str]) -> PmResult<String> {
        let resolved_root = node_resolve(&[root]);
        let resolved_path = node_resolve_under(&resolved_root, parts);
        if resolved_path != resolved_root
            && !resolved_path.starts_with(&format!("{}{}", resolved_root, node_sep()))
        {
            return Err(PmError::new(format!(
                "Refusing to use path outside package install root: {resolved_path}"
            )));
        }
        Ok(resolved_path)
    }

    fn get_base_dir_for_scope(&self, scope: SourceScope) -> String {
        if scope == SourceScope::Project {
            let _ = self.assert_project_trusted_for_scope(scope);
            return node_join(&self.cwd, &[CONFIG_DIR_NAME]);
        }
        if scope == SourceScope::User {
            return self.agent_dir.clone();
        }
        self.cwd.clone()
    }

    fn resolve_path(&self, input: &str) -> String {
        resolve_path_with(input, &self.cwd, &resolve_path_options(), cfg!(windows))
            .unwrap_or_else(|_| input.to_string())
    }

    fn resolve_path_from_base(&self, input: &str, base_dir: &str) -> String {
        resolve_path_with(input, base_dir, &resolve_path_options(), cfg!(windows))
            .unwrap_or_else(|_| input.to_string())
    }

    // -----------------------------------------------------------------
    // npm version lookups
    // -----------------------------------------------------------------

    fn installed_npm_matches_configured_version(
        &self,
        source: &NpmSource,
        installed_path: &str,
    ) -> bool {
        let Some(installed_version) = self.get_installed_npm_version(installed_path) else {
            return false;
        };
        match &source.range {
            Some(range) => vendor::range_satisfies(&installed_version, range),
            None => true,
        }
    }

    fn npm_has_available_update(&self, source: &NpmSource, installed_path: &str) -> PmResult<bool> {
        if is_offline_mode_enabled() {
            return Ok(false);
        }

        let Some(installed_version) = self.get_installed_npm_version(installed_path) else {
            return Ok(false);
        };

        let spec = source
            .version
            .as_deref()
            .map_or(source.name.as_str(), |_| source.spec.as_str());
        match self.get_latest_npm_version(spec, source.range.as_ref()) {
            Ok(target_version) => Ok(vendor::semver_gt(&target_version, &installed_version)),
            Err(_) => Ok(false),
        }
    }

    fn get_installed_npm_version(&self, installed_path: &str) -> Option<String> {
        let package_json_path = node_join(installed_path, &["package.json"]);
        if !std::path::Path::new(&package_json_path).exists() {
            return None;
        }
        let content = std::fs::read_to_string(&package_json_path).ok()?;
        let pkg: serde_json::Value = serde_json::from_str(strip_bom(&content)).ok()?;
        pkg.get("version")
            .and_then(|version| version.as_str())
            .map(str::to_string)
    }

    /// Upstream `getLatestNpmVersion`.
    pub fn get_latest_npm_version(
        &self,
        package_spec: &str,
        range: Option<&vendor::Range>,
    ) -> PmResult<String> {
        let (command, npm_args) = self.get_npm_command()?;
        let mut args = npm_args;
        args.extend([
            "view".to_string(),
            package_spec.to_string(),
            "version".to_string(),
            "--json".to_string(),
        ]);
        let stdout = self
            .runner
            .run_capture(
                &command,
                &args,
                Some(&self.cwd),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let raw = stdout.trim();
        if raw.is_empty() {
            return Err(PmError::new("Empty response from npm view"));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(raw).map_err(|error| PmError::new(error.to_string()))?;
        if let Some(version) = parsed.as_str() {
            return Ok(version.to_string());
        }
        if let Some(versions_value) = parsed.as_array() {
            let versions: Vec<&str> = versions_value
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|version| !version.is_empty())
                .collect();
            let latest = match range {
                Some(range) => vendor::max_satisfying(&versions, Some(range)),
                None => vendor::max_satisfying(&versions, None),
            };
            if let Some(latest) = latest {
                return Ok(latest);
            }
        }
        Err(PmError::new("Unexpected response from npm view"))
    }

    fn git_has_available_update(&self, installed_path: &str) -> PmResult<bool> {
        if is_offline_mode_enabled() {
            return Ok(false);
        }

        let local_head = self
            .runner
            .run_capture(
                "git",
                &["rev-parse".to_string(), "HEAD".to_string()],
                Some(installed_path),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let remote_head = self.get_remote_git_head(installed_path)?;
        Ok(local_head.trim() != remote_head.trim())
    }

    fn get_remote_git_head(&self, installed_path: &str) -> PmResult<String> {
        if let Some(upstream_ref) = self.get_git_upstream_ref(installed_path)? {
            let remote_head = self.run_git_remote_command(
                installed_path,
                &["ls-remote".to_string(), "origin".to_string(), upstream_ref],
            )?;
            if let Some(found) = find_remote_head(&remote_head, 40, None) {
                return Ok(found);
            }
        }

        let remote_head = self.run_git_remote_command(
            installed_path,
            &[
                "ls-remote".to_string(),
                "origin".to_string(),
                "HEAD".to_string(),
            ],
        )?;
        match find_remote_head(&remote_head, 40, Some("HEAD")) {
            Some(found) => Ok(found),
            None => Err(PmError::new("Failed to determine remote HEAD")),
        }
    }

    fn get_local_git_update_target(&self, installed_path: &str) -> PmResult<LocalGitUpdateTarget> {
        let upstream_result = self.runner.run_capture(
            "git",
            &[
                "rev-parse".to_string(),
                "--abbrev-ref".to_string(),
                "@{upstream}".to_string(),
            ],
            Some(installed_path),
            Some(NETWORK_TIMEOUT_MS),
            &[],
        );
        if let Ok(upstream) = upstream_result {
            let trimmed_upstream = upstream.trim();
            if !trimmed_upstream.starts_with("origin/") {
                // Falls through to the origin/HEAD fallback below.
            } else {
                let branch = &trimmed_upstream["origin/".len()..];
                if branch.is_empty() {
                    // Falls through.
                } else {
                    let head = self
                        .runner
                        .run_capture(
                            "git",
                            &["rev-parse".to_string(), "@{upstream}".to_string()],
                            Some(installed_path),
                            Some(NETWORK_TIMEOUT_MS),
                            &[],
                        )
                        .map_err(|error| PmError::new(error.message))?;
                    return Ok(LocalGitUpdateTarget {
                        ref_: "@{upstream}".to_string(),
                        head,
                        fetch_args: vec![
                            "fetch".to_string(),
                            "--prune".to_string(),
                            "--no-tags".to_string(),
                            "origin".to_string(),
                            format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
                        ],
                    });
                }
            }
        }

        let _ = self.runner.run(
            "git",
            &[
                "remote".to_string(),
                "set-head".to_string(),
                "origin".to_string(),
                "-a".to_string(),
            ],
            Some(installed_path),
        );
        let head = self
            .runner
            .run_capture(
                "git",
                &["rev-parse".to_string(), "origin/HEAD".to_string()],
                Some(installed_path),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let origin_head_ref = self
            .runner
            .run_capture(
                "git",
                &[
                    "symbolic-ref".to_string(),
                    "refs/remotes/origin/HEAD".to_string(),
                ],
                Some(installed_path),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .unwrap_or_default();
        let branch = origin_head_ref
            .trim()
            .strip_prefix("refs/remotes/origin/")
            .unwrap_or(origin_head_ref.trim())
            .to_string();
        if !branch.is_empty() {
            return Ok(LocalGitUpdateTarget {
                ref_: "origin/HEAD".to_string(),
                head,
                fetch_args: vec![
                    "fetch".to_string(),
                    "--prune".to_string(),
                    "--no-tags".to_string(),
                    "origin".to_string(),
                    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
                ],
            });
        }
        Ok(LocalGitUpdateTarget {
            ref_: "origin/HEAD".to_string(),
            head,
            fetch_args: vec![
                "fetch".to_string(),
                "--prune".to_string(),
                "--no-tags".to_string(),
                "origin".to_string(),
                "+HEAD:refs/remotes/origin/HEAD".to_string(),
            ],
        })
    }

    fn get_git_upstream_ref(&self, installed_path: &str) -> PmResult<Option<String>> {
        let upstream = self
            .runner
            .run_capture(
                "git",
                &[
                    "rev-parse".to_string(),
                    "--abbrev-ref".to_string(),
                    "@{upstream}".to_string(),
                ],
                Some(installed_path),
                Some(NETWORK_TIMEOUT_MS),
                &[],
            )
            .map_err(|error| PmError::new(error.message))?;
        let trimmed = upstream.trim();
        if !trimmed.starts_with("origin/") {
            return Ok(None);
        }
        let branch = &trimmed["origin/".len()..];
        Ok(if branch.is_empty() {
            None
        } else {
            Some(format!("refs/heads/{branch}"))
        })
    }

    fn run_git_remote_command(&self, installed_path: &str, args: &[String]) -> PmResult<String> {
        self.runner
            .run_capture(
                "git",
                args,
                Some(installed_path),
                Some(NETWORK_TIMEOUT_MS),
                &[("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())],
            )
            .map_err(|error| PmError::new(error.message))
    }

    // -----------------------------------------------------------------
    // Resource collection
    // -----------------------------------------------------------------

    fn collect_package_resources(
        &self,
        package_root: &str,
        accumulator: &mut ResourceAccumulator,
        filter: Option<&PackageFilter>,
        metadata: &PathMetadata,
    ) -> bool {
        if let Some(filter) = filter {
            for resource_type in RESOURCE_TYPES {
                let patterns = resource_filter_patterns(filter, resource_type);
                if filter.autoload == Some(false) {
                    self.apply_package_delta_filter(
                        package_root,
                        &patterns.unwrap_or_default(),
                        resource_type,
                        accumulator,
                        metadata,
                    );
                } else if let Some(patterns) = patterns {
                    self.apply_package_filter(
                        package_root,
                        &patterns,
                        resource_type,
                        accumulator,
                        metadata,
                    );
                } else {
                    self.collect_default_resources(
                        package_root,
                        resource_type,
                        accumulator,
                        metadata,
                    );
                }
            }
            return true;
        }

        let manifest = read_pi_manifest(&node_join(package_root, &["package.json"]));
        if let Some(manifest) = manifest {
            for resource_type in RESOURCE_TYPES {
                let entries = manifest_field(&manifest, resource_type);
                self.add_manifest_entries(
                    entries.as_deref(),
                    package_root,
                    resource_type,
                    accumulator,
                    metadata,
                );
            }
            return true;
        }

        let mut has_any_dir = false;
        for resource_type in RESOURCE_TYPES {
            let dir = node_join(package_root, &[resource_type]);
            if std::path::Path::new(&dir).exists() {
                // Collect all files from the directory (all enabled by default).
                let files = collect_resource_files(&dir, resource_type);
                for file in files {
                    accumulator
                        .target(resource_type)
                        .add(&file, metadata.clone(), true);
                }
                has_any_dir = true;
            }
        }
        has_any_dir
    }

    fn collect_default_resources(
        &self,
        package_root: &str,
        resource_type: &str,
        accumulator: &mut ResourceAccumulator,
        metadata: &PathMetadata,
    ) {
        let manifest = read_pi_manifest(&node_join(package_root, &["package.json"]));
        let entries = manifest.and_then(|manifest| manifest_field(&manifest, resource_type));
        if let Some(entries) = entries {
            self.add_manifest_entries(
                Some(&entries),
                package_root,
                resource_type,
                accumulator,
                metadata,
            );
            return;
        }
        let dir = node_join(package_root, &[resource_type]);
        if std::path::Path::new(&dir).exists() {
            let files = collect_resource_files(&dir, resource_type);
            for file in files {
                accumulator
                    .target(resource_type)
                    .add(&file, metadata.clone(), true);
            }
        }
    }

    fn apply_package_filter(
        &self,
        package_root: &str,
        user_patterns: &[String],
        resource_type: &str,
        accumulator: &mut ResourceAccumulator,
        metadata: &PathMetadata,
    ) {
        let all_files = self.collect_manifest_files(package_root, resource_type);

        if user_patterns.is_empty() {
            // Empty array explicitly disables all resources of this type.
            for file in all_files {
                accumulator
                    .target(resource_type)
                    .add(&file, metadata.clone(), false);
            }
            return;
        }

        let enabled_by_user = apply_patterns(&all_files, user_patterns, package_root);
        for file in all_files {
            let enabled = enabled_by_user.contains(&file);
            accumulator
                .target(resource_type)
                .add(&file, metadata.clone(), enabled);
        }
    }

    fn apply_package_delta_filter(
        &self,
        package_root: &str,
        user_patterns: &[String],
        resource_type: &str,
        accumulator: &mut ResourceAccumulator,
        metadata: &PathMetadata,
    ) {
        if user_patterns.is_empty() {
            return;
        }

        let all_files = self.collect_manifest_files(package_root, resource_type);
        let enabled_by_user =
            apply_autoload_disabled_patterns(&all_files, user_patterns, package_root);
        for (file_path, enabled) in enabled_by_user {
            accumulator
                .target(resource_type)
                .add(&file_path, metadata.clone(), enabled);
        }
    }

    /// Upstream `collectManifestFiles`: all manifest-passing files for a
    /// resource type.
    fn collect_manifest_files(&self, package_root: &str, resource_type: &str) -> Vec<String> {
        let manifest = read_pi_manifest(&node_join(package_root, &["package.json"]));
        let entries = manifest.and_then(|manifest| manifest_field(&manifest, resource_type));
        if let Some(entries) = entries.filter(|entries| !entries.is_empty()) {
            let all_files =
                self.collect_files_from_manifest_entries(&entries, package_root, resource_type);
            let manifest_patterns: Vec<String> = entries
                .iter()
                .filter(|entry| is_override_pattern(entry))
                .cloned()
                .collect();
            if manifest_patterns.is_empty() {
                return all_files;
            }
            return apply_patterns(&all_files, &manifest_patterns, package_root);
        }

        let convention_dir = node_join(package_root, &[resource_type]);
        if !std::path::Path::new(&convention_dir).exists() {
            return Vec::new();
        }
        collect_resource_files(&convention_dir, resource_type)
    }

    fn add_manifest_entries(
        &self,
        entries: Option<&[String]>,
        root: &str,
        resource_type: &str,
        accumulator: &mut ResourceAccumulator,
        metadata: &PathMetadata,
    ) {
        let Some(entries) = entries else {
            return;
        };

        let all_files = self.collect_files_from_manifest_entries(entries, root, resource_type);
        let patterns: Vec<String> = entries
            .iter()
            .filter(|entry| is_override_pattern(entry))
            .cloned()
            .collect();
        let enabled_paths = apply_patterns(&all_files, &patterns, root);

        for file in all_files {
            if enabled_paths.contains(&file) {
                accumulator
                    .target(resource_type)
                    .add(&file, metadata.clone(), true);
            }
        }
    }

    fn collect_files_from_manifest_entries(
        &self,
        entries: &[String],
        root: &str,
        resource_type: &str,
    ) -> Vec<String> {
        let source_entries: Vec<&String> = entries
            .iter()
            .filter(|entry| !is_override_pattern(entry))
            .collect();
        let mut resolved = Vec::new();
        for entry in source_entries {
            if !has_glob_pattern(entry) {
                resolved.push(node_resolve_under(root, &[entry]));
            } else {
                resolved.extend(expand_package_glob(entry, root));
            }
        }
        self.collect_files_from_paths(&resolved, resource_type)
    }

    fn resolve_local_entries(
        &self,
        entries: &[String],
        resource_type: &str,
        accumulator: &mut ResourceAccumulator,
        metadata: &PathMetadata,
        base_dir: &str,
    ) {
        if entries.is_empty() {
            return;
        }

        // Collect all files from plain entries (non-pattern entries).
        let (plain, patterns) = split_patterns(entries);
        let resolved_plain: Vec<String> = plain
            .iter()
            .map(|p| self.resolve_path_from_base(p, base_dir))
            .collect();
        let all_files = self.collect_files_from_paths(&resolved_plain, resource_type);

        let enabled_paths = apply_patterns(&all_files, &patterns, base_dir);

        for file in all_files {
            let enabled = enabled_paths.contains(&file);
            accumulator
                .target(resource_type)
                .add(&file, metadata.clone(), enabled);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_auto_discovered_resources(
        &self,
        accumulator: &mut ResourceAccumulator,
        global_settings: &SettingsData,
        project_settings: &SettingsData,
        global_base_dir: &str,
        project_base_dir: &str,
    ) {
        let user_metadata = PathMetadata {
            source: "auto".to_string(),
            scope: SourceScope::User,
            origin: PathMetadataOrigin::TopLevel,
            base_dir: Some(global_base_dir.to_string()),
        };
        let project_metadata = PathMetadata {
            source: "auto".to_string(),
            scope: SourceScope::Project,
            origin: PathMetadataOrigin::TopLevel,
            base_dir: Some(project_base_dir.to_string()),
        };

        let user_overrides = AutoOverrides {
            extensions: global_settings.extensions.clone(),
            skills: global_settings.skills.clone(),
            prompts: global_settings.prompts.clone(),
            themes: global_settings.themes.clone(),
        };
        let project_overrides = AutoOverrides {
            extensions: project_settings.extensions.clone(),
            skills: project_settings.skills.clone(),
            prompts: project_settings.prompts.clone(),
            themes: project_settings.themes.clone(),
        };

        let user_dirs = AutoDirs {
            extensions: node_join(global_base_dir, &["extensions"]),
            skills: node_join(global_base_dir, &["skills"]),
            prompts: node_join(global_base_dir, &["prompts"]),
            themes: node_join(global_base_dir, &["themes"]),
        };
        let project_dirs = AutoDirs {
            extensions: node_join(project_base_dir, &["extensions"]),
            skills: node_join(project_base_dir, &["skills"]),
            prompts: node_join(project_base_dir, &["prompts"]),
            themes: node_join(project_base_dir, &["themes"]),
        };
        let user_agents_skills_dir = node_join(&get_home_dir(), &[".agents", "skills"]);
        let project_trusted = self.settings_manager.is_project_trusted();
        let project_agents_skill_dirs: Vec<String> = if project_trusted {
            collect_ancestor_agents_skill_dirs(&self.cwd)
                .into_iter()
                .filter(|dir| node_resolve(&[dir]) != node_resolve(&[&user_agents_skills_dir]))
                .collect()
        } else {
            Vec::new()
        };

        let add_resources = |accumulator: &mut ResourceAccumulator,
                             resource_type: &str,
                             paths: Vec<String>,
                             metadata: &PathMetadata,
                             overrides: &[String],
                             base_dir: &str| {
            for path in paths {
                let enabled = is_enabled_by_overrides(&path, overrides, base_dir);
                accumulator
                    .target(resource_type)
                    .add(&path, metadata.clone(), enabled);
            }
        };

        if project_trusted {
            // Project extensions from .pi/
            add_resources(
                accumulator,
                "extensions",
                collect_auto_extension_entries(&project_dirs.extensions),
                &project_metadata,
                &project_overrides.extensions,
                project_base_dir,
            );

            // Project skills from .pi/
            add_resources(
                accumulator,
                "skills",
                collect_auto_skill_entries(&project_dirs.skills, "pi"),
                &project_metadata,
                &project_overrides.skills,
                project_base_dir,
            );
        }

        // Project skills from .agents/ (each with its own baseDir)
        for agents_skills_dir in &project_agents_skill_dirs {
            let agents_base_dir = node_dirname(agents_skills_dir); // the .agents directory
            let mut agents_metadata = project_metadata.clone();
            agents_metadata.base_dir = Some(agents_base_dir.clone());
            add_resources(
                accumulator,
                "skills",
                collect_auto_skill_entries(agents_skills_dir, "agents"),
                &agents_metadata,
                &project_overrides.skills,
                &agents_base_dir,
            );
        }

        if project_trusted {
            add_resources(
                accumulator,
                "prompts",
                collect_auto_prompt_entries(&project_dirs.prompts),
                &project_metadata,
                &project_overrides.prompts,
                project_base_dir,
            );
            add_resources(
                accumulator,
                "themes",
                collect_auto_theme_entries(&project_dirs.themes),
                &project_metadata,
                &project_overrides.themes,
                project_base_dir,
            );
        }

        // User extensions from ~/.pi/agent/
        add_resources(
            accumulator,
            "extensions",
            collect_auto_extension_entries(&user_dirs.extensions),
            &user_metadata,
            &user_overrides.extensions,
            global_base_dir,
        );

        // User skills from ~/.pi/agent/
        add_resources(
            accumulator,
            "skills",
            collect_auto_skill_entries(&user_dirs.skills, "pi"),
            &user_metadata,
            &user_overrides.skills,
            global_base_dir,
        );

        // User skills from ~/.agents/ (with its own baseDir)
        let user_agents_base_dir = node_dirname(&user_agents_skills_dir);
        let mut user_agents_metadata = user_metadata.clone();
        user_agents_metadata.base_dir = Some(user_agents_base_dir.clone());
        add_resources(
            accumulator,
            "skills",
            collect_auto_skill_entries(&user_agents_skills_dir, "agents"),
            &user_agents_metadata,
            &user_overrides.skills,
            &user_agents_base_dir,
        );

        add_resources(
            accumulator,
            "prompts",
            collect_auto_prompt_entries(&user_dirs.prompts),
            &user_metadata,
            &user_overrides.prompts,
            global_base_dir,
        );
        add_resources(
            accumulator,
            "themes",
            collect_auto_theme_entries(&user_dirs.themes),
            &user_metadata,
            &user_overrides.themes,
            global_base_dir,
        );
    }

    fn collect_files_from_paths(&self, paths: &[String], resource_type: &str) -> Vec<String> {
        let mut files = Vec::new();
        for path in paths {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            match entry_kind_of(std::path::Path::new(path)) {
                Some(EntryKind::File) => files.push(path.clone()),
                Some(EntryKind::Dir) => files.extend(collect_resource_files(path, resource_type)),
                None => {}
            }
        }
        files
    }

    fn to_resolved_paths(&self, accumulator: ResourceAccumulator) -> ResolvedPaths {
        let map_to_resolved = |entries: OrderedMap| -> Vec<ResolvedResource> {
            let mut resolved: Vec<ResolvedResource> = entries
                .entries
                .into_iter()
                .map(|(path, entry)| ResolvedResource {
                    path,
                    enabled: entry.enabled,
                    metadata: entry.metadata,
                })
                .collect();
            resolved.sort_by_key(|entry| resource_precedence_rank(&entry.metadata));

            let mut seen = std::collections::HashSet::new();
            resolved.retain(|entry| {
                let canonical_path = canonicalize_path(&entry.path);
                seen.insert(canonical_path)
            });
            resolved
        };

        ResolvedPaths {
            extensions: map_to_resolved(accumulator.extensions),
            skills: map_to_resolved(accumulator.skills),
            prompts: map_to_resolved(accumulator.prompts),
            themes: map_to_resolved(accumulator.themes),
        }
    }
}

struct LocalGitUpdateTarget {
    ref_: String,
    #[allow(dead_code)]
    head: String,
    fetch_args: Vec<String>,
}

struct AutoOverrides {
    extensions: Vec<String>,
    skills: Vec<String>,
    prompts: Vec<String>,
    themes: Vec<String>,
}

struct AutoDirs {
    extensions: String,
    skills: String,
    prompts: String,
    themes: String,
}

fn project_resource_entries(settings: &SettingsData, resource_type: &str) -> Vec<String> {
    match resource_type {
        "extensions" => settings.extensions.clone(),
        "skills" => settings.skills.clone(),
        "prompts" => settings.prompts.clone(),
        "themes" => settings.themes.clone(),
        _ => Vec::new(),
    }
}

fn resource_filter_patterns(filter: &PackageFilter, resource_type: &str) -> Option<Vec<String>> {
    match resource_type {
        "extensions" => filter.extensions.clone(),
        "skills" => filter.skills.clone(),
        "prompts" => filter.prompts.clone(),
        "themes" => filter.themes.clone(),
        _ => None,
    }
}

fn manifest_field(
    manifest: &crate::coding_agent::extensions::loader::PiManifest,
    resource_type: &str,
) -> Option<Vec<String>> {
    match resource_type {
        "extensions" => manifest.extensions.clone(),
        "skills" => manifest.skills.clone(),
        "prompts" => manifest.prompts.clone(),
        "themes" => manifest.themes.clone(),
        _ => None,
    }
}

/// `/^([0-9a-f]{40})\s+HEAD$/m` and `/^([0-9a-f]{40})\s+/m` captures.
fn find_remote_head(output: &str, hex_length: usize, suffix: Option<&str>) -> Option<String> {
    for line in output.lines() {
        let bytes = line.as_bytes();
        let hex_ok = bytes.len() >= hex_length
            && bytes[..hex_length]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
        if !hex_ok {
            continue;
        }
        let rest = &line[hex_length..];
        let matched = match suffix {
            Some(word) => {
                let trimmed = rest.trim_end_matches('\r');
                trimmed.starts_with(' ') && trimmed.trim() == word
            }
            None => rest.starts_with(' '),
        };
        if matched {
            return Some(line[..hex_length].to_string());
        }
    }
    None
}
