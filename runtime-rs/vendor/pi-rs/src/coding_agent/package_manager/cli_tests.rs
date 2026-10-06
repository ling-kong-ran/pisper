//! Tests for the package-manager CLI port.
//!
//! Layer 1: every entry of `tests/fixtures/pm_oracle/cli.oracle.json` (captured by
//! running the byte-identical upstream `package-manager-cli.ts` under node
//! with stubbed externals) is replayed against a scripted
//! [`TestHost`]; joined stdout/stderr bytes and the exit code must match.
//!
//! Layer 2: `parsePackageCommand` argument-surface ports (target
//! resolution, conflict precedence) beyond what the error battery pins.
//!
//! Disclosed: `printSelfUpdateNote` markdown rendering (pi-tui `Markdown`
//! lives in the TUI slice) travels through the host seam and is covered
//! structurally here, not byte-pinned against the real renderer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use super::cli::{
    self, CommandOutcome, LatestPiRelease, PackageCommand, PackageCommandHost, SelfUpdateCommand,
    SelfUpdatePackageTarget, UpdateLock, PACKAGE_NAME,
};
use super::{
    DefaultPackageManager, PackageManagerOptions, PackageSourceEntry, ResolvedPaths, SettingsData,
    SettingsManagerHandle,
};

use super::tests::{env_lock, FakeRunner, SpawnOutcome};

// ===========================================================================
// File-backed settings fixture (mirrors the oracle settings stub)
// ===========================================================================

struct FileSettingsState {
    global: SettingsData,
    project: SettingsData,
    project_trusted: bool,
}

struct FileSettings {
    state: Mutex<FileSettingsState>,
}

fn read_settings_file(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

fn settings_from_value(value: &Value) -> SettingsData {
    SettingsData {
        packages: value
            .get("packages")
            .and_then(Value::as_array)
            .map(|packages| {
                packages
                    .iter()
                    .map(|entry| {
                        serde_json::from_value::<PackageSourceEntry>(entry.clone()).unwrap()
                    })
                    .collect()
            })
            .unwrap_or_default(),
        extensions: string_list(value, "extensions"),
        skills: string_list(value, "skills"),
        prompts: string_list(value, "prompts"),
        themes: string_list(value, "themes"),
        npm_command: value
            .get("npmCommand")
            .and_then(Value::as_array)
            .map(|command| {
                command
                    .iter()
                    .map(|part| part.as_str().unwrap_or_default().to_string())
                    .collect()
            }),
    }
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

impl FileSettings {
    fn create(cwd: &Path, agent_dir: &Path, project_trusted: bool) -> Arc<FileSettings> {
        // Mirrors the capture stub: settings files load at construction
        // (regardless of trust); getProjectSettings gates at access time.
        let global = settings_from_value(&read_settings_file(&agent_dir.join("settings.json")));
        let project =
            settings_from_value(&read_settings_file(&cwd.join(".pi").join("settings.json")));
        Arc::new(FileSettings {
            state: Mutex::new(FileSettingsState {
                global,
                project,
                project_trusted,
            }),
        })
    }
}

impl SettingsManagerHandle for FileSettings {
    fn global_settings(&self) -> SettingsData {
        self.state.lock().unwrap().global.clone()
    }

    fn project_settings(&self) -> SettingsData {
        let state = self.state.lock().unwrap();
        if state.project_trusted {
            state.project.clone()
        } else {
            SettingsData::default()
        }
    }

    fn is_project_trusted(&self) -> bool {
        self.state.lock().unwrap().project_trusted
    }

    fn set_project_trusted(&self, trusted: bool) {
        self.state.lock().unwrap().project_trusted = trusted;
    }

    fn npm_command(&self) -> Option<Vec<String>> {
        self.state.lock().unwrap().global.npm_command.clone()
    }

    fn set_packages(&self, packages: Vec<PackageSourceEntry>) {
        self.state.lock().unwrap().global.packages = packages;
    }

    fn set_project_packages(&self, packages: Vec<PackageSourceEntry>) {
        self.state.lock().unwrap().project.packages = packages;
    }
}

// ===========================================================================
// Test host
// ===========================================================================

const VERSION: &str = "0.85.1";

struct TestHost {
    runner: Arc<FakeRunner>,
    cwd: PathBuf,
    agent_dir: PathBuf,
    package_dir: PathBuf,
    env: Mutex<Map<String, Value>>,
    latest_release: Mutex<Result<Option<LatestPiRelease>, String>>,
    version_command_output: Mutex<Result<String, String>>,
    install_method: &'static str,
}

impl TestHost {
    fn new(dir: &Path) -> TestHost {
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        TestHost {
            runner: FakeRunner::new(),
            cwd: dir.to_path_buf(),
            agent_dir,
            package_dir: dir.to_path_buf(),
            env: Mutex::new(Map::new()),
            latest_release: Mutex::new(Ok(None)),
            version_command_output: Mutex::new(Ok(String::new())),
            install_method: "unknown",
        }
    }

    fn set_env(&self, key: &str, value: &str) {
        self.env
            .lock()
            .unwrap()
            .insert(key.to_string(), json!(value));
    }

    fn set_latest_release(&self, release: LatestPiRelease) {
        *self.latest_release.lock().unwrap() = Ok(Some(release));
    }
}

impl PackageCommandHost for TestHost {
    fn cwd(&self) -> String {
        self.cwd.to_string_lossy().into_owned()
    }

    fn agent_dir(&self) -> String {
        self.agent_dir.to_string_lossy().into_owned()
    }

    fn package_dir(&self) -> String {
        if let Some(package_dir) = self.env_var("PI_PACKAGE_DIR") {
            return package_dir;
        }
        self.package_dir.to_string_lossy().into_owned()
    }

    fn version(&self) -> String {
        VERSION.to_string()
    }

    fn env_var(&self, name: &str) -> Option<String> {
        self.env
            .lock()
            .unwrap()
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty())
    }

    fn command_runner(&self) -> Arc<dyn super::CommandRunner> {
        self.runner.clone()
    }

    fn create_settings_manager(
        &self,
        cwd: &str,
        agent_dir: &str,
        project_trusted: bool,
    ) -> Arc<dyn super::SettingsManagerHandle> {
        FileSettings::create(Path::new(cwd), Path::new(agent_dir), project_trusted)
    }

    fn saved_project_trusted(&self, _cwd: &str) -> bool {
        false
    }

    fn resolve_project_trusted(
        &self,
        _cwd: &str,
        trust_override: Option<bool>,
        _default_project_trust: Option<String>,
    ) -> bool {
        trust_override.unwrap_or(false)
    }

    fn drain_settings_errors(
        &self,
        _settings: &Arc<dyn SettingsManagerHandle>,
    ) -> Vec<(String, String)> {
        Vec::new()
    }

    fn default_project_trust(&self, _settings: &Arc<dyn SettingsManagerHandle>) -> Option<String> {
        None
    }

    fn get_latest_pi_release(&self) -> Result<Option<LatestPiRelease>, String> {
        self.latest_release.lock().unwrap().clone()
    }

    fn refresh_model_catalogs(&self, _agent_dir: &str) -> Result<(), String> {
        Ok(())
    }

    fn detect_install_method(&self) -> &'static str {
        self.install_method
    }

    fn self_update_command(
        &self,
        _npm_command: Option<&[String]>,
        _target: &SelfUpdatePackageTarget,
    ) -> Option<SelfUpdateCommand> {
        None
    }

    fn self_update_unavailable_instruction(
        &self,
        _npm_command: Option<&[String]>,
        target: &SelfUpdatePackageTarget,
    ) -> String {
        format!(
            "Update {} using the package manager, wrapper, or source checkout that provides this installation.",
            target.install_spec
        )
    }

    fn run_self_update(&self, _command: &SelfUpdateCommand) -> Result<(), String> {
        Ok(())
    }

    fn render_markdown_note(&self, note: &str, _width: usize) -> Option<Vec<String>> {
        Some(note.split('\n').map(str::to_string).collect())
    }

    fn fetch_installer_artifact(&self, _url: &str) -> Result<String, String> {
        Ok("{}".to_string())
    }

    fn run_managed_npm_ci(&self, _stage_dir: &str, _args: &[String]) -> Result<(), Option<u32>> {
        Ok(())
    }

    fn run_version_command(&self, _bin_path: &str) -> Result<String, String> {
        self.version_command_output.lock().unwrap().clone()
    }
}

// ===========================================================================
// Oracle replay
// ===========================================================================

const CLI_ORACLE: &str = include_str!("../../../tests/fixtures/pm_oracle/cli.oracle.json");

fn cli_oracle() -> Value {
    serde_json::from_str(CLI_ORACLE).unwrap()
}

fn outcome_value(outcome: &CommandOutcome) -> Value {
    json!({
        "handled": outcome.handled,
        "exitCode": outcome.exit_code,
        "stdout": outcome.stdout,
        "stderr": outcome.stderr,
    })
}

/// Mask the temp root and the harness script path, then normalize
/// separators on both sides (matches the oracle capture normalization).
fn normalize_cli(value: &mut Value, root: &Path) {
    let root_text = root.to_string_lossy().into_owned();
    match value {
        Value::String(text) => {
            let replaced = if root_text.is_empty() {
                text.clone()
            } else {
                text.replace(&root_text, "$T")
            };
            *text = replaced.replace('\\', "/");
        }
        Value::Array(entries) => {
            for entry in entries {
                normalize_cli(entry, root);
            }
        }
        Value::Object(object) => {
            object.remove("exitCalled");
            for (_, entry) in object.iter_mut() {
                normalize_cli(entry, root);
            }
        }
        _ => {}
    }
}

fn scenario_root(name: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp
        .path()
        .join(name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-"));
    std::fs::create_dir_all(&dir).unwrap();
    (temp, dir)
}

fn assert_cli_oracle(name: &str, actual: &Value, root: &Path) {
    let mut expected = cli_oracle()
        .get(name)
        .unwrap_or_else(|| panic!("cli oracle entry {name} missing"))
        .clone();
    normalize_cli(&mut expected, root);
    let mut actual = actual.clone();
    normalize_cli(&mut actual, root);
    assert_eq!(&actual, &expected, "cli scenario {name} diverged");
}

#[test]
fn cli_oracle_help_battery() {
    let _guard = env_lock();
    for command in ["install", "remove", "update", "list"] {
        let (temp, dir) = scenario_root(&format!("help-{command}"));
        let host = TestHost::new(&dir);
        let args = vec![command.to_string(), "--help".to_string()];
        let outcome = cli::handle_package_command(&host, &args).unwrap();
        assert_cli_oracle(
            &format!("help/{command}"),
            &outcome_value(&outcome),
            temp.path(),
        );
        // The `-h` capture is byte-identical to the `--help` capture.
        assert_cli_oracle(
            &format!("help/{command}-h"),
            &outcome_value(&outcome),
            temp.path(),
        );
    }
}

#[test]
fn cli_oracle_config_help() {
    let _guard = env_lock();
    let (temp, dir) = scenario_root("help-config");
    let host = TestHost::new(&dir);
    let outcome =
        cli::handle_config_command(&host, &["config".to_string(), "--help".to_string()]).unwrap();
    assert_cli_oracle("help/config", &outcome_value(&outcome), temp.path());
    let outcome =
        cli::handle_config_command(&host, &["config".to_string(), "-h".to_string()]).unwrap();
    assert_cli_oracle("help/config-h", &outcome_value(&outcome), temp.path());
}

#[test]
fn cli_oracle_error_battery() {
    let _guard = env_lock();
    let cases: Vec<Vec<&str>> = [
        vec!["install"],
        vec!["install", "--bogus"],
        vec!["install", "-x"],
        vec!["install", "--extension"],
        vec!["install", "a", "b"],
        vec!["remove"],
        vec!["remove", "--all"],
        vec!["remove", "--self"],
        vec!["remove", "--models"],
        vec!["remove", "npm:x", "npm:y"],
        vec!["remove", "--extension"],
        vec!["remove", "--extension", "value"],
        vec!["list", "--force"],
        vec!["list", "extra"],
        vec!["update", "-l"],
        vec!["update", "--local"],
        vec!["update", "--all", "--self"],
        vec!["update", "--all", "--extensions"],
        vec!["update", "--all", "--models"],
        vec!["update", "--all", "--extension", "x"],
        vec!["update", "--all", "positional"],
        vec!["update", "--models", "--self"],
        vec!["update", "--models", "--extensions"],
        vec!["update", "--models", "--all"],
        vec!["update", "--models", "--extension", "x"],
        vec!["update", "--models", "positional"],
        vec!["update", "--extension"],
        vec!["update", "--extension", "-x"],
        vec!["update", "--extension", "a", "--extension", "b"],
        vec!["update", "--extension", "a", "--self"],
        vec!["update", "--extension", "a", "--extensions"],
        vec!["update", "--extension", "a", "--all"],
        vec!["update", "--extension", "a", "positional"],
        vec!["update", "src", "--self"],
        vec!["update", "src", "--extensions"],
        vec!["update", "src", "--all"],
        vec!["uninstall"],
    ]
    .iter()
    .map(Vec::clone)
    .collect();

    for case in &cases {
        let name = format!("errors/{}", case.join(" "));
        let (temp, dir) = scenario_root("error-case");
        let host = TestHost::new(&dir);
        let args: Vec<String> = case.iter().map(|part| part.to_string()).collect();
        let outcome = cli::handle_package_command(&host, &args).unwrap();
        assert_cli_oracle(&name, &outcome_value(&outcome), temp.path());
    }
}

#[test]
fn cli_oracle_config_error_battery() {
    let _guard = env_lock();

    for (name, case) in [
        ("errors/config-unknown-option", vec!["config", "--bogus"]),
        ("errors/config-unknown-short", vec!["config", "-z"]),
        (
            "errors/config-unexpected-argument",
            vec!["config", "positional"],
        ),
    ] {
        let (temp, dir) = scenario_root("config-error");
        let host = TestHost::new(&dir);
        let args: Vec<String> = case.iter().map(|part| part.to_string()).collect();
        let outcome = cli::handle_config_command(&host, &args).unwrap();
        assert_cli_oracle(name, &outcome_value(&outcome), temp.path());
    }
}

#[test]
fn cli_oracle_unhandled_commands() {
    let _guard = env_lock();

    let (temp, dir) = scenario_root("unhandled");
    let host = TestHost::new(&dir);
    assert!(cli::handle_package_command(&host, &["bogus".to_string()]).is_none());
    assert!(cli::handle_package_command(&host, &[]).is_none());
    // Pin the oracle's `unhandled/*` entries: handled=false, no output.
    for name in ["unhandled/bogus-command", "unhandled/empty"] {
        let mut expected = cli_oracle().get(name).unwrap().clone();
        normalize_cli(&mut expected, temp.path());
        assert_eq!(expected["handled"], json!(false));
        assert_eq!(expected["stdout"], json!([]));
        assert_eq!(expected["stderr"], json!([]));
    }
}

#[test]
fn cli_oracle_flow_list_empty() {
    let _guard = env_lock();
    let (temp, dir) = scenario_root("flow-list-empty");
    let host = TestHost::new(&dir);
    let outcome = cli::handle_package_command(&host, &["list".to_string()]).unwrap();
    assert_cli_oracle("flow/list-empty", &outcome_value(&outcome), temp.path());
}

#[test]
fn cli_oracle_flow_list_packages() {
    let _guard = env_lock();
    let (temp, dir) = scenario_root("flow-list-packages");
    std::fs::create_dir_all(dir.join(".pi")).unwrap();
    std::fs::create_dir_all(dir.join("agent")).unwrap();
    std::fs::write(
        dir.join("agent").join("settings.json"),
        r#"{"packages":["npm:user-pkg",{"source":"npm:filtered-pkg","extensions":[]},"git:github.com/user/repo","./missing-local"]}"#,
    )
    .unwrap();
    let user_pkg = dir.join("agent/npm/node_modules/user-pkg");
    std::fs::create_dir_all(&user_pkg).unwrap();
    std::fs::write(
        user_pkg.join("package.json"),
        r#"{"name":"user-pkg","version":"1.0.0"}"#,
    )
    .unwrap();
    let host = TestHost::new(&dir);
    let legacy_root = dir.join("npm-legacy-root");
    host.runner.script(Arc::new(
        move |_command: &str, args: &[String], _cwd: Option<&str>| {
            if args[0] == "root" {
                return Ok(SpawnOutcome {
                    stdout: format!("{}\n", legacy_root.to_string_lossy()),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "list" {
                return Ok(SpawnOutcome {
                    stdout: "[]".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Err(format!("unexpected spawn {}", args.join(" ")))
        },
    ));
    let outcome =
        cli::handle_package_command(&host, &["list".to_string(), "--approve".to_string()]).unwrap();
    assert_cli_oracle("flow/list-packages", &outcome_value(&outcome), temp.path());
}

#[test]
fn cli_oracle_flow_trust_gates_and_suggestions() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();

    for (name, args) in [
        (
            "flow/install-local-untrusted",
            vec!["install", "./pkg", "-l"],
        ),
        ("flow/remove-local-untrusted", vec!["remove", "./pkg", "-l"]),
    ] {
        let (_, dir) = scenario_root("trust-gate");
        let host = TestHost::new(&dir);
        let args: Vec<String> = args.iter().map(|part| part.to_string()).collect();
        let outcome = cli::handle_package_command(&host, &args).unwrap();
        assert_cli_oracle(name, &outcome_value(&outcome), temp.path());
    }

    {
        let (_, dir) = scenario_root("flow-remove-unknown-package");
        let host = TestHost::new(&dir);
        let outcome = cli::handle_package_command(
            &host,
            &["remove".to_string(), "npm:not-installed".to_string()],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/remove-unknown-package",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    {
        let (_, dir) = scenario_root("flow-update-suggestion");
        let host = TestHost::new(&dir);
        let outcome = cli::handle_package_command(
            &host,
            &[
                "update".to_string(),
                "example".to_string(),
                "--approve".to_string(),
            ],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/update-suggestion-npm",
            &outcome_value(&outcome),
            temp.path(),
        );

        let host = TestHost::new(&dir);
        let outcome = cli::handle_package_command(
            &host,
            &[
                "update".to_string(),
                "github.com/example/repo".to_string(),
                "--approve".to_string(),
            ],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/update-suggestion-git",
            &outcome_value(&outcome),
            temp.path(),
        );
    }
}

#[test]
fn cli_oracle_flow_update_models_and_self() {
    let _guard = env_lock();

    {
        let (temp, dir) = scenario_root("flow-update-models");
        let host = TestHost::new(&dir);
        let outcome =
            cli::handle_package_command(&host, &["update".to_string(), "--models".to_string()])
                .unwrap();
        assert_cli_oracle("flow/update-models", &outcome_value(&outcome), temp.path());
    }

    for (name, release_version) in [
        ("flow/update-self-uptodate", "0.85.1"),
        ("flow/update-self-uptodate-explicit", "0.85.1"),
        ("flow/update-self-older-release", "0.1.0"),
        ("flow/update-self-newer-unmanaged", "0.86.0"),
        ("flow/update-self-newer-note", "0.86.0"),
    ] {
        // win32-only capture: upstream gates the unmanaged-self-update
        // rejection on `process.platform === "win32"` (upstream
        // package-manager-cli.ts: "pi self-update on Windows is only
        // supported for npm and pnpm installs."). On POSIX the same scenario
        // falls through to the platform-neutral `printSelfUpdateUnavailable`
        // path (both scenarios pin the win32 branch — the note scenario only
        // reaches `printSelfUpdateNote` after a self-update command exists),
        // so the oracle reply is unobservable off-Windows.
        if !cfg!(windows)
            && matches!(
                name,
                "flow/update-self-newer-unmanaged" | "flow/update-self-newer-note"
            )
        {
            continue;
        }
        let (temp, dir) = scenario_root(name.replace('/', "-").as_str());
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: release_version.to_string(),
            package_name: None,
            note: if name == "flow/update-self-newer-note" {
                Some("Release notes heading\n- bullet one".to_string())
            } else {
                None
            },
        });
        let args: Vec<String> = match name {
            "flow/update-self-uptodate" => vec!["update".to_string()],
            "flow/update-self-uptodate-explicit" => vec!["update".to_string(), "pi".to_string()],
            _ => vec!["update".to_string(), "--self".to_string()],
        };
        let outcome = cli::handle_package_command(&host, &args).unwrap();
        assert_cli_oracle(name, &outcome_value(&outcome), temp.path());
    }
}

fn prepare_managed_install(host: &TestHost, version: &str) -> PathBuf {
    let install_root = host.cwd.join("install");
    let release_dir = install_root
        .join("releases")
        .join(version)
        .join("node_modules")
        .join("@earendil-works")
        .join("pi-coding-agent");
    std::fs::create_dir_all(&release_dir).unwrap();
    std::fs::write(
        install_root.join("managed-install.json"),
        r#"{"kind":"pi-managed-install","schemaVersion":1,"layout":"releases-v1"}"#,
    )
    .unwrap();
    host.set_env("PI_MANAGED_INSTALL_ROOT", &install_root.to_string_lossy());
    host.set_env("PI_PACKAGE_DIR", &release_dir.to_string_lossy());
    install_root
}

#[test]
fn cli_oracle_flow_managed_install() {
    let _guard = env_lock();

    {
        let (temp, dir) = scenario_root("flow-update-managed-marker-missing");
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: "0.86.0".to_string(),
            package_name: None,
            note: None,
        });
        prepare_managed_install(&host, "0.85.1");
        // Remove the marker: the release dir exists but the marker is gone.
        std::fs::remove_file(dir.join("install").join("managed-install.json")).unwrap();
        let outcome =
            cli::handle_package_command(&host, &["update".to_string(), "--self".to_string()])
                .unwrap();
        assert_cli_oracle(
            "flow/update-managed-marker-missing",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    {
        let (temp, dir) = scenario_root("flow-update-managed-force-rejected");
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: "0.86.0".to_string(),
            package_name: None,
            note: None,
        });
        prepare_managed_install(&host, "0.85.1");
        let outcome = cli::handle_package_command(
            &host,
            &[
                "update".to_string(),
                "--self".to_string(),
                "--force".to_string(),
            ],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/update-managed-force-rejected",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    {
        let (temp, dir) = scenario_root("flow-update-managed-uptodate");
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: "0.85.1".to_string(),
            package_name: None,
            note: None,
        });
        prepare_managed_install(&host, "0.85.1");
        let outcome = cli::handle_package_command(&host, &["update".to_string()]).unwrap();
        assert_cli_oracle(
            "flow/update-managed-uptodate",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    for name in [
        "flow/update-managed-already-installed",
        "flow/update-managed-activate",
    ] {
        let (temp, dir) = scenario_root(name.replace('/', "-").as_str());
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: "0.86.0".to_string(),
            package_name: None,
            note: None,
        });
        let install_root = prepare_managed_install(&host, "0.86.0");
        std::fs::write(
            install_root
                .join("releases")
                .join("0.86.0")
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("package.json"),
            r#"{"name":"@earendil-works/pi-coding-agent","version":"0.86.0"}"#,
        )
        .unwrap();
        // Smoke test returns the empty version (oracle stub default).
        let outcome = cli::handle_package_command(&host, &["update".to_string()]).unwrap();
        assert_cli_oracle(name, &outcome_value(&outcome), temp.path());
    }

    {
        let (temp, dir) = scenario_root("flow-update-managed-smoke-ok");
        let host = TestHost::new(&dir);
        host.set_latest_release(LatestPiRelease {
            version: "0.86.0".to_string(),
            package_name: None,
            note: None,
        });
        let install_root = prepare_managed_install(&host, "0.86.0");
        std::fs::write(
            install_root
                .join("releases")
                .join("0.86.0")
                .join("node_modules")
                .join("@earendil-works")
                .join("pi-coding-agent")
                .join("package.json"),
            r#"{"name":"@earendil-works/pi-coding-agent","version":"0.86.0"}"#,
        )
        .unwrap();
        *host.version_command_output.lock().unwrap() = Ok("0.86.0".to_string());
        let outcome = cli::handle_package_command(&host, &["update".to_string()]).unwrap();
        assert_cli_oracle(
            "flow/update-managed-smoke-ok",
            &outcome_value(&outcome),
            temp.path(),
        );
        // The release activation file was written with the version line.
        assert_eq!(
            std::fs::read_to_string(install_root.join("current-version")).unwrap(),
            "0.86.0\n"
        );
    }
}

#[test]
fn cli_oracle_flow_install_failures() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();

    {
        let (temp, dir) = scenario_root("flow-install-spawn-failure");
        let host = TestHost::new(&dir);
        host.runner.script(Arc::new(|_command, _args, _cwd| {
            Err("simulated npm install failure".to_string())
        }));
        let outcome = cli::handle_package_command(
            &host,
            &["install".to_string(), "npm:nonexistent@1.0.0".to_string()],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/install-spawn-failure",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    {
        let (temp, dir) = scenario_root("flow-install-git-clone-failure");
        let host = TestHost::new(&dir);
        host.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                return Err("simulated git clone failure".to_string());
            }
            Ok(SpawnOutcome::default())
        }));
        let outcome = cli::handle_package_command(
            &host,
            &[
                "install".to_string(),
                "git:github.com/nonexistent/repo".to_string(),
            ],
        )
        .unwrap();
        assert_cli_oracle(
            "flow/install-git-clone-failure",
            &outcome_value(&outcome),
            temp.path(),
        );
    }

    {
        let (_, dir) = scenario_root("flow-config-untrusted-nonlocal");
        let host = TestHost::new(&dir);
        let outcome = cli::handle_config_command(&host, &["config".to_string()]).unwrap();
        assert_cli_oracle(
            "flow/config-untrusted-nonlocal",
            &outcome_value(&outcome),
            temp.path(),
        );
    }
}

// ===========================================================================
// Layer 2: structural ports (not oracle-pinned)
// ===========================================================================

#[test]
fn parse_package_command_target_resolution() {
    use super::cli::parse_package_command;
    let args =
        |parts: &[&str]| -> Vec<String> { parts.iter().map(|&part| part.to_string()).collect() };

    let options = parse_package_command(&args(&["update"])).unwrap();
    assert_eq!(
        options.update_target,
        Some(super::cli::UpdateTarget::SelfUpdate)
    );
    assert!(options.show_extensions_skipped_note);

    let options = parse_package_command(&args(&["update", "--extensions"])).unwrap();
    assert_eq!(
        options.update_target,
        Some(super::cli::UpdateTarget::Extensions { source: None })
    );
    assert!(!options.show_extensions_skipped_note);

    let options = parse_package_command(&args(&["update", "self"])).unwrap();
    assert_eq!(
        options.update_target,
        Some(super::cli::UpdateTarget::SelfUpdate)
    );

    let options = parse_package_command(&args(&["update", "pi", "--extensions"])).unwrap();
    assert_eq!(options.update_target, Some(super::cli::UpdateTarget::All));

    let options = parse_package_command(&args(&["update", "--self", "--extensions"])).unwrap();
    assert_eq!(options.update_target, Some(super::cli::UpdateTarget::All));

    let options = parse_package_command(&args(&["update", "--extension", "npm:x"])).unwrap();
    assert_eq!(
        options.update_target,
        Some(super::cli::UpdateTarget::Extensions {
            source: Some("npm:x".to_string())
        })
    );

    let options = parse_package_command(&args(&["update", "npm:x"])).unwrap();
    assert_eq!(
        options.update_target,
        Some(super::cli::UpdateTarget::Extensions {
            source: Some("npm:x".to_string())
        })
    );

    let options = parse_package_command(&args(&["uninstall", "npm:x"])).unwrap();
    assert_eq!(options.command, PackageCommand::Remove);
    assert_eq!(options.source.as_deref(), Some("npm:x"));

    let options = parse_package_command(&args(&["install", "npm:x", "-a", "-na"]));
    assert_eq!(options.unwrap().project_trust_override, Some(false));

    assert!(parse_package_command(&args(&["bogus"])).is_none());
    assert!(parse_package_command(&args(&[])).is_none());
}

#[test]
fn help_texts_are_stable() {
    for command in [
        PackageCommand::Install,
        PackageCommand::Remove,
        PackageCommand::Update,
        PackageCommand::List,
    ] {
        let rendered = cli::render_package_command_help(command);
        assert!(rendered.starts_with("Usage:\n  pi "));
        assert!(rendered.ends_with('\n'));
    }
    assert!(cli::render_config_command_help().starts_with("Usage:\n  pi config"));
}

#[test]
fn managed_release_version_validation() {
    // MANAGED_RELEASE_VERSION_RE: /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/
    assert!(super::cli::cli_is_managed_release_version("0.86.0"));
    assert!(super::cli::cli_is_managed_release_version("1.2.3-rc.2"));
    assert!(super::cli::cli_is_managed_release_version(
        "1.2.3-rc.2+meta"
    ));
    assert!(super::cli::cli_is_managed_release_version("1.2.3+meta"));
    assert!(!super::cli::cli_is_managed_release_version("v1.2.3"));
    assert!(!super::cli::cli_is_managed_release_version("1.2"));
    assert!(!super::cli::cli_is_managed_release_version(""));
    assert!(!super::cli::cli_is_managed_release_version("1.2.3-"));
    assert!(!super::cli::cli_is_managed_release_version("1.2.3+"));
}

#[test]
fn run_managed_self_update_rejects_invalid_version() {
    let _guard = env_lock();
    let (_, dir) = scenario_root("managed-invalid-version");
    let host = TestHost::new(&dir);
    let error =
        cli::run_managed_self_update_for_test(&host, &dir.to_string_lossy(), "not-a-version")
            .unwrap_err();
    assert_eq!(
        error.message,
        "Invalid managed release version: not-a-version"
    );
}

#[test]
fn cleanup_managed_install_is_silent_without_root() {
    let _guard = env_lock();
    let (_, dir) = scenario_root("cleanup-no-root");
    let host = TestHost::new(&dir);
    cli::cleanup_managed_install(&host);
}

#[test]
fn self_update_note_falls_back_to_plain_text() {
    let _guard = env_lock();
    let (_, dir) = scenario_root("note-fallback");
    let host = TestHost::new(&dir);
    let mut stdout = Vec::new();
    cli::print_self_update_note_for_test(&host, &mut stdout, "  note line  \n");
    assert_eq!(
        stdout,
        vec![
            String::new(),
            "Update note".to_string(),
            "note line".to_string(),
            String::new()
        ]
    );
    // Empty notes are dropped entirely.
    let mut stdout = Vec::new();
    cli::print_self_update_note_for_test(&host, &mut stdout, "   \n");
    assert!(stdout.is_empty());
}

#[test]
fn update_lock_releases_on_drop() {
    let released = Arc::new(Mutex::new(false));
    let released_sink = Arc::clone(&released);
    let lock = UpdateLock::new(Box::new(move || {
        *released_sink.lock().unwrap() = true;
    }));
    drop(lock);
    assert!(*released.lock().unwrap());
}

#[test]
fn list_command_renders_sections_like_upstream() {
    let _guard = env_lock();
    let (_, dir) = scenario_root("list-sections");
    std::fs::create_dir_all(dir.join("agent")).unwrap();
    std::fs::write(
        dir.join("agent").join("settings.json"),
        r#"{"packages":["npm:user-a"]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.join(".pi")).unwrap();
    std::fs::write(
        dir.join(".pi").join("settings.json"),
        r#"{"packages":["npm:project-b"]}"#,
    )
    .unwrap();
    let user_pkg = dir.join("agent/npm/node_modules/user-a");
    std::fs::create_dir_all(&user_pkg).unwrap();
    std::fs::write(
        user_pkg.join("package.json"),
        r#"{"name":"user-a","version":"1.0.0"}"#,
    )
    .unwrap();
    let project_pkg = dir
        .join(".pi")
        .join("npm")
        .join("node_modules")
        .join("project-b");
    std::fs::create_dir_all(&project_pkg).unwrap();
    std::fs::write(
        project_pkg.join("package.json"),
        r#"{"name":"project-b","version":"1.0.0"}"#,
    )
    .unwrap();
    let host = TestHost::new(&dir);
    let outcome =
        cli::handle_package_command(&host, &["list".to_string(), "--approve".to_string()]).unwrap();
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(
        outcome.stdout.join("\n").split('\n').next(),
        Some("User packages:")
    );
    let joined = outcome.stdout.join("\n");
    assert!(joined.contains("Project packages:"));
    assert!(joined.contains(&format!("    {}", project_pkg.to_string_lossy())));
}

#[test]
fn self_update_plan_reports_up_to_date() {
    let _guard = env_lock();
    let (_, dir) = scenario_root("plan-uptodate");
    let host = TestHost::new(&dir);
    host.set_latest_release(LatestPiRelease {
        version: VERSION.to_string(),
        package_name: None,
        note: None,
    });
    let mut stdout = Vec::new();
    let plan = cli::get_self_update_plan_for_test(&host, &mut stdout, false).unwrap();
    assert!(!plan.should_run);
    assert_eq!(
        stdout,
        vec![format!("pi is already up to date (v{VERSION})")]
    );

    host.set_latest_release(LatestPiRelease {
        version: "0.86.0".to_string(),
        package_name: None,
        note: None,
    });
    let mut stdout = Vec::new();
    let plan = cli::get_self_update_plan_for_test(&host, &mut stdout, false).unwrap();
    assert!(plan.should_run);
    assert_eq!(plan.install_spec, format!("{PACKAGE_NAME}@0.86.0"));
    assert!(stdout.is_empty());

    // Force reinstalls even at the same version.
    host.set_latest_release(LatestPiRelease {
        version: VERSION.to_string(),
        package_name: None,
        note: None,
    });
    let mut stdout = Vec::new();
    let plan = cli::get_self_update_plan_for_test(&host, &mut stdout, true).unwrap();
    assert!(plan.should_run);
    assert!(stdout.is_empty());
}

#[test]
fn update_models_reports_success_and_failure() {
    let _guard = env_lock();
    struct FailingHost(TestHost);
    impl PackageCommandHost for FailingHost {
        fn cwd(&self) -> String {
            self.0.cwd()
        }
        fn agent_dir(&self) -> String {
            self.0.agent_dir()
        }
        fn package_dir(&self) -> String {
            self.0.package_dir()
        }
        fn version(&self) -> String {
            self.0.version()
        }
        fn env_var(&self, name: &str) -> Option<String> {
            self.0.env_var(name)
        }
        fn command_runner(&self) -> Arc<dyn super::CommandRunner> {
            self.0.command_runner()
        }
        fn create_settings_manager(
            &self,
            cwd: &str,
            agent_dir: &str,
            project_trusted: bool,
        ) -> Arc<dyn SettingsManagerHandle> {
            self.0
                .create_settings_manager(cwd, agent_dir, project_trusted)
        }
        fn saved_project_trusted(&self, cwd: &str) -> bool {
            self.0.saved_project_trusted(cwd)
        }
        fn resolve_project_trusted(
            &self,
            cwd: &str,
            trust_override: Option<bool>,
            default_project_trust: Option<String>,
        ) -> bool {
            self.0
                .resolve_project_trusted(cwd, trust_override, default_project_trust)
        }
        fn drain_settings_errors(
            &self,
            settings: &Arc<dyn SettingsManagerHandle>,
        ) -> Vec<(String, String)> {
            self.0.drain_settings_errors(settings)
        }
        fn default_project_trust(
            &self,
            settings: &Arc<dyn SettingsManagerHandle>,
        ) -> Option<String> {
            self.0.default_project_trust(settings)
        }
        fn get_latest_pi_release(&self) -> Result<Option<LatestPiRelease>, String> {
            self.0.get_latest_pi_release()
        }
        fn refresh_model_catalogs(&self, _agent_dir: &str) -> Result<(), String> {
            Err("catalogs unavailable".to_string())
        }
        fn detect_install_method(&self) -> &'static str {
            self.0.detect_install_method()
        }
        fn self_update_command(
            &self,
            npm_command: Option<&[String]>,
            target: &SelfUpdatePackageTarget,
        ) -> Option<SelfUpdateCommand> {
            self.0.self_update_command(npm_command, target)
        }
        fn self_update_unavailable_instruction(
            &self,
            npm_command: Option<&[String]>,
            target: &SelfUpdatePackageTarget,
        ) -> String {
            self.0
                .self_update_unavailable_instruction(npm_command, target)
        }
        fn run_self_update(&self, command: &SelfUpdateCommand) -> Result<(), String> {
            self.0.run_self_update(command)
        }
        fn fetch_installer_artifact(&self, url: &str) -> Result<String, String> {
            self.0.fetch_installer_artifact(url)
        }
        fn run_managed_npm_ci(&self, stage_dir: &str, args: &[String]) -> Result<(), Option<u32>> {
            self.0.run_managed_npm_ci(stage_dir, args)
        }
        fn run_version_command(&self, bin_path: &str) -> Result<String, String> {
            self.0.run_version_command(bin_path)
        }
    }

    let (_, dir) = scenario_root("models-failure");
    let host = FailingHost(TestHost::new(&dir));
    let outcome =
        cli::handle_package_command(&host, &["update".to_string(), "--models".to_string()])
            .unwrap();
    assert_eq!(outcome.exit_code, 1);
    assert_eq!(outcome.stderr, vec!["Error: catalogs unavailable"]);
}

#[test]
fn resolved_paths_default_is_empty() {
    let resolved = ResolvedPaths::default();
    assert!(resolved.extensions.is_empty());
    assert!(resolved.skills.is_empty());
    assert!(resolved.prompts.is_empty());
    assert!(resolved.themes.is_empty());
}

#[test]
fn default_package_manager_threaded_update_smoke() {
    // Direct construction parity: the CLI path builds the manager through
    // PackageManagerOptions with the host's runner.
    let _guard = env_lock();
    let (_, dir) = scenario_root("pm-options-smoke");
    let host = TestHost::new(&dir);
    let settings = host.create_settings_manager(
        &dir.to_string_lossy(),
        &host.agent_dir.to_string_lossy(),
        false,
    );
    let manager = DefaultPackageManager::new(PackageManagerOptions {
        cwd: dir.to_string_lossy().into_owned(),
        agent_dir: host.agent_dir.to_string_lossy().into_owned(),
        settings_manager: settings,
        command_runner: Some(host.command_runner()),
    });
    let resolved = manager.resolve(None).unwrap();
    assert_eq!(resolved, ResolvedPaths::default());
    assert!(manager.check_for_available_updates().unwrap().is_empty());
}
