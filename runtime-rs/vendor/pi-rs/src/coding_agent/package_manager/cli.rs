//! Port of upstream `src/package-manager-cli.ts` (byte-identical copy under
//! `tests/fixtures/pm_oracle/src/`; oracle capture `cli.oracle.json` pins the help
//! texts, argument-error texts, list rendering, trust gates, self-update
//! plan messaging and managed-install flows).
//!
//! Design notes:
//!
//! - Output is returned as [`CommandOutcome`] (stdout lines, stderr lines,
//!   exit code) instead of writing to the real console / setting
//!   `process.exitCode`; the caller (a future `main` adapter) prints them.
//! - **chalk**: the port renders plain (uncolored) text, which is exactly
//!   what upstream produces on non-TTY streams — the oracle captures are
//!   byte-identical. ANSI styling for interactive terminals is a
//!   presentation concern not ported (disclosed).
//! - `process.exit(0)` in `handleConfigCommand` maps to a `handled` outcome
//!   with exit code 0.
//! - Everything requiring un-ported modules (version check network,
//!   ModelRuntime, config TUI, trust store, package dir/install-method
//!   detection, self-update spawning, pi-tui `Markdown`, `proper-lockfile`,
//!   installer artifact fetch, npm smoke test) travels through the
//!   [`PackageCommandHost`] trait with the exact upstream message formats
//!   reproduced here.

use std::sync::Arc;

use super::{node_join, node_resolve_under, DefaultPackageManager, PackageManagerOptions, PmError};
use crate::coding_agent::core::CONFIG_DIR_NAME;
use crate::coding_agent::utils::paths::{canonicalize_path, get_cwd_relative_path};

pub const APP_NAME: &str = "pi";
pub const PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";
const DEFAULT_INSTALLER_API_BASE: &str = "https://pi.dev/api/installer/releases";
const MANAGED_INSTALL_MARKER: &str = "managed-install.json";

// ===========================================================================
// Command surface types
// ===========================================================================

/// Upstream `PackageCommand`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageCommand {
    Install,
    Remove,
    Update,
    List,
}

impl PackageCommand {
    fn as_str(&self) -> &'static str {
        match self {
            PackageCommand::Install => "install",
            PackageCommand::Remove => "remove",
            PackageCommand::Update => "update",
            PackageCommand::List => "list",
        }
    }
}

/// Upstream `UpdateTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateTarget {
    All,
    SelfUpdate,
    Extensions { source: Option<String> },
    Models,
}

/// Upstream `PackageCommandOptions` (parse result).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCommandOptions {
    pub command: PackageCommand,
    pub source: Option<String>,
    pub update_target: Option<UpdateTarget>,
    pub show_extensions_skipped_note: bool,
    pub local: bool,
    pub force: bool,
    pub project_trust_override: Option<bool>,
    pub help: bool,
    pub invalid_option: Option<String>,
    pub invalid_argument: Option<String>,
    pub missing_option_value: Option<String>,
    pub conflicting_options: Option<String>,
}

/// Rendered command output (console.log/console.error/process.exitCode).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutcome {
    pub handled: bool,
    pub exit_code: i32,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

impl CommandOutcome {
    fn ok(stdout: Vec<String>) -> CommandOutcome {
        CommandOutcome {
            handled: true,
            exit_code: 0,
            stdout,
            stderr: Vec::new(),
        }
    }

    fn fail(stderr: Vec<String>) -> CommandOutcome {
        CommandOutcome {
            handled: true,
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        }
    }

    /// Joined stdout bytes (the oracle comparison target).
    pub fn stdout_joined(&self) -> String {
        self.stdout.join("\n")
    }

    /// Joined stderr bytes (the oracle comparison target).
    pub fn stderr_joined(&self) -> String {
        self.stderr.join("\n")
    }
}

// ===========================================================================
// Usage / help texts (byte-exact vs oracle)
// ===========================================================================

/// Upstream `getPackageCommandUsage`.
pub fn get_package_command_usage(command: PackageCommand) -> String {
    match command {
        PackageCommand::Install => {
            format!("{APP_NAME} install <source> [-l] [--approve|--no-approve]")
        }
        PackageCommand::Remove => {
            format!("{APP_NAME} remove <source> [-l] [--approve|--no-approve]")
        }
        PackageCommand::Update => format!(
            "{APP_NAME} update [source|self|pi] [--self|--extensions|--models|--all] [--extension <source>] [--approve|--no-approve] [--force]"
        ),
        PackageCommand::List => format!("{APP_NAME} list [--approve|--no-approve]"),
    }
}

pub const CONFIG_COMMAND_USAGE: &str = "pi config [-l] [--approve|--no-approve]";

/// Upstream `printConfigCommandHelp` (rendered text).
pub fn render_config_command_help() -> String {
    format!(
        "Usage:\n  {CONFIG_COMMAND_USAGE}\n\nOpen the resource configuration TUI to enable or disable package resources.\nWithout -l, starts in global settings (~/{CONFIG_DIR_NAME}/agent/settings.json).\nPress Tab in the TUI to switch between global and project-local modes.\n\nOptions:\n  -l, --local       Edit project overrides ({CONFIG_DIR_NAME}/settings.json)\n  -a, --approve     Trust project-local files for this command with -l\n  -na, --no-approve Ignore project-local files for this command with -l\n"
    )
}

/// Upstream `printPackageCommandHelp` (rendered text).
pub fn render_package_command_help(command: PackageCommand) -> String {
    match command {
        PackageCommand::Install => format!(
            "Usage:\n  {}\n\nInstall a package and add it to settings.\n\nOptions:\n  -l, --local       Install project-locally ({CONFIG_DIR_NAME}/settings.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP_NAME} install npm:@foo/bar\n  {APP_NAME} install git:github.com/user/repo\n  {APP_NAME} install git:git@github.com:user/repo\n  {APP_NAME} install https://github.com/user/repo\n  {APP_NAME} install ssh://git@github.com/user/repo\n  {APP_NAME} install ./local/path\n",
            get_package_command_usage(PackageCommand::Install)
        ),
        PackageCommand::Remove => format!(
            "Usage:\n  {}\n\nRemove a package and its source from settings.\nAlias: {APP_NAME} uninstall <source> [-l]\n\nOptions:\n  -l, --local       Remove from project settings ({CONFIG_DIR_NAME}/settings.json)\n  -a, --approve     Trust project-local files for this command\n  -na, --no-approve Ignore project-local files for this command\n\nExamples:\n  {APP_NAME} remove npm:@foo/bar\n  {APP_NAME} uninstall npm:@foo/bar\n",
            get_package_command_usage(PackageCommand::Remove)
        ),
        PackageCommand::Update => format!(
            "Usage:\n  {}\n\nUpdate pi, installed packages, or model catalogs.\n\nOptions:\n  --self                  Update pi only (default when no target is given)\n  --extensions            Update installed packages only\n  --models                Refresh model catalogs only\n  --all                   Update pi and installed packages\n  --extension <source>    Update one package only\n  -a, --approve           Trust project-local files for this command\n  -na, --no-approve       Ignore project-local files for this command\n  --force                 Reinstall pi even if the current version is latest\n\nShort forms:\n  {APP_NAME} update                Update pi only\n  {APP_NAME} update --all          Update pi and all extensions\n  {APP_NAME} update --models       Refresh model catalogs only\n  {APP_NAME} update <source>       Update one package\n  {APP_NAME} update pi             Update pi only (self works as alias to pi)\n",
            get_package_command_usage(PackageCommand::Update)
        ),
        PackageCommand::List => format!(
            "Usage:\n  {}\n\nList installed packages from user and project settings.\n\nOptions:\n  -a, --approve      Trust project-local files for this command\n  -na, --no-approve  Ignore project-local files for this command\n",
            get_package_command_usage(PackageCommand::List)
        ),
    }
}

// ===========================================================================
// Argument parsing
// ===========================================================================

/// Upstream `parsePackageCommand`.
pub fn parse_package_command(args: &[String]) -> Option<PackageCommandOptions> {
    let (raw_command, rest) = args.split_first()?;
    let command = match raw_command.as_str() {
        "uninstall" => PackageCommand::Remove,
        "install" => PackageCommand::Install,
        "remove" => PackageCommand::Remove,
        "update" => PackageCommand::Update,
        "list" => PackageCommand::List,
        _ => return None,
    };

    let mut local = false;
    let mut force = false;
    let mut project_trust_override: Option<bool> = None;
    let mut help = false;
    let mut invalid_option: Option<String> = None;
    let mut invalid_argument: Option<String> = None;
    let mut missing_option_value: Option<String> = None;
    let mut conflicting_options: Option<String> = None;
    let mut source: Option<String> = None;
    let mut self_flag = false;
    let mut extensions_flag = false;
    let mut models_flag = false;
    let mut all_flag = false;
    let mut extension_flag_source: Option<String> = None;

    let mut index = 0;
    while index < rest.len() {
        let arg = &rest[index];
        if arg == "-h" || arg == "--help" {
            help = true;
            index += 1;
            continue;
        }

        if arg == "-l" || arg == "--local" {
            if matches!(command, PackageCommand::Install | PackageCommand::Remove) {
                local = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--self" {
            if command == PackageCommand::Update {
                self_flag = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--extensions" {
            if command == PackageCommand::Update {
                extensions_flag = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--models" {
            if command == PackageCommand::Update {
                models_flag = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--all" {
            if command == PackageCommand::Update {
                all_flag = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--approve" || arg == "-a" {
            project_trust_override = Some(true);
            index += 1;
            continue;
        }

        if arg == "--no-approve" || arg == "-na" {
            project_trust_override = Some(false);
            index += 1;
            continue;
        }

        if arg == "--force" {
            if command == PackageCommand::Update {
                force = true;
            } else {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            }
            index += 1;
            continue;
        }

        if arg == "--extension" {
            if command != PackageCommand::Update {
                invalid_option = invalid_option.or_else(|| Some(arg.clone()));
                index += 1;
                continue;
            }

            let value = rest.get(index + 1);
            match value {
                None => {
                    missing_option_value = missing_option_value.or_else(|| Some(arg.clone()));
                }
                Some(value) if value.starts_with('-') => {
                    missing_option_value = missing_option_value.or_else(|| Some(arg.clone()));
                }
                Some(_) if extension_flag_source.is_some() => {
                    conflicting_options = conflicting_options
                        .or_else(|| Some("--extension can only be provided once".to_string()));
                    index += 1;
                }
                Some(value) => {
                    extension_flag_source = Some(value.clone());
                    index += 1;
                }
            }
            index += 1;
            continue;
        }

        if arg.starts_with('-') {
            invalid_option = invalid_option.or_else(|| Some(arg.clone()));
            index += 1;
            continue;
        }

        if source.is_none() {
            source = Some(arg.clone());
        } else {
            invalid_argument = invalid_argument.or_else(|| Some(arg.clone()));
        }
        index += 1;
    }

    let mut update_target: Option<UpdateTarget> = None;
    let mut show_extensions_skipped_note = false;
    if command == PackageCommand::Update {
        if all_flag
            && (self_flag || extensions_flag || models_flag || extension_flag_source.is_some())
        {
            conflicting_options = conflicting_options.or_else(|| {
                Some(
                    "--all cannot be combined with --self, --extensions, --models, or --extension"
                        .to_string(),
                )
            });
        }
        if all_flag && source.is_some() {
            conflicting_options = conflicting_options
                .or_else(|| Some("--all cannot be combined with a positional source".to_string()));
        }

        if models_flag {
            if self_flag || extensions_flag || all_flag || extension_flag_source.is_some() {
                conflicting_options = conflicting_options
                    .or_else(|| Some("--models cannot be combined with --self, --extensions, --all, or --extension".to_string()));
            }
            if source.is_some() {
                conflicting_options = conflicting_options.or_else(|| {
                    Some("--models cannot be combined with a positional source".to_string())
                });
            }
            update_target = Some(UpdateTarget::Models);
        } else if let Some(extension_source) = extension_flag_source.clone() {
            if self_flag || extensions_flag || all_flag {
                conflicting_options = conflicting_options.or_else(|| {
                    Some(
                        "--extension cannot be combined with --self, --extensions, or --all"
                            .to_string(),
                    )
                });
            }
            if source.is_some() {
                conflicting_options = conflicting_options.or_else(|| {
                    Some("--extension cannot be combined with a positional source".to_string())
                });
            }
            update_target = Some(UpdateTarget::Extensions {
                source: Some(extension_source),
            });
        } else if let Some(source) = source.clone() {
            let source_is_self = source == "self" || source == "pi";
            if source_is_self {
                update_target = Some(if extensions_flag {
                    UpdateTarget::All
                } else {
                    UpdateTarget::SelfUpdate
                });
            } else {
                if extensions_flag || self_flag || all_flag {
                    conflicting_options = conflicting_options
                        .or_else(|| Some("positional update targets cannot be combined with --self, --extensions, or --all".to_string()));
                }
                update_target = Some(UpdateTarget::Extensions {
                    source: Some(source),
                });
            }
        } else if all_flag || (self_flag && extensions_flag) {
            // `--all`, or `--self --extensions`, update everything.
            update_target = Some(UpdateTarget::All);
        } else if self_flag {
            update_target = Some(UpdateTarget::SelfUpdate);
        } else if extensions_flag {
            update_target = Some(UpdateTarget::Extensions { source: None });
        } else {
            update_target = Some(UpdateTarget::SelfUpdate);
            show_extensions_skipped_note = true;
        }
    }

    Some(PackageCommandOptions {
        command,
        source,
        update_target,
        show_extensions_skipped_note,
        local,
        force,
        project_trust_override,
        help,
        invalid_option,
        invalid_argument,
        missing_option_value,
        conflicting_options,
    })
}

/// Upstream `updateTargetIncludesSelf`.
fn update_target_includes_self(target: &UpdateTarget) -> bool {
    matches!(target, UpdateTarget::All | UpdateTarget::SelfUpdate)
}

/// Upstream `updateTargetIncludesExtensions`.
fn update_target_includes_extensions(target: &UpdateTarget) -> bool {
    matches!(target, UpdateTarget::All | UpdateTarget::Extensions { .. })
}

// ===========================================================================
// Host seam
// ===========================================================================

/// Upstream `LatestPiRelease` (utils/version-check.ts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatestPiRelease {
    pub version: String,
    pub package_name: Option<String>,
    pub note: Option<String>,
}

/// Upstream `SelfUpdatePackageTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdatePackageTarget {
    pub package_name: String,
    pub install_spec: String,
}

/// Upstream `SelfUpdateCommand`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfUpdateCommand {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    pub steps: Option<Vec<SelfUpdateCommand>>,
}

/// Acquired update lock (released on drop; upstream `proper-lockfile`).
pub struct UpdateLock {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl UpdateLock {
    pub fn new(release: Box<dyn FnOnce() + Send>) -> UpdateLock {
        UpdateLock {
            release: Some(release),
        }
    }

    pub fn noop() -> UpdateLock {
        UpdateLock { release: None }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// Runtime seam for everything outside the ported modules.
pub trait PackageCommandHost: Send + Sync {
    /// `process.cwd()`.
    fn cwd(&self) -> String;
    /// `getAgentDir()`.
    fn agent_dir(&self) -> String;
    /// `getPackageDir()`.
    fn package_dir(&self) -> String;
    /// `VERSION`.
    fn version(&self) -> String;
    /// `process.argv[1]` (executable location in the unavailable note).
    fn entrypoint(&self) -> Option<String> {
        None
    }
    /// `process.env[name]` (non-empty values).
    fn env_var(&self, name: &str) -> Option<String>;

    fn stdin_is_tty(&self) -> bool {
        false
    }

    fn stdout_is_tty(&self) -> bool {
        false
    }

    /// `process.stdout.columns`.
    fn stdout_columns(&self) -> Option<usize> {
        None
    }

    /// Command runner for package-manager spawns (and npm smoke tests).
    fn command_runner(&self) -> Arc<dyn super::CommandRunner>;

    /// In-memory settings manager mirroring upstream
    /// `SettingsManager.create(cwd, agentDir, { projectTrusted })`.
    fn create_settings_manager(
        &self,
        cwd: &str,
        agent_dir: &str,
        project_trusted: bool,
    ) -> Arc<dyn super::SettingsManagerHandle>;

    /// `ProjectTrustStore.get(cwd) === true`.
    fn saved_project_trusted(&self, cwd: &str) -> bool;

    /// `hasTrustRequiringProjectResources(cwd)`.
    fn has_trust_requiring_project_resources(&self, cwd: &str) -> bool {
        let _ = cwd;
        false
    }

    /// `DefaultResourceLoader.loadProjectTrustExtensions()` error surface
    /// (resource-loader seam): `(path, error)` pairs.
    fn load_project_trust_extension_errors(
        &self,
        cwd: &str,
        agent_dir: &str,
    ) -> Vec<(String, String)> {
        let _ = (cwd, agent_dir);
        Vec::new()
    }

    /// `resolveProjectTrusted({...})` (core/project-trust.ts seam).
    fn resolve_project_trusted(
        &self,
        cwd: &str,
        trust_override: Option<bool>,
        default_project_trust: Option<String>,
    ) -> bool;

    /// `settingsManager.drainErrors()` → `(scope, message)` pairs.
    fn drain_settings_errors(
        &self,
        settings: &Arc<dyn super::SettingsManagerHandle>,
    ) -> Vec<(String, String)>;

    /// `getDefaultProjectTrust()` snapshot for the trust resolution seam.
    fn default_project_trust(
        &self,
        settings: &Arc<dyn super::SettingsManagerHandle>,
    ) -> Option<String>;

    /// `getLatestPiRelease(VERSION, { retry: true })`; the error is already
    /// formatted through `formatVersionCheckError`.
    fn get_latest_pi_release(&self) -> Result<Option<LatestPiRelease>, String>;

    /// `ModelRuntime.refresh` ("update --models").
    fn refresh_model_catalogs(&self, agent_dir: &str) -> Result<(), String>;

    /// `detectInstallMethod()`.
    fn detect_install_method(&self) -> &'static str;

    /// `getSelfUpdateCommand(PACKAGE_NAME, npmCommand, target)`.
    fn self_update_command(
        &self,
        npm_command: Option<&[String]>,
        target: &SelfUpdatePackageTarget,
    ) -> Option<SelfUpdateCommand>;

    /// `getSelfUpdateUnavailableInstruction(PACKAGE_NAME, npmCommand, target)`.
    fn self_update_unavailable_instruction(
        &self,
        npm_command: Option<&[String]>,
        target: &SelfUpdatePackageTarget,
    ) -> String;

    /// `runSelfUpdate(command)`: spawn each step with inherited stdio.
    fn run_self_update(&self, command: &SelfUpdateCommand) -> Result<(), String>;

    /// `prepareWindowsNpmSelfUpdate()` (windows-self-update seam; no-op).
    fn prepare_windows_npm_self_update(&self) {}

    /// pi-tui `Markdown.render` seam; `None` reproduces the upstream
    /// fallback to the raw note.
    fn render_markdown_note(&self, note: &str, width: usize) -> Option<Vec<String>> {
        let _ = width;
        Some(note.split('\n').map(str::to_string).collect())
    }

    /// `fetchInstallerArtifact(url)` — installer release artifacts.
    fn fetch_installer_artifact(&self, url: &str) -> Result<String, String>;

    /// `runManagedNpmCi(stageDir)` — `npm ci ...` with inherited stdio;
    /// returns the exit code (or `None` for spawn failure).
    fn run_managed_npm_ci(&self, stage_dir: &str, args: &[String]) -> Result<(), Option<u32>>;

    /// Managed release smoke test: run `<bin> --version`, trimmed stdout.
    fn run_version_command(&self, bin_path: &str) -> Result<String, String>;

    /// `lockfile.lock(lockDir)`; `Err(true)` = ELOCKED conflict,
    /// `Err(false)` = other failure.
    fn lock_update(&self, managed_root: &str) -> Result<UpdateLock, bool> {
        let _ = managed_root;
        Ok(UpdateLock::noop())
    }

    /// `selectConfig({...})` (config TUI seam).
    fn select_config(
        &self,
        settings: &Arc<dyn super::SettingsManagerHandle>,
        global_resolved: &super::ResolvedPaths,
        project_resolved: &super::ResolvedPaths,
        write_scope_project: bool,
        project_mode_available: bool,
    ) {
        let _ = (
            settings,
            global_resolved,
            project_resolved,
            write_scope_project,
            project_mode_available,
        );
    }
}

// ===========================================================================
// Managed install helpers
// ===========================================================================

fn is_managed_release_version(version: &str) -> bool {
    // /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/
    let mut rest = version;
    for part in 0..3 {
        if part > 0 {
            rest = match rest.strip_prefix('.') {
                Some(rest) => rest,
                None => return false,
            };
        }
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        rest = &rest[digits..];
    }
    if let Some(pre) = rest.strip_prefix('-') {
        let length = pre
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .count();
        if length == 0 {
            return false;
        }
        rest = &pre[length..];
    }
    if let Some(build) = rest.strip_prefix('+') {
        let length = build
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .count();
        if length == 0 {
            return false;
        }
        rest = &build[length..];
    }
    rest.is_empty()
}

/// Upstream `getActiveManagedInstallRoot`.
pub fn get_active_managed_install_root(
    host: &dyn PackageCommandHost,
) -> Result<Option<String>, PmError> {
    let Some(configured_root) = host
        .env_var("PI_MANAGED_INSTALL_ROOT")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let managed_root = super::node_resolve(&[&configured_root]);
    let releases_dir = canonicalize_path(&node_join(&managed_root, &["releases"]));
    // The launcher environment is inherited by child processes. Do not
    // classify a source checkout or another Pi installation launched from
    // managed Pi as managed.
    let package_dir = canonicalize_path(&host.package_dir());
    if get_cwd_relative_path(&package_dir, &releases_dir)
        .ok()
        .flatten()
        .is_none()
    {
        return Ok(None);
    }

    let marker_path = node_join(&managed_root, &[MANAGED_INSTALL_MARKER]);
    let valid = std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .map(|marker| {
            marker.get("kind").and_then(|kind| kind.as_str()) == Some("pi-managed-install")
                && marker
                    .get("schemaVersion")
                    .and_then(|version| version.as_i64())
                    == Some(1)
                && marker.get("layout").and_then(|layout| layout.as_str()) == Some("releases-v1")
        })
        .unwrap_or(false);
    if !valid {
        return Err(PmError::new(format!(
            "Managed install marker is missing or invalid: {marker_path}"
        )));
    }

    Ok(Some(managed_root))
}

/// Upstream `verifyManagedRelease`.
fn verify_managed_release(
    host: &dyn PackageCommandHost,
    release_dir: &str,
    expected_version: &str,
) -> Result<(), PmError> {
    let bin_name = if cfg!(windows) {
        format!("{APP_NAME}.cmd")
    } else {
        APP_NAME.to_string()
    };
    let bin_path = node_resolve_under(release_dir, &["node_modules", ".bin", &bin_name]);
    let installed_version = host.run_version_command(&bin_path).map_err(|reason| {
        PmError::new(format!(
            "Could not verify managed Pi {expected_version}: {reason}"
        ))
    })?;
    if installed_version.trim() != expected_version {
        return Err(PmError::new(format!(
            "Managed Pi smoke test returned version {}; expected {expected_version}.",
            installed_version.trim()
        )));
    }
    Ok(())
}

/// Upstream `activateManagedRelease`.
fn activate_managed_release(host: &dyn PackageCommandHost, managed_root: &str, version: &str) {
    let current_path = node_join(managed_root, &["current-version"]);
    let unique = format!(
        "current-version.tmp.{}-{}",
        host_pid(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    );
    let _ = host;
    let temporary_path = node_join(managed_root, &[&unique]);
    let _ = std::fs::write(&temporary_path, format!("{version}\n"));
    let _ = std::fs::rename(&temporary_path, &current_path);
    let _ = std::fs::remove_file(&temporary_path);
}

fn host_pid() -> u32 {
    std::process::id()
}

/// Upstream `cleanupManagedStaging`.
fn cleanup_managed_staging(managed_root: &str) {
    let staging_root = node_join(managed_root, &["staging"]);
    let Ok(entries) = std::fs::read_dir(&staging_root) else {
        // The staging directory does not exist yet or is not writable.
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("update-") {
            let path = node_join(&staging_root, &[&name]);
            let _ = std::fs::remove_dir_all(&path);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Upstream `cleanupManagedInstall`.
pub fn cleanup_managed_install(host: &dyn PackageCommandHost) {
    let managed_root = match get_active_managed_install_root(host) {
        Ok(managed_root) => managed_root,
        Err(_) => return,
    };
    let Some(managed_root) = managed_root else {
        return;
    };

    let lock_dir = node_join(&managed_root, &["update"]);
    if let Ok(lock) = host.lock_update(&lock_dir) {
        cleanup_managed_staging(&managed_root);
        drop(lock);
    }
    // A live update owns the staging directory, or cleanup is unavailable.
}

/// Upstream `runManagedNpmCi` argument vector.
fn managed_npm_ci_args() -> Vec<String> {
    [
        "ci",
        "--ignore-scripts",
        "--min-release-age=0",
        "--omit=dev",
        "--include=optional",
        "--no-fund",
        "--no-audit",
        "--loglevel=error",
        "--progress=false",
    ]
    .iter()
    .map(|part| (*part).to_string())
    .collect()
}

/// Upstream `runManagedSelfUpdate`.
fn run_managed_self_update(
    host: &dyn PackageCommandHost,
    managed_root: &str,
    version: &str,
) -> Result<(), PmError> {
    if !is_managed_release_version(version) {
        return Err(PmError::new(format!(
            "Invalid managed release version: {version}"
        )));
    }

    let lock_dir = node_join(managed_root, &["update"]);
    let release_lock = match host.lock_update(&lock_dir) {
        Ok(lock) => lock,
        Err(true) => {
            return Err(PmError::new(
                "Another managed pi update is already running.",
            ));
        }
        Err(false) => return Err(PmError::new("Could not acquire update lock")),
    };

    let result = (|| -> Result<(), PmError> {
        cleanup_managed_staging(managed_root);
        let installer_api_base = host
            .env_var("PI_INSTALLER_API_BASE")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_INSTALLER_API_BASE.to_string())
            .trim_end_matches('/')
            .to_string();
        let release_url = format!("{installer_api_base}/{}", uri_encode_component(version));
        let staging_root = node_join(managed_root, &["staging"]);
        let releases_root = node_join(managed_root, &["releases"]);
        let _ = std::fs::create_dir_all(&releases_root);
        let release_dir = node_join(&releases_root, &[version]);
        if std::path::Path::new(&release_dir).exists() {
            verify_managed_release(host, &release_dir, version)?;
            activate_managed_release(host, managed_root, version);
            return Ok(());
        }

        let _ = std::fs::create_dir_all(&staging_root);
        let stage_dir = create_staging_dir(&staging_root)?;
        let stage_guard = StageGuard {
            stage_dir: stage_dir.clone(),
        };
        let package_json_content = host
            .fetch_installer_artifact(&format!("{release_url}/package.json"))
            .map_err(PmError::new)?;
        let package_lock_content = host
            .fetch_installer_artifact(&format!("{release_url}/package-lock.json"))
            .map_err(PmError::new)?;
        std::fs::write(
            node_join(&stage_dir, &["package.json"]),
            package_json_content,
        )
        .map_err(|error| PmError::new(error.to_string()))?;
        std::fs::write(
            node_join(&stage_dir, &["package-lock.json"]),
            package_lock_content,
        )
        .map_err(|error| PmError::new(error.to_string()))?;

        host.run_managed_npm_ci(&stage_dir, &managed_npm_ci_args())
            .map_err(|code| {
                PmError::new(format!(
                    "npm {} exited with code {}",
                    managed_npm_ci_args().join(" "),
                    code.map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ))
            })?;
        verify_managed_release(host, &stage_dir, version)?;
        std::fs::rename(&stage_dir, &release_dir)
            .map_err(|error| PmError::new(error.to_string()))?;
        drop(stage_guard);
        activate_managed_release(host, managed_root, version);
        Ok(())
    })();

    drop(release_lock);
    result
}

struct StageGuard {
    stage_dir: String,
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.stage_dir);
    }
}

/// `mkdtempSync(join(stagingRoot, "update-"))` equivalent (unique name
/// format is not pinned by the oracle).
fn create_staging_dir(staging_root: &str) -> Result<String, PmError> {
    for attempt in 0..1000u32 {
        let candidate = node_join(
            staging_root,
            &[&format!("update-{}-{:06}", host_pid(), attempt)],
        );
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PmError::new(error.to_string())),
        }
    }
    Err(PmError::new("Could not create staging directory"))
}

/// `encodeURIComponent` for the version strings accepted above.
fn uri_encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        let character = byte as char;
        if character.is_ascii_alphanumeric()
            || matches!(
                character,
                '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')'
            )
        {
            out.push(character);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ===========================================================================
// Self-update plan
// ===========================================================================

struct SelfUpdatePlan {
    package_name: String,
    install_spec: String,
    version: String,
    should_run: bool,
    note: Option<String>,
}

/// Upstream `isNewerPackageVersion` (utils/version-check.ts).
fn is_newer_package_version(candidate_version: &str, current_version: &str) -> bool {
    match (
        super::vendor::SemVer::parse(candidate_version.trim()),
        super::vendor::SemVer::parse(current_version.trim()),
    ) {
        (Some(candidate), Some(current)) => {
            candidate.cmp_version(&current) == std::cmp::Ordering::Greater
        }
        _ => candidate_version.trim() != current_version.trim(),
    }
}

fn get_self_update_plan(
    host: &dyn PackageCommandHost,
    stdout: &mut Vec<String>,
    force: bool,
) -> Result<SelfUpdatePlan, PmError> {
    let version = host.version();
    let latest_release = host
        .get_latest_pi_release()
        .map_err(|error| {
            PmError::new(format!(
                "Could not determine latest {APP_NAME} version: {error}"
            ))
        })?
        .ok_or_else(|| PmError::new(format!("Could not determine latest {APP_NAME} version.")))?;

    let package_name = latest_release
        .package_name
        .clone()
        .unwrap_or_else(|| PACKAGE_NAME.to_string());
    let install_spec = format!("{}@{}", package_name, latest_release.version);
    if force
        || package_name != PACKAGE_NAME
        || is_newer_package_version(&latest_release.version, &version)
    {
        return Ok(SelfUpdatePlan {
            package_name,
            install_spec,
            version: latest_release.version,
            note: latest_release.note,
            should_run: true,
        });
    }

    stdout.push(format!("{APP_NAME} is already up to date (v{version})"));
    Ok(SelfUpdatePlan {
        package_name,
        install_spec,
        version: latest_release.version,
        note: None,
        should_run: false,
    })
}

/// Upstream `printSelfUpdateNote`.
fn print_self_update_note(host: &dyn PackageCommandHost, stdout: &mut Vec<String>, note: &str) {
    let trimmed = note.trim();
    if trimmed.is_empty() {
        return;
    }

    stdout.push(String::new());
    stdout.push("Update note".to_string());
    let width = host.stdout_columns().unwrap_or(80).max(20);
    match host.render_markdown_note(trimmed, width) {
        Some(lines) => {
            let rendered: Vec<String> = lines
                .into_iter()
                .map(|line| line.trim_end().to_string())
                .collect();
            stdout.push(rendered.join("\n"));
        }
        None => stdout.push(trimmed.to_string()),
    }
    stdout.push(String::new());
}

// ===========================================================================
// Command handling
// ===========================================================================

fn usage_error(first: String, second: String) -> CommandOutcome {
    CommandOutcome::fail(vec![first, second])
}

fn usage_lines(command: PackageCommand) -> String {
    get_package_command_usage(command)
}

struct CommandContext {
    settings: Arc<dyn super::SettingsManagerHandle>,
    project_trust_warnings: Vec<String>,
}

/// Upstream `createCommandSettingsManager`.
fn create_command_settings_manager(
    host: &dyn PackageCommandHost,
    project_trust_override: Option<bool>,
    use_saved_project_trust_only: bool,
) -> CommandContext {
    let cwd = host.cwd();
    let agent_dir = host.agent_dir();
    let settings = host.create_settings_manager(&cwd, &agent_dir, false);
    let mut project_trust_warnings: Vec<String> = Vec::new();
    if use_saved_project_trust_only {
        let saved_project_trusted = host.saved_project_trusted(&cwd);
        let trusted = project_trust_override.unwrap_or(saved_project_trusted);
        set_project_trusted(&settings, trusted);
        return CommandContext {
            settings,
            project_trust_warnings,
        };
    }

    let extensions_errors =
        if project_trust_override.is_none() && host.has_trust_requiring_project_resources(&cwd) {
            // Resource-loader trust-extension preload seam (not ported): the
            // host reports load failures, if any.
            host.load_project_trust_extension_errors(&cwd, &agent_dir)
        } else {
            Vec::new()
        };
    for (path, error) in extensions_errors {
        project_trust_warnings.push(format!("Failed to load extension \"{path}\": {error}"));
    }

    let project_trusted = host.resolve_project_trusted(
        &cwd,
        project_trust_override,
        host.default_project_trust(&settings),
    );
    set_project_trusted(&settings, project_trusted);
    CommandContext {
        settings,
        project_trust_warnings,
    }
}

fn set_project_trusted(settings: &Arc<dyn super::SettingsManagerHandle>, trusted: bool) {
    // `setProjectTrusted` is part of the settings seam used by the CLI.
    settings.set_project_trusted(trusted);
}

fn report_project_trust_warnings(outcome: &mut CommandOutcome, warnings: &[String]) {
    for warning in warnings {
        outcome.stderr.push(format!("Warning: {warning}"));
    }
}

fn report_settings_errors(
    host: &dyn PackageCommandHost,
    outcome: &mut CommandOutcome,
    context: &str,
    settings: &Arc<dyn super::SettingsManagerHandle>,
) {
    for (scope, message) in host.drain_settings_errors(settings) {
        outcome
            .stderr
            .push(format!("Warning ({context}, {scope} settings): {message}"));
    }
}

/// Upstream `handlePackageCommand`.
pub fn handle_package_command(
    host: &dyn PackageCommandHost,
    args: &[String],
) -> Option<CommandOutcome> {
    let options = parse_package_command(args)?;
    let mut outcome = CommandOutcome::default();

    if options.help {
        outcome
            .stdout
            .push(render_package_command_help(options.command));
        outcome.handled = true;
        return Some(outcome);
    }

    if let Some(invalid_option) = &options.invalid_option {
        return Some(usage_error(
            format!(
                "Unknown option {invalid_option} for \"{}\".",
                options.command.as_str()
            ),
            format!(
                "Use \"{APP_NAME} --help\" or \"{}\".",
                usage_lines(options.command)
            ),
        ));
    }

    if let Some(missing_option_value) = &options.missing_option_value {
        return Some(usage_error(
            format!("Missing value for {missing_option_value}."),
            format!("Usage: {}", usage_lines(options.command)),
        ));
    }

    if let Some(invalid_argument) = &options.invalid_argument {
        return Some(usage_error(
            format!("Unexpected argument {invalid_argument}."),
            format!("Usage: {}", usage_lines(options.command)),
        ));
    }

    if let Some(conflicting_options) = &options.conflicting_options {
        return Some(usage_error(
            conflicting_options.clone(),
            format!("Usage: {}", usage_lines(options.command)),
        ));
    }

    let source = options.source.clone();
    if matches!(
        options.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && source.is_none()
    {
        return Some(usage_error(
            format!("Missing {} source.", options.command.as_str()),
            format!("Usage: {}", usage_lines(options.command)),
        ));
    }

    if options.command == PackageCommand::Update
        && options.update_target.as_ref() == Some(&UpdateTarget::Models)
    {
        return Some(match host.refresh_model_catalogs(&host.agent_dir()) {
            Ok(()) => CommandOutcome::ok(vec!["Model catalogs refreshed".to_string()]),
            Err(message) => CommandOutcome::fail(vec![format!("Error: {message}")]),
        });
    }

    let cwd = host.cwd();
    let agent_dir = host.agent_dir();
    let writes_project_package_config = matches!(
        options.command,
        PackageCommand::Install | PackageCommand::Remove
    ) && options.local;
    let context = create_command_settings_manager(
        host,
        options.project_trust_override,
        options.command == PackageCommand::Update,
    );
    report_project_trust_warnings(&mut outcome, &context.project_trust_warnings);
    if !context.settings.is_project_trusted() && writes_project_package_config {
        return Some(CommandOutcome::fail(vec![
            "Project is not trusted. Use --approve to modify local package config.".to_string(),
        ]));
    }
    report_settings_errors(host, &mut outcome, "package command", &context.settings);
    let self_update_npm_command = context.settings.global_settings().npm_command.clone();

    let package_manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd,
        agent_dir,
        settings_manager: context.settings.clone(),
        command_runner: Some(host.command_runner()),
    });

    let progress_host_out = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let progress_host_out = Arc::clone(&progress_host_out);
        package_manager.set_progress_callback(Some(Arc::new(move |event| {
            if event.event_type == "start" {
                if let Ok(mut guard) = progress_host_out.lock() {
                    guard.push(format!("{}\n", event.message.clone().unwrap_or_default()));
                }
            }
        })));
    }

    let result: Result<Option<CommandOutcome>, PmError> = (|| match options.command {
        PackageCommand::Install => {
            let source = source.unwrap_or_default();
            package_manager.install_and_persist(&source, options.local)?;
            Ok(Some(CommandOutcome::ok(vec![format!(
                "Installed {source}"
            )])))
        }

        PackageCommand::Remove => {
            let source = source.unwrap_or_default();
            let removed = package_manager.remove_and_persist(&source, options.local)?;
            if !removed {
                return Ok(Some(CommandOutcome::fail(vec![format!(
                    "No matching package found for {source}"
                )])));
            }
            Ok(Some(CommandOutcome::ok(vec![format!("Removed {source}")])))
        }

        PackageCommand::List => {
            let configured_packages = package_manager.list_configured_packages();
            if configured_packages.is_empty() {
                return Ok(Some(CommandOutcome::ok(vec![
                    "No packages installed.".to_string()
                ])));
            }
            let mut stdout: Vec<String> = Vec::new();
            let user_packages: Vec<_> = configured_packages
                .iter()
                .filter(|pkg| pkg.scope == super::SourceScope::User)
                .collect();
            let project_packages: Vec<_> = configured_packages
                .iter()
                .filter(|pkg| pkg.scope == super::SourceScope::Project)
                .collect();

            let format_package = |stdout: &mut Vec<String>, pkg: &super::ConfiguredPackage| {
                let display = if pkg.filtered {
                    format!("{} (filtered)", pkg.source)
                } else {
                    pkg.source.clone()
                };
                stdout.push(format!("  {display}"));
                if let Some(installed_path) = &pkg.installed_path {
                    stdout.push(format!("    {installed_path}"));
                }
            };

            if !user_packages.is_empty() {
                stdout.push("User packages:".to_string());
                for pkg in &user_packages {
                    format_package(&mut stdout, pkg);
                }
            }

            if !project_packages.is_empty() {
                if !user_packages.is_empty() {
                    stdout.push(String::new());
                }
                stdout.push("Project packages:".to_string());
                for pkg in &project_packages {
                    format_package(&mut stdout, pkg);
                }
            }

            Ok(Some(CommandOutcome::ok(stdout)))
        }

        PackageCommand::Update => {
            let mut stdout: Vec<String> = Vec::new();
            let target = options
                .update_target
                .clone()
                .unwrap_or(UpdateTarget::SelfUpdate);
            if options.show_extensions_skipped_note {
                stdout.push(format!(
                        "Extensions are skipped. Run {APP_NAME} update --extensions to update extensions."
                    ));
            }
            if update_target_includes_extensions(&target) {
                let update_source = match &target {
                    UpdateTarget::Extensions { source } => source.clone(),
                    _ => None,
                };
                package_manager.update(update_source.as_deref())?;
                if let Some(update_source) = update_source {
                    stdout.push(format!("Updated {update_source}"));
                } else {
                    stdout.push("Updated packages".to_string());
                }
            }
            if update_target_includes_self(&target) {
                let managed_install_root = get_active_managed_install_root(host)?;
                if managed_install_root.is_some() && options.force {
                    return Ok(Some(CommandOutcome::fail(vec![format!(
                        "Managed {APP_NAME} installations do not support --force; rerun the installer to repair this installation."
                    )])));
                }
                let self_update_plan = get_self_update_plan(host, &mut stdout, options.force)?;
                if !self_update_plan.should_run {
                    return Ok(Some(CommandOutcome::ok(stdout)));
                }
                if let Some(managed_install_root) = managed_install_root {
                    if let Some(note) = &self_update_plan.note {
                        print_self_update_note(host, &mut stdout, note);
                    }
                    stdout.push(format!("Updating managed {APP_NAME} installation..."));
                    if let Err(error) = run_managed_self_update(
                        host,
                        &managed_install_root,
                        &self_update_plan.version,
                    ) {
                        return Ok(Some(CommandOutcome {
                            handled: true,
                            exit_code: 1,
                            stdout,
                            stderr: vec![format!("Error: {}", error.message)],
                        }));
                    }
                    stdout.push(format!(
                        "Updated {APP_NAME} from {} to {}",
                        host.version(),
                        self_update_plan.version
                    ));
                    return Ok(Some(CommandOutcome::ok(stdout)));
                }

                let install_method = host.detect_install_method();
                if cfg!(windows) && install_method != "npm" && install_method != "pnpm" {
                    let mut stderr = vec![format!(
                        "{APP_NAME} self-update on Windows is only supported for npm and pnpm installs."
                    )];
                    stderr.push(format!(
                        "Detected install method: {install_method}. Update {APP_NAME} manually."
                    ));
                    return Ok(Some(CommandOutcome {
                        handled: true,
                        exit_code: 1,
                        stdout,
                        stderr,
                    }));
                }
                let self_update_target = SelfUpdatePackageTarget {
                    package_name: self_update_plan.package_name.clone(),
                    install_spec: self_update_plan.install_spec.clone(),
                };
                let self_update_command = host
                    .self_update_command(self_update_npm_command.as_deref(), &self_update_target);
                let Some(self_update_command) = self_update_command else {
                    let mut stderr = vec![
                        format!("error: {APP_NAME} cannot self-update this installation."),
                        host.self_update_unavailable_instruction(
                            self_update_npm_command.as_deref(),
                            &self_update_target,
                        ),
                    ];
                    if let Some(entrypoint) = host.entrypoint() {
                        stderr.push(String::new());
                        stderr.push(format!("Location of {APP_NAME} executable: {entrypoint}"));
                    }
                    return Ok(Some(CommandOutcome {
                        handled: true,
                        exit_code: 1,
                        stdout,
                        stderr,
                    }));
                };
                if let Some(note) = &self_update_plan.note {
                    print_self_update_note(host, &mut stdout, note);
                }
                if install_method == "npm" {
                    host.prepare_windows_npm_self_update();
                }
                if let Err(message) = host.run_self_update(&self_update_command) {
                    let mut stderr = vec![format!("Error: {message}")];
                    if install_method == "pnpm" {
                        stderr.push(
                                "If pnpm reports missing package versions, its cached registry metadata may be stale.".to_string(),
                            );
                        stderr.push(format!(
                            "Run `pnpm store prune` and retry `{APP_NAME} update --self`."
                        ));
                    }
                    stderr.push(format!(
                        "If this keeps failing, run this command yourself: {}",
                        self_update_command.display
                    ));
                    return Ok(Some(CommandOutcome {
                        handled: true,
                        exit_code: 1,
                        stdout,
                        stderr,
                    }));
                }
                stdout.push(format!(
                    "Updated {APP_NAME} from {} to {}",
                    host.version(),
                    self_update_plan.version
                ));
                // Upstream v1.0.0: the pi.dev installer migrates global npm installs to a managed
                // install that pins all dependencies. It does not migrate pnpm, yarn, or bun.
                if install_method == "npm" {
                    let installer_command = if cfg!(windows) {
                        "powershell -c \"irm https://pi.dev/install.ps1 | iex\""
                    } else {
                        "curl -fsSL https://pi.dev/install.sh | sh"
                    };
                    stdout.push(String::new());
                    stdout.push(format!(
                        "This npm installation of {APP_NAME} does not pin its dependencies."
                    ));
                    stdout.push(
                        "Run the installer to migrate to a managed installation that does:"
                            .to_string(),
                    );
                    stdout.push(String::new());
                    stdout.push(format!("  {installer_command}"));
                }
            }
            Ok(Some(CommandOutcome::ok(stdout)))
        }
    })();

    match result {
        Ok(Some(mut command_outcome)) => {
            if let Ok(mut progress) = progress_host_out.lock() {
                progress.append(&mut command_outcome.stdout);
                command_outcome.stdout = std::mem::take(&mut *progress);
            }
            command_outcome.handled = true;
            Some(command_outcome)
        }
        Ok(None) => None,
        Err(error) => {
            if let Ok(progress) = progress_host_out.lock() {
                outcome.stdout.extend(progress.iter().cloned());
            }
            outcome.stderr.push(format!("Error: {}", error.message));
            outcome.exit_code = 1;
            outcome.handled = true;
            Some(outcome)
        }
    }
}

/// Upstream `handleConfigCommand`.
pub fn handle_config_command(
    host: &dyn PackageCommandHost,
    args: &[String],
) -> Option<CommandOutcome> {
    let (command, rest) = args.split_first()?;
    if command != "config" {
        return None;
    }
    let mut outcome = CommandOutcome::default();

    if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        outcome.stdout.push(render_config_command_help());
        outcome.handled = true;
        return Some(outcome);
    }

    let mut local = false;
    let mut project_trust_override: Option<bool> = None;
    for arg in rest {
        if arg == "-l" || arg == "--local" {
            local = true;
        } else if arg == "-a" || arg == "--approve" {
            project_trust_override = Some(true);
        } else if arg == "-na" || arg == "--no-approve" {
            project_trust_override = Some(false);
        } else if arg.starts_with('-') {
            outcome
                .stderr
                .push(format!("Unknown option {arg} for \"config\"."));
            outcome.stderr.push(format!(
                "Use \"{APP_NAME} --help\" or \"{CONFIG_COMMAND_USAGE}\"."
            ));
            outcome.exit_code = 1;
            outcome.handled = true;
            return Some(outcome);
        } else {
            outcome.stderr.push(format!("Unexpected argument {arg}."));
            outcome
                .stderr
                .push(format!("Usage: {CONFIG_COMMAND_USAGE}"));
            outcome.exit_code = 1;
            outcome.handled = true;
            return Some(outcome);
        }
    }

    let cwd = host.cwd();
    let agent_dir = host.agent_dir();
    let context = create_command_settings_manager(host, project_trust_override, false);
    report_project_trust_warnings(&mut outcome, &context.project_trust_warnings);
    if local && !context.settings.is_project_trusted() {
        outcome.stderr.push(
            "Project is not trusted. Use --approve to modify local resource config.".to_string(),
        );
        outcome.exit_code = 1;
        outcome.handled = true;
        return Some(outcome);
    }
    report_settings_errors(host, &mut outcome, "config command", &context.settings);

    let global_settings = host.create_settings_manager(&cwd, &agent_dir, false);
    let global_manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd: cwd.clone(),
        agent_dir: agent_dir.clone(),
        settings_manager: global_settings.clone(),
        command_runner: Some(host.command_runner()),
    });
    let global_resolved = global_manager.resolve(None).unwrap_or_default();
    let project_resolved = if context.settings.is_project_trusted() {
        DefaultPackageManager::new(PackageManagerOptions {
            cwd: cwd.clone(),
            agent_dir: agent_dir.clone(),
            settings_manager: context.settings.clone(),
            command_runner: Some(host.command_runner()),
        })
        .resolve(None)
        .unwrap_or_default()
    } else {
        global_resolved.clone()
    };

    host.select_config(
        &context.settings,
        &global_resolved,
        &project_resolved,
        local,
        context.settings.is_project_trusted(),
    );

    // `process.exit(0)`
    outcome.exit_code = 0;
    outcome.handled = true;
    Some(outcome)
}

// ===========================================================================
// Test-facing wrappers (upstream drives these private functions through the
// TS `private` erasure in its suite; the port exposes equivalents)
// ===========================================================================

#[doc(hidden)]
pub fn cli_is_managed_release_version(version: &str) -> bool {
    is_managed_release_version(version)
}

#[doc(hidden)]
pub fn run_managed_self_update_for_test(
    host: &dyn PackageCommandHost,
    managed_root: &str,
    version: &str,
) -> Result<(), PmError> {
    run_managed_self_update(host, managed_root, version)
}

#[doc(hidden)]
pub fn print_self_update_note_for_test(
    host: &dyn PackageCommandHost,
    stdout: &mut Vec<String>,
    note: &str,
) {
    print_self_update_note(host, stdout, note)
}

#[doc(hidden)]
pub fn get_self_update_plan_for_test(
    host: &dyn PackageCommandHost,
    stdout: &mut Vec<String>,
    force: bool,
) -> Result<SelfUpdatePlanForTest, PmError> {
    get_self_update_plan(host, stdout, force).map(|plan| SelfUpdatePlanForTest {
        package_name: plan.package_name,
        install_spec: plan.install_spec,
        version: plan.version,
        should_run: plan.should_run,
        note: plan.note,
    })
}

/// Public mirror of the private `SelfUpdatePlan` for tests/hosts.
pub struct SelfUpdatePlanForTest {
    pub package_name: String,
    pub install_spec: String,
    pub version: String,
    pub should_run: bool,
    pub note: Option<String>,
}
