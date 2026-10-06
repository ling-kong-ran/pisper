//! Tests for the package-manager port (slice M5 W3.9), two layers:
//!
//! 1. **Oracle replay**: every scenario in `tests/fixtures/pm_oracle/core.oracle.json`
//!    — captured by running the byte-identical upstream TypeScript sources
//!    under node (`--experimental-strip-types`) with the pinned npm
//!    `semver`/`minimatch`/`ignore`/`hosted-git-info` versions — is replayed
//!    against an identical temp tree; rendered results (paths relativized to
//!    the scenario dir, temp root masked to `$T`, separators normalized to
//!    `/`) must match byte-for-byte as canonical JSON.
//! 2. **Ported executable spec**: upstream `test/package-manager.test.ts` /
//!    `test/package-manager-ssh.test.ts` cases that are not fs/spawn
//!    scenario-shaped (behavioral assertions) are ported against scripted
//!    [`FakeRunner`] transports, mirroring the upstream suite's spies on
//!    `runCommand`/`runCommandCapture`/`runCommandSync`.
//!
//! Disclosed upstream test blocks (not portable to this layer):
//! - `command spawning: argv entries containing spaces` — the real-process
//!   spawn argv fidelity is `std::process::Command`'s array contract; the
//!   ported test asserts argv recording through the seam plus a real
//!   delayed-stdout capture (see `real_runner_captures_delayed_stdout`).
//! - `should wait for close before resolving captured stdout` — JS stream
//!   ordering; the sync runner reads pipes to EOF (same observable data),
//!   pinned by the real-process capture test.
//! - The `minimatch`/`ignore`/`semver` battery rows double as pins for the
//!   vendored library behavior (see [`vendor`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use super::{
    CommandRunner, DefaultPackageManager, PackageSourceEntry, ProgressEvent, SettingsData,
    SettingsManagerHandle, SourceScope,
};

// ===========================================================================
// Shared fixtures
// ===========================================================================

/// Serializes tests that mutate process env (HOME / PI_OFFLINE), which is
/// process-global.
pub(super) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Upstream `SettingsManager.inMemory()` for the consumed surface.
#[derive(Default)]
struct TestSettingsState {
    global: SettingsData,
    project: SettingsData,
    project_trusted: bool,
}

struct TestSettings {
    state: Mutex<TestSettingsState>,
}

impl TestSettings {
    fn new(settings: Value) -> Arc<Self> {
        let global = settings_data_from_json(&settings);
        Arc::new(TestSettings {
            state: Mutex::new(TestSettingsState {
                global,
                project: SettingsData::default(),
                project_trusted: true,
            }),
        })
    }
}

fn settings_data_from_json(value: &Value) -> SettingsData {
    let packages = value
        .get("packages")
        .and_then(|packages| packages.as_array())
        .map(|packages| {
            packages
                .iter()
                .map(|entry| serde_json::from_value::<PackageSourceEntry>(entry.clone()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    let strings = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(|entries| entries.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    SettingsData {
        packages,
        extensions: strings("extensions"),
        skills: strings("skills"),
        prompts: strings("prompts"),
        themes: strings("themes"),
        npm_command: value
            .get("npmCommand")
            .and_then(|command| command.as_array())
            .map(|command| {
                command
                    .iter()
                    .map(|part| part.as_str().unwrap_or_default().to_string())
                    .collect()
            }),
    }
}

impl SettingsManagerHandle for TestSettings {
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

/// Scripted child-process outcomes (mirrors the oracle child-process stub).
#[derive(Debug, Clone, Default)]
pub(super) struct SpawnOutcome {
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) code: i64,
}

#[derive(Debug, Clone)]
struct CallRecord {
    kind: &'static str,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
}

type ScriptHandlerFn =
    dyn Fn(&str, &[String], Option<&str>) -> Result<SpawnOutcome, String> + Send + Sync;
type ScriptHandler = Arc<ScriptHandlerFn>;

#[derive(Default)]
struct FakeRunnerState {
    log: Vec<CallRecord>,
}

pub(super) struct FakeRunner {
    state: Mutex<FakeRunnerState>,
    handler: Mutex<Option<ScriptHandler>>,
}

impl FakeRunner {
    pub(super) fn new() -> Arc<FakeRunner> {
        Arc::new(FakeRunner {
            state: Mutex::new(FakeRunnerState::default()),
            handler: Mutex::new(None),
        })
    }

    pub(super) fn script(&self, handler: ScriptHandler) {
        *self.handler.lock().unwrap() = Some(handler);
    }

    fn log_len(&self) -> usize {
        self.state.lock().unwrap().log.len()
    }

    /// `(command, args)` call tuples for assertions.
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.state
            .lock()
            .unwrap()
            .log
            .iter()
            .map(|record| (record.command.clone(), record.args.clone()))
            .collect()
    }

    fn log_json(&self) -> Value {
        let state = self.state.lock().unwrap();
        Value::Array(
            state
                .log
                .iter()
                .map(|record| {
                    let mut object = Map::new();
                    object.insert("kind".into(), json!(record.kind));
                    object.insert("command".into(), json!(record.command));
                    object.insert("args".into(), json!(record.args));
                    let mut options = Map::new();
                    if let Some(cwd) = &record.cwd {
                        options.insert("cwd".into(), json!(cwd));
                    }
                    object.insert("options".into(), Value::Object(options));
                    Value::Object(object)
                })
                .collect(),
        )
    }
}

impl FakeRunnerState {
    fn record(&mut self, kind: &'static str, command: &str, args: &[String], cwd: Option<&str>) {
        self.log.push(CallRecord {
            kind,
            command: command.to_string(),
            args: args.to_vec(),
            cwd: cwd.map(str::to_string),
        });
    }
}

fn default_outcome() -> SpawnOutcome {
    SpawnOutcome {
        stdout: String::new(),
        stderr: String::new(),
        code: 0,
    }
}

impl super::CommandRunner for FakeRunner {
    fn run(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Result<(), super::CommandError> {
        self.state
            .lock()
            .unwrap()
            .record("spawn", command, args, cwd);
        // Clone the handler out so concurrent spawns do not serialize on
        // the handler lock while a scripted sleep is running.
        let handler = self.handler.lock().unwrap().clone();
        let outcome = match handler {
            Some(handler) => handler(command, args, cwd),
            None => Ok(default_outcome()),
        };
        match outcome {
            Ok(outcome) => {
                if outcome.code == 0 {
                    Ok(())
                } else {
                    Err(super::CommandError::new(format!(
                        "{} failed with code {}",
                        join_command_args(command, args),
                        outcome.code
                    )))
                }
            }
            Err(message) => Err(super::CommandError::new(message)),
        }
    }

    fn run_capture(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
        _timeout_ms: Option<u64>,
        _extra_env: &[(String, String)],
    ) -> Result<String, super::CommandError> {
        self.state
            .lock()
            .unwrap()
            .record("spawn", command, args, cwd);
        // Clone the handler out so concurrent spawns do not serialize on
        // the handler lock while a scripted sleep is running.
        let handler = self.handler.lock().unwrap().clone();
        let outcome = match handler {
            Some(handler) => handler(command, args, cwd),
            None => Ok(default_outcome()),
        };
        match outcome {
            Ok(outcome) => {
                if outcome.code == 0 {
                    Ok(outcome.stdout.trim().to_string())
                } else {
                    Err(super::CommandError::new(format!(
                        "{} failed with code {}: {}",
                        join_command_args(command, args),
                        outcome.code,
                        if outcome.stderr.is_empty() {
                            outcome.stdout
                        } else {
                            outcome.stderr
                        }
                    )))
                }
            }
            Err(message) => Err(super::CommandError::new(message)),
        }
    }

    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, super::CommandError> {
        self.state
            .lock()
            .unwrap()
            .record("spawnSync", command, args, None);
        let handler = self.handler.lock().unwrap().clone();
        let outcome = match handler {
            Some(handler) => handler(command, args, None),
            None => Ok(default_outcome()),
        };
        match outcome {
            Ok(outcome) => {
                if outcome.code == 0 {
                    Ok(if outcome.stdout.is_empty() {
                        outcome.stderr
                    } else {
                        outcome.stdout
                    }
                    .trim()
                    .to_string())
                } else {
                    Err(super::CommandError::new(format!(
                        "Failed to run {}: {}",
                        join_command_args(command, args),
                        if outcome.stderr.is_empty() {
                            outcome.stdout
                        } else {
                            outcome.stderr
                        }
                    )))
                }
            }
            Err(message) => Err(super::CommandError::new(format!(
                "Failed to run {}: {}",
                join_command_args(command, args),
                message
            ))),
        }
    }
}

fn join_command_args(command: &str, args: &[String]) -> String {
    let mut out = String::from(command);
    for arg in args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

fn pm_from(
    dir: &Path,
    agent_dir: &Path,
    settings: &Arc<TestSettings>,
    runner: &Arc<FakeRunner>,
) -> DefaultPackageManager {
    DefaultPackageManager::new(super::PackageManagerOptions {
        cwd: dir.to_string_lossy().into_owned(),
        agent_dir: agent_dir.to_string_lossy().into_owned(),
        settings_manager: settings.clone(),
        command_runner: Some(runner.clone()),
    })
}

// ===========================================================================
// Oracle plumbing
// ===========================================================================

const ORACLE: &str = include_str!("../../../tests/fixtures/pm_oracle/core.oracle.json");

fn oracle() -> Value {
    serde_json::from_str(ORACLE).unwrap()
}

fn oracle_entry(name: &str) -> Value {
    oracle()
        .get(name)
        .unwrap_or_else(|| panic!("oracle entry {name} missing"))
        .clone()
}

/// environment-anchored: both sides normalized — absolute local sources
/// resolve onto the live drive while the capture stored the capture machine's
/// `C:` form, so `local:D:/...` / `local:/...` identities are compared
/// against `local:C:/...` through a drive placeholder.
fn scrub_identity_drive(text: &str) -> String {
    let out = crate::coding_agent::oracle_scrub::scrub_str(text);
    // `local:/absolute/...` (drive-less resolution on POSIX) shares the
    // capture's `local:<DRV>:/...` anchor. On POSIX, scrub_str's `X:/`
    // drive rewrite consumes the `l:/` inside the `local:/` prefix itself
    // (yielding `loca<DRV>:/`), so recognize that mangled form; on win32
    // the identity already carries the live drive (`local:C:/...`), which
    // scrubs straight to `local:<DRV>:/`.
    if let Some(rest) = out.strip_prefix("local:/") {
        return format!("local:<DRV>:/{rest}");
    }
    if let Some(rest) = out.strip_prefix("loca<DRV>:/") {
        return format!("local:<DRV>:/{rest}");
    }
    out
}

/// Mask the temp root and normalize separators on every string (both sides).
/// Object keys are normalized too: the `flow-update-batch-per-scope` counts
/// map keys embed spawn command lines with the platform's separators (the
/// capture stored win32 `\` forms), and keys would otherwise survive the
/// value-level walk.
fn normalize(value: &mut Value, root: &Path) {
    let root_text = root.to_string_lossy().into_owned();
    let scrub_string = |text: &str| -> String {
        let replaced = if root_text.is_empty() {
            text.to_string()
        } else {
            text.replace(&root_text, "$T")
        };
        let unified = replaced.replace('\\', "/");
        scrub_identity_drive(&unified)
    };
    match value {
        Value::String(text) => {
            *text = scrub_string(text);
        }
        Value::Array(entries) => {
            for entry in entries {
                normalize(entry, root);
            }
        }
        Value::Object(object) => {
            let remaps: Vec<(String, String)> = object
                .keys()
                .map(|key| (key.clone(), scrub_string(key)))
                .collect();
            for (old, new) in remaps {
                if old != new {
                    if let Some(entry) = object.shift_remove(&old) {
                        object.insert(new, entry);
                    }
                }
            }
            for (_, entry) in object.iter_mut() {
                normalize(entry, root);
            }
        }
        _ => {}
    }
}

/// Upstream orders resolved resources by precedence rank only (a stable
/// `Array#sort` in `toResolvedPaths`); within one rank the order is the
/// directory-read order, which the OS defines (NTFS enumerates names sorted,
/// ext4 does not), so the win32 capture cannot pin it and upstream-on-linux
/// would enumerate in the local readdir order too. Sort both sides by
/// (rank, rel): the comparison still pins rank grouping, membership, enabled
/// flags and per-resource metadata, abstracting only the readdir order.
fn canonicalize_resource_order(value: &mut Value) {
    fn resource_rank(entry: &Value) -> (u8, String) {
        let rel = entry
            .get("rel")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let rank = match entry.get("metadata") {
            Some(metadata) => {
                let origin = metadata
                    .get("origin")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if origin == "package" {
                    4
                } else {
                    let scope = metadata
                        .get("scope")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let source = metadata
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    (if scope == "project" { 0 } else { 2 }) + u8::from(source != "local")
                }
            }
            None => 0,
        };
        (rank, rel)
    }

    match value {
        Value::Object(object) => {
            for key in ["extensions", "skills", "prompts", "themes"] {
                if let Some(Value::Array(entries)) = object.get_mut(key) {
                    if entries.iter().all(|entry| entry.get("rel").is_some()) {
                        entries.sort_by_key(resource_rank);
                    }
                }
            }
            for (_, entry) in object.iter_mut() {
                canonicalize_resource_order(entry);
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                canonicalize_resource_order(entry);
            }
        }
        _ => {}
    }
}

fn assert_matches_oracle(name: &str, actual: &Value, root: &Path) {
    let mut expected = oracle_entry(name);
    normalize(&mut expected, root);
    let mut actual = actual.clone();
    normalize(&mut actual, root);
    canonicalize_resource_order(&mut expected);
    canonicalize_resource_order(&mut actual);
    assert_eq!(
        &actual, &expected,
        "scenario {name} diverged from the upstream oracle"
    );
}

/// Render `ResolvedPaths` exactly like the oracle harness `render()`.
fn render_resolved(resolved: &super::ResolvedPaths, dir: &Path) -> Value {
    fn rel_to(dir: &Path, path: &str) -> String {
        path_relative(dir, Path::new(path)).replace('\\', "/")
    }
    fn render_list(entries: &[super::ResolvedResource], dir: &Path) -> Value {
        Value::Array(
            entries
                .iter()
                .map(|entry| {
                    let mut metadata = Map::new();
                    metadata.insert("source".into(), json!(entry.metadata.source));
                    metadata.insert(
                        "scope".into(),
                        json!(match entry.metadata.scope {
                            SourceScope::User => "user",
                            SourceScope::Project => "project",
                            SourceScope::Temporary => "temporary",
                        }),
                    );
                    metadata.insert(
                        "origin".into(),
                        json!(match entry.metadata.origin {
                            super::PathMetadataOrigin::Package => "package",
                            super::PathMetadataOrigin::TopLevel => "top-level",
                        }),
                    );
                    if let Some(base_dir) = &entry.metadata.base_dir {
                        metadata.insert("baseDir".into(), json!(rel_to(dir, base_dir)));
                    }
                    json!({
                        "rel": rel_to(dir, &entry.path),
                        "enabled": entry.enabled,
                        "metadata": Value::Object(metadata),
                    })
                })
                .collect(),
        )
    }
    json!({
        "extensions": render_list(&resolved.extensions, dir),
        "skills": render_list(&resolved.skills, dir),
        "prompts": render_list(&resolved.prompts, dir),
        "themes": render_list(&resolved.themes, dir),
    })
}

/// `path.relative(from, to)` on the host platform (test helper fidelity).
fn path_relative(from: &Path, to: &Path) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_relative(
            &from.to_string_lossy(),
            &to.to_string_lossy(),
            &std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    } else {
        crate::coding_agent::utils::node_path::posix_relative(
            &from.to_string_lossy(),
            &to.to_string_lossy(),
            &std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    }
}

/// Per-scenario tree fixture (mirrors the JS harness `makeCtx`).
struct Scenario {
    dir: PathBuf,
    agent_dir: PathBuf,
    runner: Arc<FakeRunner>,
    settings: Arc<TestSettings>,
}

impl Scenario {
    fn new(root: &Path, name: &str) -> Scenario {
        let dir = root.join(name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-"));
        std::fs::create_dir_all(&dir).unwrap();
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        Scenario {
            dir,
            agent_dir,
            runner: FakeRunner::new(),
            settings: TestSettings::new(json!({})),
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn mkdir(&self, rel: &str) {
        std::fs::create_dir_all(self.dir.join(rel)).unwrap();
    }

    fn link(&self, target_rel: &str, link_rel: &str) {
        let target = self.dir.join(target_rel);
        let link = self.dir.join(link_rel);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&target, &link)
                .or_else(|_| junction_create(&target, &link))
                .unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &link).unwrap();
        }
    }

    fn pm(&self) -> DefaultPackageManager {
        pm_from(&self.dir, &self.agent_dir, &self.settings, &self.runner)
    }

    fn pm_with(&self, cwd: &Path, agent_dir: &Path) -> DefaultPackageManager {
        pm_from(cwd, agent_dir, &self.settings, &self.runner)
    }
}

#[cfg(windows)]
fn junction_create(target: &Path, link: &Path) -> std::io::Result<()> {
    // `mklink /J` equivalent via `fs_util` is unavailable; directory symlinks
    // work for the test user on this host. Surface the original error.
    let _ = target;
    let _ = link;
    Err(std::io::Error::other("junction fallback unavailable"))
}

const JS: &str = "export default function() {}";
fn skill(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name}\n---\nContent")
}
// ===========================================================================
// Library batteries (vendored pins)
// ===========================================================================

#[test]
fn oracle_semver_battery() {
    let battery = oracle().get("lib/semver").unwrap().clone();
    for entry in battery.get("valid").unwrap().as_array().unwrap() {
        let input = entry[0].as_str().unwrap();
        let expected = entry[1].as_str();
        assert_eq!(
            super::vendor::semver_valid(input).as_deref(),
            expected,
            "valid({input:?})"
        );
    }
    for entry in battery.get("validRange").unwrap().as_array().unwrap() {
        let input = entry[0].as_str().unwrap();
        // The capture stores the canonical range string; `null` marks an
        // unparseable range. Only the parse decision is asserted (the port
        // does not render canonical strings — divergence 5).
        let expected = entry[1].is_string();
        assert_eq!(
            super::vendor::parse_range(input).is_some(),
            expected,
            "validRange({input:?}) parse decision"
        );
    }
    for entry in battery.get("satisfies").unwrap().as_array().unwrap() {
        let version = entry[0][0].as_str().unwrap();
        let range_text = entry[0][1].as_str().unwrap();
        let expected = entry[1].as_bool().unwrap();
        let actual = match super::vendor::parse_range(range_text) {
            Some(range) => super::vendor::range_satisfies(version, &range),
            None => false,
        };
        assert_eq!(actual, expected, "satisfies({version:?}, {range_text:?})");
    }
    for entry in battery.get("gt").unwrap().as_array().unwrap() {
        let left = entry[0][0].as_str().unwrap();
        let right = entry[0][1].as_str().unwrap();
        let expected = entry[1].as_bool().unwrap();
        assert_eq!(
            super::vendor::semver_gt(left, right),
            expected,
            "gt({left:?}, {right:?})"
        );
    }
    for entry in battery.get("maxSatisfying").unwrap().as_array().unwrap() {
        let versions: Vec<&str> = entry[0][0]
            .as_str()
            .unwrap()
            .split('|')
            .filter(|version| !version.is_empty())
            .collect();
        let range_text = entry[0][1].as_str().unwrap();
        let expected = entry[1].as_str();
        let range = super::vendor::parse_range(range_text).unwrap();
        assert_eq!(
            super::vendor::max_satisfying(&versions, Some(&range)).as_deref(),
            expected,
            "maxSatisfying({versions:?}, {range_text:?})"
        );
    }
    for entry in battery.get("rcompare").unwrap().as_array().unwrap() {
        let left = entry[0][0].as_str().unwrap();
        let right = entry[0][1].as_str().unwrap();
        let ordering = entry[1].as_i64().unwrap();
        let actual = match super::vendor::semver_rcompare(left, right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        assert_eq!(actual, ordering, "rcompare({left:?}, {right:?})");
    }
    for entry in battery.get("sortDesc").unwrap().as_array().unwrap() {
        let mut versions: Vec<&str> = entry[0][0]
            .as_str()
            .unwrap()
            .split('|')
            .filter(|version| !version.is_empty())
            .collect();
        versions.sort_by(|a, b| super::vendor::semver_rcompare(a, b));
        assert_eq!(
            versions.join("|"),
            entry[1].as_str().unwrap(),
            "sort(rcompare)"
        );
    }
}

#[test]
fn oracle_ignore_battery() {
    let battery = oracle().get("lib/ignore").unwrap().clone();
    let temp = tempfile::tempdir().unwrap();
    for case in battery.as_array().unwrap() {
        let rules: Vec<String> = case
            .get("rules")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule.as_str().unwrap().to_string())
            .collect();
        let paths: Vec<&str> = case
            .get("paths")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        let matcher = super::vendor::IgnoreMatcher::from_rules(
            temp.path().to_string_lossy().as_ref(),
            &rules,
        );
        for (path, expected) in paths
            .iter()
            .zip(case.get("results").unwrap().as_array().unwrap())
        {
            let is_dir = path.ends_with('/');
            assert_eq!(
                matcher.ignores(&temp.path().join(path).to_string_lossy(), is_dir),
                expected.as_bool().unwrap(),
                "rules {rules:?} ignores {path:?}"
            );
        }
    }
}

#[test]
fn oracle_minimatch_battery() {
    let battery = oracle().get("lib/minimatch").unwrap().clone();
    for case in battery.as_array().unwrap() {
        let text = case[0].as_str().unwrap();
        let pattern = case[1].as_str().unwrap();
        let expected = case[2].as_bool().unwrap();
        assert_eq!(
            super::vendor::minimatch(text, pattern),
            expected,
            "minimatch({text:?}, {pattern:?})"
        );
    }
}

// ===========================================================================
// pure/sourceParsing
// ===========================================================================

fn parsed_source_json(parsed: &super::ParsedSource) -> Value {
    match parsed {
        super::ParsedSource::Npm(npm) => {
            // `version: undefined` and `range` are dropped on both sides
            // (JSON.stringify drops undefined; divergence 5 covers range).
            let mut object = Map::new();
            object.insert("type".into(), json!("npm"));
            object.insert("spec".into(), json!(npm.spec));
            object.insert("name".into(), json!(npm.name));
            if let Some(version) = &npm.version {
                object.insert("version".into(), json!(version));
            }
            object.insert("pinned".into(), json!(npm.pinned));
            Value::Object(object)
        }
        super::ParsedSource::Git(git) => {
            let mut object = Map::new();
            object.insert("type".into(), json!("git"));
            object.insert("repo".into(), json!(git.repo));
            object.insert("host".into(), json!(git.host));
            object.insert("path".into(), json!(git.path));
            if let Some(ref_) = &git.ref_ {
                object.insert("ref".into(), json!(ref_));
            }
            object.insert("pinned".into(), json!(git.pinned));
            Value::Object(object)
        }
        super::ParsedSource::Local(local) => json!({
            "type": "local",
            "path": local.path,
        }),
    }
}

#[test]
fn oracle_source_parsing_battery() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("pure-parsing");
    let agent_dir = base.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(&base, &agent_dir, &settings, &FakeRunner::new());

    let mut actual = Map::new();
    let mut parse_source = Vec::new();
    let mut identity = Vec::new();
    for entry in oracle()
        .get("pure/sourceParsing")
        .unwrap()
        .get("parseSource")
        .unwrap()
        .as_array()
        .unwrap()
    {
        let source = entry.get("source").unwrap().as_str().unwrap();
        parse_source.push(json!({
            "source": source,
            "parsed": strip_range(parsed_source_json(&manager.parse_source(source))),
        }));
    }
    actual.insert("parseSource".into(), Value::Array(parse_source));

    for pair in oracle()
        .get("pure/sourceParsing")
        .unwrap()
        .get("identity")
        .unwrap()
        .as_array()
        .unwrap()
    {
        let source = pair[0].as_str().unwrap();
        identity.push(json!([source, manager.get_package_identity(source, None)]));
    }
    actual.insert("identity".into(), Value::Array(identity));

    let mut identity_with_scope = Vec::new();
    for pair in oracle()
        .get("pure/sourceParsing")
        .unwrap()
        .get("identityWithScope")
        .unwrap()
        .as_array()
        .unwrap()
    {
        let source = pair[0].as_str().unwrap();
        let scope = pair[1].as_str().unwrap();
        let scope_value = match scope {
            "user" => SourceScope::User,
            _ => SourceScope::Project,
        };
        identity_with_scope.push(json!([
            source,
            scope,
            manager.get_package_identity(source, Some(scope_value)),
        ]));
    }
    actual.insert(
        "identityWithScope".into(),
        Value::Array(identity_with_scope),
    );

    let mut expected = oracle_entry("pure/sourceParsing");
    strip_range_recursive(&mut expected);
    normalize(&mut expected, temp.path());
    let mut actual = Value::Object(actual);
    normalize(&mut actual, temp.path());
    assert_eq!(actual, expected);
}

fn strip_range(value: Value) -> Value {
    if let Value::Object(mut object) = value {
        object.remove("range");
        Value::Object(object)
    } else {
        value
    }
}

fn strip_range_recursive(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("range");
            for (_, entry) in object.iter_mut() {
                strip_range_recursive(entry);
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                strip_range_recursive(entry);
            }
        }
        _ => {}
    }
}

// ===========================================================================
// resolve scenarios (fs trees)
// ===========================================================================

macro_rules! oracle_resolve_scenario {
    ($fn_name:ident, $oracle_name:literal, $body:expr) => {
        #[test]
        fn $fn_name() {
            let body: fn(&Scenario) -> Value = $body;
            let _guard = env_lock();
            let temp = tempfile::tempdir().unwrap();
            let scenario = Scenario::new(temp.path(), $oracle_name);
            let previous_home = std::env::var("HOME").ok();
            std::env::set_var("HOME", &scenario.dir);
            let actual = body(&scenario);
            match previous_home {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
            assert_matches_oracle($oracle_name, &actual, temp.path());
        }
    };
}

oracle_resolve_scenario!(oracle_resolve_empty, "resolve-empty", |scenario| {
    render_resolved(&scenario.pm().resolve(None).unwrap(), &scenario.dir)
});

oracle_resolve_scenario!(
    oracle_resolve_local_extension_paths,
    "resolve-local-extension-paths",
    |s| {
        s.write("agent/extensions/my-extension.ts", JS);
        s.settings.state.lock().unwrap().global.extensions =
            vec!["extensions/my-extension.ts".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_resolve_skill_paths, "resolve-skill-paths", |s| {
    s.write("agent/skills/my-skill/SKILL.md", &skill("test-skill"));
    s.settings.state.lock().unwrap().global.skills = vec!["skills".to_string()];
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_resolve_root_markdown_skill,
    "resolve-root-markdown-skill",
    |s| {
        s.write("agent/skills/single-file.md", &skill("single-file"));
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_project_paths_relative_pi,
    "resolve-project-paths-relative-pi",
    |s| {
        s.write(".pi/extensions/project-ext.ts", JS);
        s.settings.state.lock().unwrap().project.extensions =
            vec!["extensions/project-ext.ts".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_user_prompt_overrides,
    "resolve-user-prompt-overrides",
    |s| {
        s.write("agent/prompts/auto.md", "Auto prompt");
        s.settings.state.lock().unwrap().global.prompts = vec!["!prompts/auto.md".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_symlinked_resources_once,
    "resolve-symlinked-resources-once",
    |s| {
        s.write("shared-resources/extensions/shared.ts", JS);
        s.write(
            "shared-resources/skills/shared-skill/SKILL.md",
            &skill("shared-skill"),
        );
        s.write("shared-resources/prompts/shared.md", "Shared prompt");
        s.write(
            "shared-resources/themes/shared.json",
            r#"{"name":"shared-theme"}"#,
        );
        s.link("shared-resources/extensions", "agent/extensions");
        s.link("shared-resources/skills", "agent/skills");
        s.link("shared-resources/prompts", "agent/prompts");
        s.link("shared-resources/themes", "agent/themes");
        s.link("shared-resources/extensions", ".pi/extensions");
        s.link("shared-resources/skills", ".pi/skills");
        s.link("shared-resources/prompts", ".pi/prompts");
        s.link("shared-resources/themes", ".pi/themes");
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_project_prompt_overrides,
    "resolve-project-prompt-overrides",
    |s| {
        s.write(".pi/prompts/is.md", "Is prompt");
        s.settings.state.lock().unwrap().project.prompts = vec!["!prompts/is.md".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_manifest_in_extensions_setting,
    "resolve-manifest-in-extensions-setting",
    |s| {
        s.write(
        "my-extensions-pkg/package.json",
        r#"{"name":"my-extensions-pkg","pi":{"extensions":["./extensions/clip.ts","./extensions/cost.ts"]}}"#,
    );
        s.write("my-extensions-pkg/extensions/clip.ts", JS);
        s.write("my-extensions-pkg/extensions/cost.ts", JS);
        s.write(
            "my-extensions-pkg/extensions/helper.ts",
            "export const x = 1;",
        );
        let pkg = s
            .dir
            .join("my-extensions-pkg")
            .to_string_lossy()
            .into_owned();
        s.settings.state.lock().unwrap().global.extensions = vec![pkg];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_skill_metadata_basedirs,
    "skill-metadata-basedirs",
    |s| {
        s.write("agent/skills/user-pi/SKILL.md", &skill("user-pi"));
        s.write(".pi/skills/project-pi/SKILL.md", &skill("project-pi"));
        s.write(".agents/skills/user-agents/SKILL.md", &skill("user-agents"));
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_project_agents_basedirs,
    "project-agents-basedirs",
    |s| {
        s.mkdir("repo/.git");
        s.mkdir("repo/packages/feature");
        s.write("repo/.agents/skills/repo/SKILL.md", &skill("repo"));
        s.write(
            "repo/packages/.agents/skills/package/SKILL.md",
            &skill("package"),
        );
        let cwd = s.dir.join("repo/packages/feature");
        render_resolved(
            &s.pm_with(&cwd, &s.agent_dir).resolve(None).unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_agents_scan_git_bounded,
    "agents-scan-git-bounded",
    |s| {
        s.mkdir("repo/.git");
        s.mkdir("repo/packages/feature");
        s.write(".agents/skills/above-repo/SKILL.md", &skill("above-repo"));
        s.write(
            "repo/.agents/skills/repo-root/SKILL.md",
            &skill("repo-root"),
        );
        s.write(
            "repo/packages/.agents/skills/nested/SKILL.md",
            &skill("nested"),
        );
        let cwd = s.dir.join("repo/packages/feature");
        render_resolved(
            &s.pm_with(&cwd, &s.agent_dir).resolve(None).unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(oracle_agents_scan_no_repo, "agents-scan-no-repo", |s| {
    s.mkdir("non-repo/a/b");
    s.write("non-repo/.agents/skills/root/SKILL.md", &skill("root"));
    s.write(
        "non-repo/a/.agents/skills/middle/SKILL.md",
        &skill("middle"),
    );
    let cwd = s.dir.join("non-repo/a/b");
    render_resolved(
        &s.pm_with(&cwd, &s.agent_dir).resolve(None).unwrap(),
        &s.dir,
    )
});

oracle_resolve_scenario!(
    oracle_agents_scan_root_md_ignored,
    "agents-scan-root-md-ignored",
    |s| {
        s.write(
            ".agents/skills/nested-skill/SKILL.md",
            &skill("nested-skill"),
        );
        s.write(
            ".agents/skills/third-party/vendor/pack/deep-skill.md",
            &skill("deep-skill"),
        );
        s.write(".agents/skills/root-file.md", &skill("root-file"));
        s.write(
            ".agents/skills/third-party/child-skill.md",
            &skill("child-skill"),
        );
        s.mkdir("work");
        let cwd = s.dir.join("work");
        render_resolved(
            &s.pm_with(&cwd, &s.agent_dir).resolve(None).unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_agents_home_user_scoped,
    "agents-home-user-scoped",
    |s| {
        let cwd = s.dir.join("tests/fixtures/nested");
        let local_agent_dir = s.dir.join(".pi/agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&local_agent_dir).unwrap();
        s.write(".agents/skills/home-skill/SKILL.md", &skill("home-skill"));
        render_resolved(
            &s.pm_with(&cwd, &local_agent_dir).resolve(None).unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_agents_junction_dedupe,
    "agents-junction-dedupe",
    |s| {
        s.link(".agents/skills", "agent/skills");
        s.write(".agents/skills/foo/SKILL.md", &skill("foo"));
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_ignore_in_skill_dirs, "ignore-in-skill-dirs", |s| {
    s.write("agent/skills/.gitignore", "venv\n__pycache__\n");
    s.write("agent/skills/good-skill/SKILL.md", &skill("good-skill"));
    s.write("agent/skills/venv/bad-skill/SKILL.md", &skill("bad-skill"));
    s.settings.state.lock().unwrap().global.skills = vec!["skills".to_string()];
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_parent_gitignore_not_applied,
    "parent-gitignore-not-applied",
    |s| {
        s.write(".gitignore", ".pi\n");
        s.write(".pi/skills/auto-skill/SKILL.md", &skill("auto-skill"));
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_resolve_extension_sources_local,
    "resolve-extension-sources-local",
    |s| {
        s.write("ext.ts", JS);
        let source = s.dir.join("ext.ts").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_resolve_extension_sources_manifest,
    "resolve-extension-sources-manifest",
    |s| {
        s.write(
            "my-package/package.json",
            r#"{"name":"my-package","pi":{"extensions":["./src/index.ts"],"skills":["./skills"]}}"#,
        );
        s.write("my-package/src/index.ts", JS);
        s.write("my-package/skills/my-skill/SKILL.md", &skill("my-skill"));
        let source = s.dir.join("my-package").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_resolve_extension_sources_tilde_manifest,
    "resolve-extension-sources-tilde-manifest",
    |s| {
        s.write("tilde-manifest-package/~extensions/main.ts", JS);
        s.write("tilde-manifest-package/~/extensions/alt.ts", JS);
        s.write(
            "tilde-manifest-package/~skills/direct-skill/SKILL.md",
            &skill("direct-skill"),
        );
        s.write(
            "tilde-manifest-package/~/skills/slash-skill/SKILL.md",
            &skill("slash-skill"),
        );
        s.write(
            "tilde-manifest-package/package.json",
            r#"{"name":"tilde-manifest-package","pi":{"extensions":["~extensions/main.ts","~/extensions/alt.ts"],"skills":["~skills","~/skills"]}}"#,
        );
        let source = s
            .dir
            .join("tilde-manifest-package")
            .to_string_lossy()
            .into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_resolve_extension_sources_auto_layout,
    "resolve-extension-sources-auto-layout",
    |s| {
        s.write("auto-pkg/extensions/main.ts", JS);
        s.write("auto-pkg/themes/dark.json", "{}");
        let source = s.dir.join("auto-pkg").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_resolve_skill_root_stop,
    "resolve-skill-root-stop",
    |s| {
        s.write(
            "skill-root-pkg/skills/root-skill/SKILL.md",
            &skill("root-skill"),
        );
        s.write(
            "skill-root-pkg/skills/root-skill/nested-skill/SKILL.md",
            &skill("nested-skill"),
        );
        let source = s.dir.join("skill-root-pkg").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_manifest_glob_extensions,
    "manifest-glob-extensions",
    |s| {
        s.write("manifest-pkg/extensions/local.ts", JS);
        s.write("manifest-pkg/node_modules/dep/extensions/remote.ts", JS);
        s.write("manifest-pkg/node_modules/dep/extensions/skip.ts", JS);
        s.write(
        "manifest-pkg/package.json",
        r#"{"name":"manifest-pkg","pi":{"extensions":["extensions","node_modules/dep/extensions","!**/skip.ts"]}}"#,
    );
        let source = s.dir.join("manifest-pkg").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(oracle_manifest_glob_skills, "manifest-glob-skills", |s| {
    s.write(
        "skill-manifest-pkg/skills/good-skill/SKILL.md",
        &skill("good-skill"),
    );
    s.write(
        "skill-manifest-pkg/skills/bad-skill/SKILL.md",
        &skill("bad-skill"),
    );
    s.write(
        "skill-manifest-pkg/package.json",
        r#"{"name":"skill-manifest-pkg","pi":{"skills":["skills","!**/bad-skill"]}}"#,
    );
    let source = s
        .dir
        .join("skill-manifest-pkg")
        .to_string_lossy()
        .into_owned();
    render_resolved(
        &s.pm()
            .resolve_extension_sources(&[source], false, false)
            .unwrap(),
        &s.dir,
    )
});

oracle_resolve_scenario!(
    oracle_manifest_glob_positive_expansion,
    "manifest-glob-positive-expansion",
    |s| {
        s.write(
            "skill-manifest-glob-pkg/plugins/pdf-to-markdown/skills/pdf-to-markdown/SKILL.md",
            &skill("pdf-to-markdown"),
        );
        s.write(
            "skill-manifest-glob-pkg/plugins/nutrient-dws/skills/document-processor-api/SKILL.md",
            &skill("document-processor-api"),
        );
        s.write(
            "skill-manifest-glob-pkg/package.json",
            r#"{"name":"skill-manifest-glob-pkg","pi":{"skills":["./plugins/*/skills"]}}"#,
        );
        let source = s
            .dir
            .join("skill-manifest-glob-pkg")
            .to_string_lossy()
            .into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_manifest_glob_semantics,
    "manifest-glob-semantics",
    |s| {
        s.write("manifest-glob-semantics-pkg/extension-files/z.ts", JS);
        s.write("manifest-glob-semantics-pkg/extension-files/a.ts", JS);
        s.write(
            "manifest-glob-semantics-pkg/extension-files/.ignored.ts",
            JS,
        );
        s.write(
            "manifest-glob-semantics-pkg/extension-files/nested/.hidden.ts",
            JS,
        );
        s.write(
            "manifest-glob-semantics-pkg/extension-groups/group/index.ts",
            JS,
        );
        s.write(
            "manifest-glob-semantics-pkg/plugins/local/skills/local-skill/SKILL.md",
            &skill("local-skill"),
        );
        s.write(
            "linked-plugin-source/skills/linked-skill/SKILL.md",
            &skill("linked-skill"),
        );
        s.link(
            "linked-plugin-source",
            "manifest-glob-semantics-pkg/plugins/linked",
        );
        s.write(
        "manifest-glob-semantics-pkg/package.json",
        r#"{"name":"manifest-glob-semantics-pkg","pi":{"extensions":["./extension-files/*.ts","./extension-files/**/.ignored.ts","./extension-files/nested/.hidden.ts","./extension-groups/*/"],"skills":["./plugins/*/skills","./plugins/linked/skills"]}}"#,
    );
        let source = s
            .dir
            .join("manifest-glob-semantics-pkg")
            .to_string_lossy()
            .into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

fn package_filter_scenario(scenario: &Scenario, package: &str, filter: Value) {
    let source_entry = serde_json::from_value::<PackageSourceEntry>(json!({
        "source": scenario.dir.join(package).to_string_lossy(),
        "autoload": filter.get("autoload").cloned(),
        "extensions": filter.get("extensions").cloned().unwrap_or(Value::Null),
        "skills": filter.get("skills").cloned().unwrap_or(Value::Null),
        "prompts": filter.get("prompts").cloned().unwrap_or(Value::Null),
        "themes": filter.get("themes").cloned().unwrap_or(Value::Null),
    }))
    .unwrap();
    scenario.settings.state.lock().unwrap().global.packages = vec![source_entry];
}

oracle_resolve_scenario!(
    oracle_package_filter_layered,
    "package-filter-layered",
    |s| {
        s.write("layered-pkg/extensions/foo.ts", JS);
        s.write("layered-pkg/extensions/bar.ts", JS);
        s.write("layered-pkg/extensions/baz.ts", JS);
        s.write(
            "layered-pkg/package.json",
            r#"{"name":"layered-pkg","pi":{"extensions":["extensions","!**/baz.ts"]}}"#,
        );
        package_filter_scenario(
            s,
            "layered-pkg",
            json!({"extensions": ["!**/bar.ts"], "skills": [], "prompts": [], "themes": []}),
        );
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_package_filter_exclude,
    "package-filter-exclude",
    |s| {
        s.write("pattern-pkg/extensions/foo.ts", JS);
        s.write("pattern-pkg/extensions/bar.ts", JS);
        s.write("pattern-pkg/extensions/baz.ts", JS);
        package_filter_scenario(
            s,
            "pattern-pkg",
            json!({"extensions": ["!**/baz.ts"], "skills": [], "prompts": [], "themes": []}),
        );
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_package_filter_themes, "package-filter-themes", |s| {
    s.write("theme-pkg/themes/nice.json", "{}");
    s.write("theme-pkg/themes/ugly.json", "{}");
    package_filter_scenario(
        s,
        "theme-pkg",
        json!({"extensions": [], "skills": [], "prompts": [], "themes": ["!ugly.json"]}),
    );
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(oracle_package_filter_combo, "package-filter-combo", |s| {
    s.write("combo-pkg/extensions/alpha.ts", JS);
    s.write("combo-pkg/extensions/beta.ts", JS);
    s.write("combo-pkg/extensions/gamma.ts", JS);
    package_filter_scenario(
        s,
        "combo-pkg",
        json!({"extensions": ["**/alpha.ts", "**/beta.ts", "!**/beta.ts"], "skills": [], "prompts": [], "themes": []}),
    );
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_package_filter_direct_paths,
    "package-filter-direct-paths",
    |s| {
        s.write("direct-pkg/extensions/one.ts", JS);
        s.write("direct-pkg/extensions/two.ts", JS);
        package_filter_scenario(
            s,
            "direct-pkg",
            json!({"extensions": ["extensions/one.ts"], "skills": [], "prompts": [], "themes": []}),
        );
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_autoload_delta_over_global,
    "autoload-delta-over-global",
    |s| {
        s.write(
            "agent/npm/node_modules/pi-tools/package.json",
            r#"{"name":"pi-tools","version":"1.0.0"}"#,
        );
        s.write("agent/npm/node_modules/pi-tools/extensions/foo.ts", JS);
        s.write("agent/npm/node_modules/pi-tools/extensions/bar.ts", JS);
        s.settings.state.lock().unwrap().global.packages =
            vec![PackageSourceEntry::Plain("npm:pi-tools".to_string())];
        s.settings.state.lock().unwrap().project.packages = vec![serde_json::from_value(json!({
            "source": "npm:pi-tools",
            "autoload": false,
            "extensions": ["-extensions/foo.ts"],
        }))
        .unwrap()];
        let rendered = render_resolved(&s.pm().resolve(None).unwrap(), &s.dir);
        // The JS harness returned the (empty) spawn log explicitly here.
        let mut object = match rendered {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        object.insert("spawnLog".into(), json!([]));
        Value::Object(object)
    }
);

oracle_resolve_scenario!(
    oracle_autoload_positive_only,
    "autoload-positive-only",
    |s| {
        s.write("positive-only-pkg/extensions/foo.ts", JS);
        s.write("positive-only-pkg/extensions/bar.ts", JS);
        s.write("positive-only-pkg/skills/foo/SKILL.md", "# Foo\n");
        let source =
            path_relative(&s.dir.join(".pi"), &s.dir.join("positive-only-pkg")).replace('\\', "/");
        s.settings.state.lock().unwrap().project.packages = vec![serde_json::from_value(json!({
            "source": source,
            "autoload": false,
            "extensions": ["+extensions/foo.ts"],
        }))
        .unwrap()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_top_level_include_exclude,
    "top-level-include-exclude",
    |s| {
        s.write("agent/extensions/keep.ts", JS);
        s.write("agent/extensions/remove.ts", JS);
        s.settings.state.lock().unwrap().global.extensions =
            vec!["extensions".to_string(), "!**/remove.ts".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_top_level_theme_glob, "top-level-theme-glob", |s| {
    s.write("agent/themes/dark.json", "{}");
    s.write("agent/themes/light.json", "{}");
    s.write("agent/themes/funky.json", "{}");
    s.settings.state.lock().unwrap().global.themes =
        vec!["themes".to_string(), "!funky.json".to_string()];
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_top_level_skill_exclude,
    "top-level-skill-exclude",
    |s| {
        s.write("agent/skills/good-skill/SKILL.md", &skill("good-skill"));
        s.write("agent/skills/bad-skill/SKILL.md", &skill("bad-skill"));
        s.settings.state.lock().unwrap().global.skills =
            vec!["skills".to_string(), "!**/bad-skill".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_top_level_prompt_exclude,
    "top-level-prompt-exclude",
    |s| {
        s.write("agent/prompts/review.md", "Review code");
        s.write("agent/prompts/explain.md", "Explain code");
        s.settings.state.lock().unwrap().global.prompts =
            vec!["prompts".to_string(), "!explain.md".to_string()];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_top_level_patternless, "top-level-patternless", |s| {
    s.write("agent/extensions/my-ext.ts", JS);
    s.settings.state.lock().unwrap().global.extensions = vec!["extensions/my-ext.ts".to_string()];
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_force_include_toplevel,
    "force-include-toplevel",
    |s| {
        s.write("agent/extensions/keep.ts", JS);
        s.write("agent/extensions/excluded.ts", JS);
        s.write("agent/extensions/force-back.ts", JS);
        s.settings.state.lock().unwrap().global.extensions = vec![
            "extensions".to_string(),
            "!extensions/*.ts".to_string(),
            "+extensions/force-back.ts".to_string(),
        ];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_force_include_package, "force-include-package", |s| {
    s.write("force-pkg/extensions/alpha.ts", JS);
    s.write("force-pkg/extensions/beta.ts", JS);
    s.write("force-pkg/extensions/gamma.ts", JS);
    package_filter_scenario(
        s,
        "force-pkg",
        json!({"extensions": ["!**/*.ts", "+extensions/beta.ts"], "skills": [], "prompts": [], "themes": []}),
    );
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_force_include_multi_skills,
    "force-include-multi-skills",
    |s| {
        s.write("multi-force-pkg/skills/skill-a/SKILL.md", &skill("skill-a"));
        s.write("multi-force-pkg/skills/skill-b/SKILL.md", &skill("skill-b"));
        s.write("multi-force-pkg/skills/skill-c/SKILL.md", &skill("skill-c"));
        package_filter_scenario(
            s,
            "multi-force-pkg",
            json!({"extensions": [], "skills": ["!**/*", "+skills/skill-a", "+skills/skill-c"], "prompts": [], "themes": []}),
        );
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_force_include_after_specific_exclusion,
    "force-include-after-specific-exclusion",
    |s| {
        s.write("agent/extensions/a.ts", JS);
        s.write("agent/extensions/b.ts", JS);
        s.settings.state.lock().unwrap().global.extensions = vec![
            "extensions".to_string(),
            "!extensions/b.ts".to_string(),
            "+extensions/b.ts".to_string(),
        ];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_force_include_manifest,
    "force-include-manifest",
    |s| {
        s.write("manifest-force-pkg/extensions/one.ts", JS);
        s.write("manifest-force-pkg/extensions/two.ts", JS);
        s.write("manifest-force-pkg/extensions/three.ts", JS);
        s.write(
        "manifest-force-pkg/package.json",
        r#"{"name":"manifest-force-pkg","pi":{"extensions":["extensions","!**/two.ts","+extensions/two.ts"]}}"#,
    );
        let source = s
            .dir
            .join("manifest-force-pkg")
            .to_string_lossy()
            .into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_force_include_themes_prompts,
    "force-include-themes-prompts",
    |s| {
        s.write("agent/themes/dark.json", "{}");
        s.write("agent/themes/light.json", "{}");
        s.write("agent/themes/special.json", "{}");
        s.write("agent/prompts/review.md", "Review");
        s.write("agent/prompts/explain.md", "Explain");
        s.write("agent/prompts/debug.md", "Debug");
        {
            let mut state = s.settings.state.lock().unwrap();
            state.global.themes = vec![
                "themes".to_string(),
                "!themes/*.json".to_string(),
                "+themes/special.json".to_string(),
            ];
            state.global.prompts = vec![
                "prompts".to_string(),
                "!prompts/*.md".to_string(),
                "+prompts/debug.md".to_string(),
            ];
        }
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(
    oracle_force_exclude_toplevel,
    "force-exclude-toplevel",
    |s| {
        s.write("agent/extensions/alpha.ts", JS);
        s.write("agent/extensions/beta.ts", JS);
        s.settings.state.lock().unwrap().global.extensions = vec![
            "extensions".to_string(),
            "+extensions/alpha.ts".to_string(),
            "-extensions/alpha.ts".to_string(),
        ];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_force_exclude_package, "force-exclude-package", |s| {
    s.write("force-exclude-pkg/extensions/alpha.ts", JS);
    s.write("force-exclude-pkg/extensions/beta.ts", JS);
    package_filter_scenario(
        s,
        "force-exclude-pkg",
        json!({"extensions": ["extensions/*.ts", "+extensions/alpha.ts", "-extensions/alpha.ts"], "skills": [], "prompts": [], "themes": []}),
    );
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_dedupe_local_project_wins,
    "dedupe-local-project-wins",
    |s| {
        s.write("shared-pkg/extensions/shared.ts", JS);
        let pkg = s.dir.join("shared-pkg").to_string_lossy().into_owned();
        s.settings.state.lock().unwrap().global.packages =
            vec![PackageSourceEntry::Plain(pkg.clone())];
        s.settings.state.lock().unwrap().project.packages = vec![PackageSourceEntry::Plain(pkg)];
        render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
    }
);

oracle_resolve_scenario!(oracle_dedupe_different_kept, "dedupe-different-kept", |s| {
    s.write("pkg1/extensions/from-pkg1.ts", JS);
    s.write("pkg2/extensions/from-pkg2.ts", JS);
    s.settings.state.lock().unwrap().global.packages = vec![PackageSourceEntry::Plain(
        s.dir.join("pkg1").to_string_lossy().into_owned(),
    )];
    s.settings.state.lock().unwrap().project.packages = vec![PackageSourceEntry::Plain(
        s.dir.join("pkg2").to_string_lossy().into_owned(),
    )];
    render_resolved(&s.pm().resolve(None).unwrap(), &s.dir)
});

oracle_resolve_scenario!(
    oracle_multifile_subdir_index_only,
    "multifile-subdir-index-only",
    |s| {
        s.write(
            "multifile-pkg/extensions/subagent/index.ts",
            "import { helper } from \"./agents.ts\";\nexport default function(api) {}",
        );
        s.write(
            "multifile-pkg/extensions/subagent/agents.ts",
            "export function helper() { return \"helper\"; }",
        );
        s.write("multifile-pkg/extensions/standalone.ts", JS);
        let source = s.dir.join("multifile-pkg").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(
    oracle_multifile_manifest_subdir,
    "multifile-manifest-subdir",
    |s| {
        s.write(
            "manifest-subdir-pkg/extensions/custom/package.json",
            r#"{"pi":{"extensions":["./main.ts"]}}"#,
        );
        s.write("manifest-subdir-pkg/extensions/custom/main.ts", JS);
        s.write(
            "manifest-subdir-pkg/extensions/custom/utils.ts",
            "export const util = 1;",
        );
        let source = s
            .dir
            .join("manifest-subdir-pkg")
            .to_string_lossy()
            .into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

oracle_resolve_scenario!(oracle_multifile_mixed, "multifile-mixed", |s| {
    s.write("mixed-pkg/extensions/simple.ts", JS);
    s.write(
        "mixed-pkg/extensions/complex/index.ts",
        "import { a } from './a.ts'; export default function(api) {}",
    );
    s.write("mixed-pkg/extensions/complex/a.ts", "export const a = 1;");
    s.write("mixed-pkg/extensions/complex/b.ts", "export const b = 2;");
    let source = s.dir.join("mixed-pkg").to_string_lossy().into_owned();
    render_resolved(
        &s.pm()
            .resolve_extension_sources(&[source], false, false)
            .unwrap(),
        &s.dir,
    )
});

oracle_resolve_scenario!(
    oracle_multifile_no_entry_skipped,
    "multifile-no-entry-skipped",
    |s| {
        s.write(
            "no-entry-pkg/extensions/broken/helper.ts",
            "export const x = 1;",
        );
        s.write(
            "no-entry-pkg/extensions/broken/another.ts",
            "export const y = 2;",
        );
        s.write("no-entry-pkg/extensions/valid.ts", JS);
        let source = s.dir.join("no-entry-pkg").to_string_lossy().into_owned();
        render_resolved(
            &s.pm()
                .resolve_extension_sources(&[source], false, false)
                .unwrap(),
            &s.dir,
        )
    }
);

// ===========================================================================
// Settings normalization / install-path scenarios
// ===========================================================================

#[test]
fn oracle_settings_normalize_global_local() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-normalize-global-local");
    scenario.mkdir("packages/local-global-pkg/extensions");
    scenario.write("packages/local-global-pkg/extensions/index.ts", JS);
    let added = scenario
        .pm()
        .add_source_to_settings("./packages/local-global-pkg", false);
    let packages = scenario.settings.global_settings().packages;
    let actual = json!({
        "added": added,
        "packages": packages.iter().map(|entry| entry.source()).collect::<Vec<_>>(),
    });
    assert_matches_oracle("settings-normalize-global-local", &actual, temp.path());
}

#[test]
fn oracle_settings_normalize_project_local() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-normalize-project-local");
    scenario.write("project-local-pkg/extensions/index.ts", JS);
    let added = scenario
        .pm()
        .add_source_to_settings("./project-local-pkg", true);
    let packages = scenario.settings.project_settings().packages;
    let actual = json!({
        "added": added,
        "packages": packages.iter().map(|entry| entry.source()).collect::<Vec<_>>(),
    });
    assert_matches_oracle("settings-normalize-project-local", &actual, temp.path());
}

#[test]
fn oracle_settings_remove_equivalent_forms() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-remove-equivalent-forms");
    scenario.write("remove-local-pkg/extensions/index.ts", JS);
    let manager = scenario.pm();
    manager.add_source_to_settings("./remove-local-pkg", false);
    let absolute = format!(
        "{}/",
        scenario.dir.join("remove-local-pkg").to_string_lossy()
    );
    let removed = manager.remove_source_from_settings(&absolute, false);
    let packages = scenario.settings.global_settings().packages;
    let actual = json!({
        "removed": removed,
        "packages": packages.iter().map(|entry| entry.source()).collect::<Vec<_>>(),
    });
    assert_matches_oracle("settings-remove-equivalent-forms", &actual, temp.path());
}

#[test]
fn oracle_settings_same_ref_noop() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-same-ref-noop");
    let manager = scenario.pm();
    let first = manager.add_source_to_settings("git:github.com/user/repo@v1", false);
    let second = manager.add_source_to_settings("git:github.com/user/repo@v1", false);
    let packages = scenario.settings.global_settings().packages;
    let actual = json!({
        "first": first,
        "second": second,
        "packages": packages.iter().map(|entry| entry.source()).collect::<Vec<_>>(),
    });
    assert_matches_oracle("settings-same-ref-noop", &actual, temp.path());
}

#[test]
fn oracle_settings_ref_update() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-ref-update");
    let manager = scenario.pm();
    manager.add_source_to_settings("git:github.com/user/repo@v1", false);
    let updated = manager.add_source_to_settings("git:github.com/user/repo@v2", false);
    let packages = scenario.settings.global_settings().packages;
    let actual = json!({
        "updated": updated,
        "packages": packages.iter().map(|entry| entry.source()).collect::<Vec<_>>(),
    });
    assert_matches_oracle("settings-ref-update", &actual, temp.path());
}

#[test]
fn oracle_settings_filter_preserving_ref_update() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "settings-filter-preserving-ref-update");
    scenario.settings.state.lock().unwrap().global.packages = vec![serde_json::from_value(json!({
        "source": "git:github.com/user/repo@v1",
        "extensions": ["extensions/main.ts"],
        "skills": [],
        "prompts": ["prompts/review.md"],
        "themes": ["themes/dark.json"],
    }))
    .unwrap()];
    let updated = scenario
        .pm()
        .add_source_to_settings("git:github.com/user/repo@v2", false);
    let packages: Vec<Value> = scenario
        .settings
        .global_settings()
        .packages
        .iter()
        .map(|entry| serde_json::to_value(entry).unwrap())
        .collect();
    let actual = json!({ "updated": updated, "packages": packages });
    assert_matches_oracle(
        "settings-filter-preserving-ref-update",
        &actual,
        temp.path(),
    );
}

#[test]
fn oracle_git_install_path_traversal() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "git-install-path-traversal");
    let manager = scenario.pm();
    let parsed = manager.parse_source("git:github.com/user/repo");
    let traversal = super::ParsedSource::Git(crate::coding_agent::utils::git::GitSource {
        repo_type: "git",
        repo: "git@evil.example:../../victim/repo".to_string(),
        host: "evil.example".to_string(),
        path: "../../victim/repo".to_string(),
        ref_: None,
        pinned: false,
    });
    let _ = parsed;
    let mut out = Map::new();
    for (scope_name, scope) in [
        ("user", SourceScope::User),
        ("project", SourceScope::Project),
        ("temporary", SourceScope::Temporary),
    ] {
        let git_source = match &traversal {
            super::ParsedSource::Git(git) => git.clone(),
            _ => unreachable!(),
        };
        match manager.get_git_install_path(&git_source, scope) {
            Ok(path) => {
                out.insert(scope_name.to_string(), json!(path));
            }
            Err(error) => {
                out.insert(scope_name.to_string(), json!({ "error": error.message }));
            }
        }
    }
    let actual = Value::Object(out);
    assert_matches_oracle("git-install-path-traversal", &actual, temp.path());
}

#[test]
fn oracle_temporary_npm_path() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "temporary-npm-path");
    let manager = scenario.pm();
    let parsed = manager.parse_source("npm:left-pad");
    let super::ParsedSource::Npm(npm) = parsed else {
        unreachable!()
    };
    let install_path = manager
        .get_npm_install_path(&npm, SourceScope::Temporary)
        .unwrap();
    let temp_root = scenario.agent_dir.join("tmp").join("extensions");
    let actual = json!({
        "installPath": install_path,
        "tempRoot": temp_root.to_string_lossy(),
    });
    assert_matches_oracle("temporary-npm-path", &actual, temp.path());
}

#[test]
fn oracle_list_configured_packages() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "list-configured-packages");
    scenario.write(
        "agent/npm/node_modules/user-pkg/package.json",
        r#"{"name":"user-pkg","version":"1.0.0"}"#,
    );
    scenario.write("agent/npm/node_modules/user-pkg/extensions/index.ts", JS);
    scenario.write(
        ".pi/npm/node_modules/project-pkg/package.json",
        r#"{"name":"project-pkg","version":"1.0.0"}"#,
    );
    {
        let mut state = scenario.settings.state.lock().unwrap();
        state.global.packages = vec![
            PackageSourceEntry::Plain("npm:user-pkg".to_string()),
            serde_json::from_value(json!({"source": "npm:filtered-pkg", "extensions": []}))
                .unwrap(),
            PackageSourceEntry::Plain("local-missing".to_string()),
        ];
        state.project.packages = vec![PackageSourceEntry::Plain("npm:project-pkg".to_string())];
    }
    let nowhere = scenario.dir.join("nowhere");
    scenario
        .runner
        .script(Arc::new(move |_command, args, _cwd| {
            if args[0] == "root" {
                return Ok(SpawnOutcome {
                    stdout: format!("{}\n", nowhere.to_string_lossy()),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "list" {
                return Ok(SpawnOutcome {
                    stdout: "[]".to_string(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Err(format!("unexpected spawn {}", args.join(" ")))
        }));
    let manager = scenario.pm();
    let configured: Vec<Value> = manager
        .list_configured_packages()
        .iter()
        .map(|pkg| {
            let mut object = Map::new();
            object.insert("source".into(), json!(pkg.source));
            object.insert(
                "scope".into(),
                json!(match pkg.scope {
                    SourceScope::User => "user",
                    SourceScope::Project => "project",
                    SourceScope::Temporary => "temporary",
                }),
            );
            object.insert("filtered".into(), json!(pkg.filtered));
            if let Some(installed_path) = &pkg.installed_path {
                object.insert(
                    "installedPath".into(),
                    json!(
                        path_relative(&scenario.dir, Path::new(installed_path)).replace('\\', "/")
                    ),
                );
            }
            Value::Object(object)
        })
        .collect();
    let actual = json!({
        "value": configured,
        "spawnLog": scenario.runner.log_json(),
    });
    assert_matches_oracle("list-configured-packages", &actual, temp.path());
}

// ===========================================================================
// Flow scenarios (spawn-driven, argv-pinned)
// ===========================================================================

fn flow(oracle_name: &str, build: impl FnOnce(&Scenario) -> Value) {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), oracle_name);
    let previous_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &scenario.dir);
    let mut actual = build(&scenario);
    match previous_home {
        Some(home) => std::env::set_var("HOME", home),
        None => std::env::remove_var("HOME"),
    }
    if let Value::Object(object) = &mut actual {
        if object.get("__noSpawnLog").and_then(Value::as_bool) == Some(true) {
            object.remove("__noSpawnLog");
        } else if scenario.runner.log_len() > 0 {
            object.insert("spawnLog".into(), scenario.runner.log_json());
        }
    }
    assert_matches_oracle(oracle_name, &actual, temp.path());
}

#[test]
fn oracle_flow_npm_install_default() {
    flow("flow-npm-install-default", |s| {
        s.pm().install("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_install_mise_argv() {
    flow("flow-npm-install-mise-argv", |s| {
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@20".into(),
            "--".into(),
            "npm".into(),
        ]);
        s.pm().install("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_remove_default() {
    flow("flow-npm-remove-default", |s| {
        s.mkdir("agent/npm");
        s.pm().remove("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_remove_bun_argv() {
    flow("flow-npm-remove-bun-argv", |s| {
        s.mkdir("agent/npm");
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "bun@1".into(),
            "--".into(),
            "bun".into(),
        ]);
        s.pm().remove("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_remove_pnpm_argv() {
    flow("flow-npm-remove-pnpm-argv", |s| {
        s.mkdir("agent/npm");
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        s.pm().remove("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_install_bun_argv() {
    flow("flow-npm-install-bun-argv", |s| {
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "bun@1".into(),
            "--".into(),
            "bun".into(),
        ]);
        s.pm().install("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_npm_install_pnpm_argv() {
    flow("flow-npm-install-pnpm-argv", |s| {
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        s.pm().install("npm:@scope/pkg", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_install_success() {
    flow("flow-git-install-success", |s| {
        s.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                std::fs::write(
                    std::path::Path::new(&args[2]).join("package.json"),
                    r#"{"name":"repo","version":"1.0.0"}"#,
                )
                .unwrap();
            }
            Ok(default_outcome())
        }));
        s.pm().install("git:github.com/user/repo", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_install_clone_failure() {
    flow("flow-git-install-clone-failure", |s| {
        s.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                return Err("simulated git clone failure".to_string());
            }
            Ok(default_outcome())
        }));
        let outcome = s.pm().install("git:github.com/user/repo", false);
        json!({ "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()) })
    });
}

#[test]
fn oracle_flow_git_install_dependency_failure() {
    flow("flow-git-install-dependency-failure", |s| {
        s.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                std::fs::write(
                    std::path::Path::new(&args[2]).join("package.json"),
                    r#"{"name":"repo","version":"1.0.0"}"#,
                )
                .unwrap();
            }
            if command == "npm" {
                return Err("simulated dependency install failure".to_string());
            }
            Ok(default_outcome())
        }));
        let outcome = s.pm().install("git:github.com/user/repo", false);
        json!({ "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()) })
    });
}

#[test]
fn oracle_flow_git_pinned_ref_reconcile() {
    flow("flow-git-pinned-ref-reconcile", |s| {
        let target_dir = s
            .agent_dir
            .join("git")
            .join("github.com")
            .join("user")
            .join("repo");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(
            target_dir.join("package.json"),
            r#"{"name":"repo","version":"1.0.0"}"#,
        )
        .unwrap();
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "rev-parse" {
                if args[1] == "HEAD" {
                    return Ok(SpawnOutcome {
                        stdout: "old-head\n".into(),
                        stderr: String::new(),
                        code: 0,
                    });
                }
                if args[1] == "FETCH_HEAD^{commit}" {
                    return Ok(SpawnOutcome {
                        stdout: "new-head\n".into(),
                        stderr: String::new(),
                        code: 0,
                    });
                }
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.pm()
            .install("git:github.com/user/repo@v2", false)
            .unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_update_target_reconcile() {
    flow("flow-git-update-target-reconcile", |s| {
        let target_dir = s
            .agent_dir
            .join("git")
            .join("github.com")
            .join("user")
            .join("repo");
        std::fs::create_dir_all(&target_dir).unwrap();
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
                return Err("no upstream configured".to_string());
            }
            if args[0] == "rev-parse" && args[1] == "HEAD" {
                return Ok(SpawnOutcome {
                    stdout: "old-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse"
                && (args[1] == "origin/HEAD" || args[1] == "origin/HEAD^{commit}")
            {
                return Ok(SpawnOutcome {
                    stdout: "new-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "symbolic-ref" && args[1] == "refs/remotes/origin/HEAD" {
                return Ok(SpawnOutcome {
                    stdout: "refs/remotes/origin/main\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" {
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.pm().install("git:github.com/user/repo", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_deps_plain_install_configured() {
    flow("flow-git-deps-plain-install-configured", |s| {
        s.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                std::fs::write(
                    std::path::Path::new(&args[2]).join("package.json"),
                    r#"{"name":"repo","version":"1.0.0"}"#,
                )
                .unwrap();
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        s.pm().install("git:github.com/user/repo", false).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_update_deps_omit_dev() {
    flow("flow-git-update-deps-omit-dev", |s| {
        s.mkdir(".pi/git/github.com/user/repo");
        s.write(
            ".pi/git/github.com/user/repo/package.json",
            r#"{"name":"repo","version":"1.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
                return Ok(SpawnOutcome {
                    stdout: "origin/main\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse"
                && (args[1] == "@{upstream}" || args[1] == "@{upstream}^{commit}")
            {
                return Ok(SpawnOutcome {
                    stdout: "remote-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" && args[1] == "HEAD" {
                return Ok(SpawnOutcome {
                    stdout: "local-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" {
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().project.packages = vec![PackageSourceEntry::Plain(
            "git:github.com/user/repo".to_string(),
        )];
        s.pm().update(Some("git:github.com/user/repo")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_repair_current_checkout() {
    flow("flow-git-repair-current-checkout", |s| {
        let target_dir = s
            .agent_dir
            .join("git")
            .join("github.com")
            .join("user")
            .join("repo");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(
            target_dir.join("package.json"),
            r#"{"name":"repo","version":"1.0.0","dependencies":{"dependency":"1.0.0"}}"#,
        )
        .unwrap();
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
                return Ok(SpawnOutcome {
                    stdout: "origin/main\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse"
                && (args[1] == "@{upstream}"
                    || args[1] == "@{upstream}^{commit}"
                    || args[1] == "HEAD")
            {
                return Ok(SpawnOutcome {
                    stdout: "current-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" {
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().global.packages = vec![PackageSourceEntry::Plain(
            "git:github.com/user/repo".to_string(),
        )];
        s.pm().update(Some("git:github.com/user/repo")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_git_clean_failure_repairs_deps() {
    flow("flow-git-clean-failure-repairs-deps", |s| {
        let target_dir = s
            .agent_dir
            .join("git")
            .join("github.com")
            .join("user")
            .join("repo");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(
            target_dir.join("package.json"),
            r#"{"name":"repo","version":"1.0.0","dependencies":{"dependency":"1.0.0"}}"#,
        )
        .unwrap();
        s.runner.script(Arc::new(|command, args, _cwd| {
            if command == "git" && args[0] == "clean" {
                return Err("simulated clean failure".to_string());
            }
            if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
                return Ok(SpawnOutcome {
                    stdout: "origin/main\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse"
                && (args[1] == "@{upstream}" || args[1] == "@{upstream}^{commit}")
            {
                return Ok(SpawnOutcome {
                    stdout: "new-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" && args[1] == "HEAD" {
                return Ok(SpawnOutcome {
                    stdout: "old-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" {
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().global.packages = vec![PackageSourceEntry::Plain(
            "git:github.com/user/repo".to_string(),
        )];
        let outcome = s.pm().update(Some("git:github.com/user/repo"));
        json!({ "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()) })
    });
}

#[test]
fn oracle_flow_git_update_mise_argv_deps() {
    flow("flow-git-update-mise-argv-deps", |s| {
        s.mkdir(".pi/git/github.com/user/repo");
        s.write(
            ".pi/git/github.com/user/repo/package.json",
            r#"{"name":"repo","version":"1.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "rev-parse" && args[1] == "--abbrev-ref" && args[2] == "@{upstream}" {
                return Ok(SpawnOutcome {
                    stdout: "origin/main\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse"
                && (args[1] == "@{upstream}" || args[1] == "@{upstream}^{commit}")
            {
                return Ok(SpawnOutcome {
                    stdout: "remote-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" && args[1] == "HEAD" {
                return Ok(SpawnOutcome {
                    stdout: "local-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[0] == "rev-parse" {
                return Err(format!("Unexpected capture: {}", args.join(" ")));
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@20".into(),
            "--".into(),
            "pnpm".into(),
        ]);
        s.settings.state.lock().unwrap().project.packages = vec![PackageSourceEntry::Plain(
            "git:github.com/user/repo".to_string(),
        )];
        s.pm().update(Some("git:github.com/user/repo")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_update_npm_range_spec() {
    flow("flow-update-npm-range-spec", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "view" {
                return Ok(SpawnOutcome {
                    stdout: r#"["1.0.0","1.2.0"]"#.into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example@^1.0.0".to_string())];
        s.pm().update(Some("npm:example")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_update_npm_current_skip() {
    flow("flow-update-npm-current-skip", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.3.1"}"#,
        );
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "view" {
                return Ok(SpawnOutcome {
                    stdout: r#"["1.0.0","1.3.1","1.0.2"]"#.into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example@^1.0.0".to_string())];
        s.pm().update(Some("npm:example")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_update_npm_newer_installed_skip() {
    flow("flow-update-npm-newer-installed-skip", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"2.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, args, _cwd| {
            if args[0] == "view" {
                return Ok(SpawnOutcome {
                    stdout: r#""1.9.0""#.into(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example".to_string())];
        s.pm().update(Some("npm:example")).unwrap();
        json!({})
    });
}

#[test]
fn oracle_flow_update_migrate_legacy_user_install() {
    flow("flow-update-migrate-legacy-user-install", |s| {
        let legacy_root = s.dir.join("legacy-global").join("node_modules");
        let legacy_path = legacy_root.join("legacy-pkg");
        let managed_path = s
            .agent_dir
            .join("npm")
            .join("node_modules")
            .join("legacy-pkg");
        std::fs::create_dir_all(&legacy_path).unwrap();
        std::fs::write(
            legacy_path.join("package.json"),
            r#"{"name":"legacy-pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        let legacy_root_clone = legacy_root.clone();
        s.runner.script(Arc::new(move |command, args, _cwd| {
            if command == "npm" && args[0] == "root" {
                return Ok(SpawnOutcome {
                    stdout: format!("{}\n", legacy_root_clone.to_string_lossy()),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if command == "npm" && args[0] == "install" {
                std::fs::create_dir_all(&managed_path).unwrap();
                std::fs::write(
                    managed_path.join("package.json"),
                    r#"{"name":"legacy-pkg","version":"1.0.0"}"#,
                )
                .unwrap();
                return Ok(default_outcome());
            }
            Err(format!("Unexpected: {}", join_command_args(command, args)))
        }));
        s.settings.state.lock().unwrap().global.packages =
            vec![PackageSourceEntry::Plain("npm:legacy-pkg".to_string())];
        let manager = s.pm();
        let before = manager.get_installed_path("npm:legacy-pkg", SourceScope::User);
        manager.update(Some("npm:legacy-pkg")).unwrap();
        let after = manager.get_installed_path("npm:legacy-pkg", SourceScope::User);
        json!({
            "before": before.map(|path| path_relative(&s.dir, Path::new(&path)).replace('\\', "/")),
            "after": after.map(|path| path_relative(&s.dir, Path::new(&path)).replace('\\', "/")),
        })
    });
}

#[test]
fn oracle_flow_update_batch_per_scope() {
    flow("flow-update-batch-per-scope", |s| {
        for name in ["user-old", "user-current", "user-unknown"] {
            let path = s.agent_dir.join("npm").join("node_modules").join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            )
            .unwrap();
        }
        for name in ["project-old", "project-current"] {
            let path = s
                .dir
                .join(".pi")
                .join("npm")
                .join("node_modules")
                .join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("package.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            )
            .unwrap();
        }
        s.runner.script(Arc::new(|command, args, _cwd| {
            if args[0] == "view" {
                return match args[1].as_str() {
                    "user-old" | "project-old" => Ok(SpawnOutcome {
                        stdout: r#""2.0.0""#.into(),
                        stderr: String::new(),
                        code: 0,
                    }),
                    "user-current" | "project-current" => Ok(SpawnOutcome {
                        stdout: r#""1.0.0""#.into(),
                        stderr: String::new(),
                        code: 0,
                    }),
                    "user-unknown" => Err("registry unavailable".to_string()),
                    other => Err(format!("Unexpected lookup: {other}")),
                };
            }
            if command == "npm" && args[0] == "install" {
                return Ok(default_outcome());
            }
            if command == "git" && args[0] == "clone" {
                std::fs::create_dir_all(&args[2]).unwrap();
                return Ok(default_outcome());
            }
            if command == "git" {
                return Ok(default_outcome());
            }
            Err(format!(
                "Unexpected spawn: {}",
                join_command_args(command, args)
            ))
        }));
        {
            let mut state = s.settings.state.lock().unwrap();
            state.global.packages = [
                "npm:user-old",
                "npm:user-current",
                "npm:user-unknown",
                "npm:user-pinned@1.0.0",
                "git:github.com/example/user-repo-a",
                "git:github.com/example/user-repo-b",
                "git:github.com/example/user-repo-pinned@v1",
            ]
            .iter()
            .map(|source| PackageSourceEntry::Plain(source.to_string()))
            .collect();
            state.project.packages = [
                "npm:project-old",
                "npm:project-current",
                "npm:project-missing",
                "git:github.com/example/project-repo-a",
            ]
            .iter()
            .map(|source| PackageSourceEntry::Plain(source.to_string()))
            .collect();
        }
        s.pm().update(None).unwrap();
        // Concurrent execution order is nondeterministic; compare sorted
        // unique spawn strings plus per-call counts.
        let calls = s.runner.calls();
        // Object keys survive the value-level norm(); mask the temp root here.
        let root_text = s.dir.parent().unwrap().to_string_lossy().into_owned();
        let mut counts: Map<String, Value> = Map::new();
        let mut spawns: Vec<String> = Vec::new();
        for (command, args) in &calls {
            let key = join_command_args(command, args).replace(&root_text, "$T");
            let current = counts.get(&key).and_then(Value::as_u64).unwrap_or_default();
            counts.insert(key.clone(), json!(current + 1));
            if !spawns.contains(&key) {
                spawns.push(key);
            }
        }
        spawns.sort();
        json!({ "__noSpawnLog": true, "spawns": spawns, "counts": counts })
    });
}

#[test]
fn oracle_flow_update_suggest_npm_prefix() {
    flow("flow-update-suggest-npm-prefix", |s| {
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example".to_string())];
        let outcome = s.pm().update(Some("example"));
        json!({ "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()) })
    });
}

#[test]
fn oracle_flow_update_suggest_git_prefix() {
    flow("flow-update-suggest-git-prefix", |s| {
        s.settings.state.lock().unwrap().project.packages = vec![PackageSourceEntry::Plain(
            "git:github.com/example/repo".to_string(),
        )];
        let outcome = s.pm().update(Some("github.com/example/repo"));
        json!({ "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()) })
    });
}

#[test]
fn oracle_flow_offline_resolve_skips_install() {
    flow("flow-offline-resolve-skips-install", |s| {
        std::env::set_var("PI_OFFLINE", "1");
        s.settings.state.lock().unwrap().project.packages = vec![
            PackageSourceEntry::Plain("npm:missing-package".to_string()),
            PackageSourceEntry::Plain("git:github.com/example/missing-repo".to_string()),
        ];
        let resolved = s.pm().resolve(None).unwrap();
        std::env::remove_var("PI_OFFLINE");
        let package_origin = resolved
            .extensions
            .iter()
            .chain(&resolved.skills)
            .chain(&resolved.prompts)
            .chain(&resolved.themes)
            .filter(|entry| entry.metadata.origin == super::PathMetadataOrigin::Package)
            .count();
        json!({ "packageOriginCount": package_origin })
    });
}

#[test]
fn oracle_flow_offline_temp_git_skip_refresh() {
    flow("flow-offline-temp-git-skip-refresh", |s| {
        std::env::set_var("PI_OFFLINE", "1");
        let manager = s.pm();
        let git_source = "git:github.com/example/repo";
        let parsed = manager.parse_source(git_source);
        let super::ParsedSource::Git(git) = &parsed else {
            unreachable!()
        };
        let installed_path = manager
            .get_git_install_path(git, SourceScope::Temporary)
            .unwrap();
        std::fs::create_dir_all(std::path::Path::new(&installed_path).join("extensions")).unwrap();
        std::fs::write(
            std::path::Path::new(&installed_path)
                .join("extensions")
                .join("index.ts"),
            format!("{JS};"),
        )
        .unwrap();
        let resolved = manager
            .resolve_extension_sources(&[git_source.to_string()], false, true)
            .unwrap();
        std::env::remove_var("PI_OFFLINE");
        render_resolved(&resolved, &s.dir)
    });
}

#[test]
fn oracle_flow_offline_resolve_no_npm_view() {
    flow("flow-offline-resolve-no-npm-view", |s| {
        std::env::set_var("PI_OFFLINE", "1");
        s.mkdir(".pi/npm/node_modules/example/extensions");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.0.0"}"#,
        );
        s.write(
            ".pi/npm/node_modules/example/extensions/index.ts",
            &format!("{JS};"),
        );
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example@^1.0.0".to_string())];
        let resolved = s.pm().resolve(None).unwrap();
        std::env::remove_var("PI_OFFLINE");
        render_resolved(&resolved, &s.dir)
    });
}

#[test]
fn oracle_flow_resolve_pinned_mismatch_reinstalls() {
    flow("flow-resolve-pinned-mismatch-reinstalls", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.0.0"}"#,
        );
        let pkg_path = s.dir.join(".pi/npm/node_modules/example/package.json");
        s.runner.script(Arc::new(move |_command, args, _cwd| {
            if args[0] == "install" {
                std::fs::write(&pkg_path, r#"{"name":"example","version":"2.0.0"}"#).unwrap();
            }
            Ok(default_outcome())
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example@2.0.0".to_string())];
        let resolved = s.pm().resolve(None).unwrap();
        render_resolved(&resolved, &s.dir)
    });
}

fn package_update_json(update: &super::PackageUpdate) -> Value {
    json!({
        "source": update.source,
        "displayName": update.display_name,
        "type": update.update_type,
        "scope": match update.scope {
            SourceScope::User => "user",
            SourceScope::Project => "project",
            SourceScope::Temporary => "temporary",
        },
    })
}

#[test]
fn oracle_flow_check_updates_offline_empty() {
    flow("flow-check-updates-offline-empty", |s| {
        std::env::set_var("PI_OFFLINE", "1");
        let updates = s.pm().check_for_available_updates().unwrap();
        std::env::remove_var("PI_OFFLINE");
        json!({ "value": updates.iter().map(package_update_json).collect::<Vec<_>>() })
    });
}

#[test]
fn oracle_flow_check_updates_reports_npm() {
    flow("flow-check-updates-reports-npm", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#""1.2.3""#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example".to_string())];
        let updates = s.pm().check_for_available_updates().unwrap();
        json!({ "value": updates.iter().map(package_update_json).collect::<Vec<_>>() })
    });
}

#[test]
fn oracle_flow_check_updates_newer_installed_skip() {
    flow("flow-check-updates-newer-installed-skip", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"2.0.0"}"#,
        );
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#""1.9.0""#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        s.settings.state.lock().unwrap().project.packages =
            vec![PackageSourceEntry::Plain("npm:example".to_string())];
        let updates = s.pm().check_for_available_updates().unwrap();
        json!({ "value": updates.iter().map(package_update_json).collect::<Vec<_>>() })
    });
}

#[test]
fn oracle_flow_check_updates_pinned_skip() {
    flow("flow-check-updates-pinned-skip", |s| {
        s.mkdir(".pi/npm/node_modules/example");
        s.write(
            ".pi/npm/node_modules/example/package.json",
            r#"{"name":"example","version":"1.0.0"}"#,
        );
        let manager = s.pm();
        let parsed = manager.parse_source("git:github.com/example/repo@v1");
        let super::ParsedSource::Git(git) = &parsed else {
            unreachable!()
        };
        let installed_git_path = manager
            .get_git_install_path(git, SourceScope::Project)
            .unwrap();
        std::fs::create_dir_all(installed_git_path).unwrap();
        s.settings.state.lock().unwrap().project.packages = vec![
            PackageSourceEntry::Plain("npm:example@1.0.0".to_string()),
            PackageSourceEntry::Plain("git:github.com/example/repo@v1".to_string()),
        ];
        let updates = manager.check_for_available_updates().unwrap();
        json!({ "value": updates.iter().map(package_update_json).collect::<Vec<_>>() })
    });
}

#[test]
fn oracle_flow_latest_npm_version() {
    flow("flow-latest-npm-version", |s| {
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#""1.2.3""#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        let latest = s.pm().get_latest_npm_version("example", None).unwrap();
        json!({ "value": latest })
    });
}

#[test]
fn oracle_flow_latest_npm_version_mise_argv() {
    flow("flow-latest-npm-version-mise-argv", |s| {
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#""1.2.3""#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@20".into(),
            "--".into(),
            "npm".into(),
        ]);
        let latest = s.pm().get_latest_npm_version("@scope/pkg", None).unwrap();
        json!({ "value": latest })
    });
}

#[test]
fn oracle_flow_latest_npm_version_array_max() {
    flow("flow-latest-npm-version-array-max", |s| {
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#"["1.0.0","1.2.0","0.9.0"]"#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        let manager = s.pm();
        let no_range = manager.get_latest_npm_version("example", None).unwrap();
        let range = super::vendor::parse_range("^1.0.0").unwrap();
        let with_range = manager
            .get_latest_npm_version("example", Some(&range))
            .unwrap();
        json!({ "noRange": no_range, "withRange": with_range })
    });
}

#[test]
fn oracle_flow_latest_npm_version_errors() {
    flow("flow-latest-npm-version-errors", |s| {
        let manager = s.pm();
        let mut out = Map::new();
        let record = |out: &mut Map<String, Value>, key: &str, result: PmResultString| match result
        {
            Ok(value) => out.insert(key.to_string(), json!(value)),
            Err(error) => out.insert(key.to_string(), json!({ "error": error })),
        };
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: String::new(),
                stderr: String::new(),
                code: 0,
            })
        }));
        record(
            &mut out,
            "empty",
            unwrap_message(manager.get_latest_npm_version("example", None)),
        );
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: String::new(),
                stderr: "registry down".into(),
                code: 1,
            })
        }));
        record(
            &mut out,
            "registryError",
            unwrap_message(manager.get_latest_npm_version("example", None)),
        );
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#"{"weird": 1}"#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        record(
            &mut out,
            "nonString",
            unwrap_message(manager.get_latest_npm_version("example", None)),
        );
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: r#"["", 42, "1.0.0"]"#.into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        record(
            &mut out,
            "mixedArray",
            unwrap_message(manager.get_latest_npm_version("example", None)),
        );
        Value::Object(out)
    });
}

type PmResultString = Result<String, String>;

fn unwrap_message(result: Result<String, super::PmError>) -> PmResultString {
    result.map_err(|error| error.message)
}

#[test]
fn oracle_flow_installed_path_legacy_roots() {
    flow("flow-installed-path-legacy-roots", |s| {
        let root20 = s.dir.join("node20").join("lib").join("node_modules");
        std::fs::create_dir_all(root20.join("@scope").join("pkg")).unwrap();
        let root22 = s.dir.join("node22").join("lib").join("node_modules");
        s.runner.script(Arc::new(move |command, args, _cwd| {
            if command != "mise" {
                return Err(format!("unexpected command {command}"));
            }
            if args[1] == "node@20" {
                return Ok(SpawnOutcome {
                    stdout: format!("{}\n", root20.to_string_lossy()),
                    stderr: String::new(),
                    code: 0,
                });
            }
            if args[1] == "node@22" {
                return Ok(SpawnOutcome {
                    stdout: format!("{}\n", root22.to_string_lossy()),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Err(format!("unexpected args {}", args.join(" ")))
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@20".into(),
            "--".into(),
            "npm".into(),
        ]);
        let manager = s.pm();
        let first = manager.get_installed_path("npm:@scope/pkg", SourceScope::User);
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@22".into(),
            "--".into(),
            "npm".into(),
        ]);
        let second = manager.get_installed_path("npm:@scope/pkg", SourceScope::User);
        let mut out = Map::new();
        out.insert(
            "first".into(),
            first.map_or(Value::Null, |path| {
                json!(path_relative(&s.dir, Path::new(&path)).replace('\\', "/"))
            }),
        );
        // JS drops undefined keys from the capture object.
        if let Some(path) = second {
            out.insert(
                "second".into(),
                json!(path_relative(&s.dir, Path::new(&path)).replace('\\', "/")),
            );
        }
        Value::Object(out)
    });
}

#[test]
fn oracle_flow_installed_path_pnpm_list_legacy() {
    flow("flow-installed-path-pnpm-list-legacy", |s| {
        let pnpm_root = s.dir.join("pnpm").join("global").join("v11");
        let package_path = pnpm_root
            .join("20-hash")
            .join("node_modules")
            .join("pnpm-pkg");
        std::fs::create_dir_all(package_path.join("extensions")).unwrap();
        std::fs::write(
            package_path.join("package.json"),
            r#"{"name":"pnpm-pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            package_path.join("extensions").join("index.ts"),
            format!("{JS};"),
        )
        .unwrap();
        let package_path_clone = package_path.clone();
        let pnpm_root_clone = pnpm_root.clone();
        s.runner.script(Arc::new(move |command, args, _cwd| {
            if command != "pnpm" {
                return Err(format!("unexpected command {command}"));
            }
            if args.join(" ") == "list -g --depth 0 --json" {
                let payload = json!([{
                    "path": pnpm_root_clone,
                    "dependencies": { "pnpm-pkg": { "version": "1.0.0", "path": package_path_clone } },
                }]);
                return Ok(SpawnOutcome {
                    stdout: payload.to_string(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Err(format!("unexpected args {}", args.join(" ")))
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        s.settings.state.lock().unwrap().global.packages =
            vec![PackageSourceEntry::Plain("npm:pnpm-pkg".to_string())];
        let manager = s.pm();
        let resolved = manager.resolve(None).unwrap();
        // JS spread the rendered object and merged installedPath at top level.
        let mut object = match render_resolved(&resolved, &s.dir) {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        if let Some(installed_path) = manager.get_installed_path("npm:pnpm-pkg", SourceScope::User)
        {
            object.insert(
                "installedPath".into(),
                json!(path_relative(&s.dir, Path::new(&installed_path)).replace('\\', "/")),
            );
        }
        Value::Object(object)
    });
}

#[test]
fn oracle_flow_installed_path_pnpm_list_wrapped() {
    flow("flow-installed-path-pnpm-list-wrapped", |s| {
        let pnpm_root = s.dir.join("pnpm").join("global").join("v11");
        let package_path = pnpm_root
            .join("20-hash")
            .join("node_modules")
            .join("pnpm-pkg");
        std::fs::create_dir_all(&package_path).unwrap();
        let package_path_clone = package_path.clone();
        let pnpm_root_clone = pnpm_root.clone();
        s.runner.script(Arc::new(move |command, args, _cwd| {
            if command != "mise" {
                return Err(format!("unexpected command {command}"));
            }
            if args.join(" ") == "exec node@20 -- pnpm list -g --depth 0 --json" {
                let payload = json!([{
                    "path": pnpm_root_clone,
                    "dependencies": { "pnpm-pkg": { "path": package_path_clone } },
                }]);
                return Ok(SpawnOutcome {
                    stdout: payload.to_string(),
                    stderr: String::new(),
                    code: 0,
                });
            }
            Err(format!("unexpected args {}", args.join(" ")))
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec![
            "mise".into(),
            "exec".into(),
            "node@20".into(),
            "--".into(),
            "pnpm".into(),
        ]);
        let manager = s.pm();
        let installed = manager.get_installed_path("npm:pnpm-pkg", SourceScope::User);
        let mut out = Map::new();
        if let Some(path) = installed {
            out.insert(
                "installedPath".into(),
                json!(path_relative(&s.dir, Path::new(&path)).replace('\\', "/")),
            );
        }
        Value::Object(out)
    });
}

#[test]
fn oracle_flow_installed_path_pnpm_list_malformed() {
    flow("flow-installed-path-pnpm-list-malformed", |s| {
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Ok(SpawnOutcome {
                stdout: "not json".into(),
                stderr: String::new(),
                code: 0,
            })
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        let manager = s.pm();
        let installed = manager.get_installed_path("npm:pnpm-pkg", SourceScope::User);
        let mut out = Map::new();
        if let Some(path) = installed {
            out.insert(
                "installedPath".into(),
                json!(path_relative(&s.dir, Path::new(&path)).replace('\\', "/")),
            );
        }
        Value::Object(out)
    });
}

#[test]
fn oracle_flow_managed_install_wins_no_reinstall() {
    flow("flow-managed-install-wins-no-reinstall", |s| {
        let package_path = s
            .agent_dir
            .join("npm")
            .join("node_modules")
            .join("pnpm-pkg");
        std::fs::create_dir_all(package_path.join("extensions")).unwrap();
        std::fs::write(
            package_path.join("package.json"),
            r#"{"name":"pnpm-pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            package_path.join("extensions").join("index.ts"),
            format!("{JS};"),
        )
        .unwrap();
        s.runner.script(Arc::new(|command, _args, _cwd| {
            if command == "pnpm" {
                return Ok(default_outcome());
            }
            Err(format!("Unexpected: {command}"))
        }));
        s.settings.state.lock().unwrap().global.npm_command = Some(vec!["pnpm".into()]);
        s.settings.state.lock().unwrap().global.packages =
            vec![PackageSourceEntry::Plain("npm:pnpm-pkg".to_string())];
        let manager = s.pm();
        manager.resolve(None).unwrap();
        let installs_after_first = s
            .runner
            .calls()
            .iter()
            .filter(|(command, args)| command == "pnpm" && args[0] == "install")
            .count();
        manager.resolve(None).unwrap();
        let installs_after_second = s
            .runner
            .calls()
            .iter()
            .filter(|(command, args)| command == "pnpm" && args[0] == "install")
            .count();
        json!({ "installsAfterFirst": installs_after_first, "installsAfterSecond": installs_after_second })
    });
}

#[test]
fn oracle_flow_progress_events_install_failure() {
    flow("flow-progress-events-install-failure", |s| {
        let manager = s.pm();
        let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        manager.set_progress_callback(Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone());
        })));
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Err("simulated npm install failure".to_string())
        }));
        let outcome = manager.install("npm:nonexistent-package@1.0.0", false);
        let rendered: Vec<Value> = events
            .lock()
            .unwrap()
            .iter()
            .map(|event| {
                json!({
                    "type": event.event_type,
                    "action": event.action,
                    "source": event.source,
                    "message": event.message,
                })
            })
            .collect();
        json!({
            "events": rendered,
            "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()),
        })
    });
}

#[test]
fn oracle_flow_progress_local_no_events() {
    flow("flow-progress-local-no-events", |s| {
        let manager = s.pm();
        let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        manager.set_progress_callback(Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone());
        })));
        s.write("ext.ts", JS);
        let source = s.dir.join("ext.ts").to_string_lossy().into_owned();
        manager
            .resolve_extension_sources(&[source], false, false)
            .unwrap();
        json!({ "events": events.lock().unwrap().len() })
    });
}

#[test]
fn oracle_flow_progress_github_clone_attempt() {
    flow("flow-progress-github-clone-attempt", |s| {
        let manager = s.pm();
        let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        manager.set_progress_callback(Some(Arc::new(move |event| {
            sink.lock().unwrap().push(event.clone());
        })));
        s.runner.script(Arc::new(|_command, _args, _cwd| {
            Err("simulated git clone failure".to_string())
        }));
        let outcome = manager.install("https://github.com/nonexistent/repo", false);
        let rendered: Vec<Value> = events
            .lock()
            .unwrap()
            .iter()
            .map(|event| {
                json!({
                    "type": event.event_type,
                    "action": event.action,
                    "source": event.source,
                    "message": event.message,
                })
            })
            .collect();
        json!({
            "events": rendered,
            "outcome": outcome.err().map(|error| error.message).unwrap_or_else(|| "resolved".to_string()),
        })
    });
}

// ===========================================================================
// Ported behavioral tests (upstream suite, non-scenario-shaped)
// ===========================================================================

/// Upstream `should use npmCommand argv for npm installs` — exact argv.
#[test]
fn npm_command_argv_is_used_for_installs() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let settings = TestSettings::new(json!({}));
    settings.state.lock().unwrap().global.npm_command = Some(vec![
        "mise".into(),
        "exec".into(),
        "node@20".into(),
        "--".into(),
        "npm".into(),
    ]);
    let runner = FakeRunner::new();
    let manager = pm_from(temp.path(), &agent_dir, &settings, &runner);
    manager.install("npm:@scope/pkg", false).unwrap();
    assert_eq!(
        runner.calls(),
        vec![(
            "mise".to_string(),
            vec![
                "exec".to_string(),
                "node@20".to_string(),
                "--".to_string(),
                "npm".to_string(),
                "install".to_string(),
                "@scope/pkg".to_string(),
                "--prefix".to_string(),
                agent_dir.join("npm").to_string_lossy().into_owned(),
                "--legacy-peer-deps".to_string(),
            ]
        )]
    );
}

/// Upstream `should preserve argv entries containing spaces` (seam level):
/// argv entries are passed verbatim, no shell splitting.
#[test]
fn runner_records_argv_entries_with_spaces() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    let runner = FakeRunner::new();
    let manager = pm_from(temp.path(), &temp.path().join("agent"), &settings, &runner);
    let value_with_space = r"C:\Users\A B\.pi\npm";
    let captured = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let sink = Arc::clone(&captured);
    runner.script(Arc::new(move |_command, args, _cwd| {
        sink.lock().unwrap().push(args.to_vec());
        Ok(SpawnOutcome {
            stdout: format!("{value_with_space}\n"),
            stderr: String::new(),
            code: 0,
        })
    }));
    let output = manager
        .run_command_sync(
            "node",
            &[
                "-e".to_string(),
                "console.log(process.argv[1])".to_string(),
                value_with_space.to_string(),
            ],
        )
        .unwrap();
    assert_eq!(output, value_with_space);
    assert_eq!(
        captured.lock().unwrap().last().unwrap()[2],
        value_with_space
    );
}

/// Real-process analogue of the upstream close-before-resolve semantics:
/// the sync/capture runners drain child pipes to EOF, so output written
/// after a delay is captured (divergence 4).
#[test]
fn real_runner_captures_delayed_stdout() {
    let (command, script) = if cfg!(windows) {
        (
            "cmd".to_string(),
            vec![
                "/c".to_string(),
                "echo start& ping -n 2 127.0.0.1 >nul& echo end".to_string(),
            ],
        )
    } else {
        (
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "echo start; sleep 1; echo end".to_string(),
            ],
        )
    };
    let runner = super::RealCommandRunner;
    let output = runner
        .run_capture(&command, &script, None, Some(15000), &[])
        .expect("capture failed");
    // cmd emits CRLF; the assertion cares about the delayed second line.
    assert_eq!(output.replace('\r', ""), "start\nend");
}

/// Upstream `should reconcile an existing git checkout to a pinned ref
/// during install` — exact argv sequence.
#[test]
fn git_install_reconciles_existing_checkout_to_pinned_ref() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agent");
    let target_dir = agent_dir
        .join("git")
        .join("github.com")
        .join("user")
        .join("repo");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(
        target_dir.join("package.json"),
        r#"{"name":"repo","version":"1.0.0"}"#,
    )
    .unwrap();
    let settings = TestSettings::new(json!({}));
    let runner = FakeRunner::new();
    runner.script(Arc::new(|_command, args, _cwd| {
        if args[0] == "rev-parse" {
            return match args[1].as_str() {
                "HEAD" => Ok(SpawnOutcome {
                    stdout: "old-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                }),
                "FETCH_HEAD^{commit}" => Ok(SpawnOutcome {
                    stdout: "new-head\n".into(),
                    stderr: String::new(),
                    code: 0,
                }),
                other => Err(format!(
                    "Unexpected runCommandCapture args: rev-parse {other}"
                )),
            };
        }
        // fetch / reset / clean and the npm dependency install run with
        // inherited stdio; success is all they need to contribute here.
        Ok(default_outcome())
    }));
    let manager = pm_from(temp.path(), &agent_dir, &settings, &runner);
    manager
        .install("git:github.com/user/repo@v2", false)
        .unwrap();
    let calls = runner.calls();
    assert!(calls.contains(&(
        "git".to_string(),
        vec!["fetch".to_string(), "origin".to_string(), "v2".to_string()]
    )));
    assert!(calls.contains(&(
        "git".to_string(),
        vec![
            "reset".to_string(),
            "--hard".to_string(),
            "FETCH_HEAD^{commit}".to_string()
        ]
    )));
    assert!(calls.contains(&(
        "git".to_string(),
        vec!["clean".to_string(), "-fdx".to_string()]
    )));
    assert!(calls.contains(&(
        "npm".to_string(),
        vec!["install".to_string(), "--omit=dev".to_string()]
    )));
}

/// Upstream `should batch npm updates per scope and run git updates in
/// parallel while skipping pinned npm and current packages` — concurrency
/// measurement (threads really overlap).
#[test]
fn update_runs_npm_batches_and_git_updates_concurrently() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    for name in ["user-old", "user-current", "user-unknown"] {
        let path = agent_dir.join("npm").join("node_modules").join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("package.json"),
            format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
        )
        .unwrap();
    }
    for name in ["project-old", "project-current"] {
        let path = temp
            .path()
            .join(".pi")
            .join("npm")
            .join("node_modules")
            .join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("package.json"),
            format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
        )
        .unwrap();
    }
    let settings = TestSettings::new(json!({}));
    {
        let mut state = settings.state.lock().unwrap();
        state.global.packages = [
            "npm:user-old",
            "npm:user-current",
            "npm:user-unknown",
            "npm:user-pinned@1.0.0",
            "git:github.com/example/user-repo-a",
            "git:github.com/example/user-repo-b",
            "git:github.com/example/user-repo-pinned@v1",
        ]
        .iter()
        .map(|source| PackageSourceEntry::Plain(source.to_string()))
        .collect();
        state.project.packages = [
            "npm:project-old",
            "npm:project-current",
            "npm:project-missing",
            "git:github.com/example/project-repo-a",
        ]
        .iter()
        .map(|source| PackageSourceEntry::Plain(source.to_string()))
        .collect();
    }
    let runner = FakeRunner::new();
    let active_npm = Arc::new(AtomicI64::new(0));
    let max_npm = Arc::new(AtomicI64::new(0));
    let _unused_max_npm_guard = ();
    let active_git = Arc::new(AtomicUsize::new(0));
    let max_git = Arc::new(AtomicUsize::new(0));
    let active_clone = Arc::new(AtomicI64::new(0));
    let max_clone = Arc::new(AtomicI64::new(0));
    let runner_for_script = Arc::clone(&runner);
    let active_npm_clone = Arc::clone(&active_npm);
    let max_npm_clone = Arc::clone(&max_npm);
    let active_git_clone = Arc::clone(&active_git);
    let max_git_clone = Arc::clone(&max_git);
    let active_clone_for_git = Arc::clone(&active_clone);
    let max_clone_for_script = Arc::clone(&max_clone);
    runner.script(Arc::new(move |command, args, _cwd| {
        if args[0] == "view" {
            return match args[1].as_str() {
                "user-old" | "project-old" => Ok(SpawnOutcome {
                    stdout: r#""2.0.0""#.into(),
                    stderr: String::new(),
                    code: 0,
                }),
                "user-current" | "project-current" => Ok(SpawnOutcome {
                    stdout: r#""1.0.0""#.into(),
                    stderr: String::new(),
                    code: 0,
                }),
                "user-unknown" => Err("registry unavailable".to_string()),
                other => Err(format!("Unexpected lookup: {other}")),
            };
        }
        if command == "npm" && args[0] == "install" {
            let current = active_npm_clone.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            max_npm_clone.fetch_max(current, AtomicOrdering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            active_npm_clone.fetch_sub(1, AtomicOrdering::SeqCst);
            return Ok(default_outcome());
        }
        if command == "git" && args[0] == "clone" {
            let current = active_clone_for_git.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            max_clone_for_script.fetch_max(current, AtomicOrdering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            active_clone_for_git.fetch_sub(1, AtomicOrdering::SeqCst);
            std::fs::create_dir_all(&args[2]).unwrap();
            return Ok(default_outcome());
        }
        if command == "git" {
            let current = active_git_clone.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            max_git_clone.fetch_max(current, AtomicOrdering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            active_git_clone.fetch_sub(1, AtomicOrdering::SeqCst);
            return Ok(default_outcome());
        }
        let _ = runner_for_script;
        Err(format!(
            "Unexpected spawn: {}",
            join_command_args(command, args)
        ))
    }));
    let manager = pm_from(temp.path(), &agent_dir, &settings, &runner);
    manager.update(None).unwrap();
    let npm_installs = runner
        .calls()
        .iter()
        .filter(|(command, args)| command == "npm" && args[0] == "install")
        .count();
    let git_clones = runner
        .calls()
        .iter()
        .filter(|(command, args)| command == "git" && args[0] == "clone")
        .count();
    assert_eq!(npm_installs, 2, "one batch install per scope");
    assert_eq!(
        git_clones, 4,
        "pinned git refs included as checkout targets"
    );
    assert!(
        max_npm.load(AtomicOrdering::SeqCst) > 1,
        "scope batches run concurrently"
    );
    assert!(
        max_clone.load(AtomicOrdering::SeqCst) > 1,
        "git updates run concurrently"
    );
    let _ = (active_npm, max_npm, active_git, max_git);
}

/// Upstream `should suggest npm source prefixes for update lookups`.
#[test]
fn update_error_suggests_configured_source() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    settings.state.lock().unwrap().project.packages =
        vec![PackageSourceEntry::Plain("npm:example".to_string())];
    let runner = FakeRunner::new();
    let manager = pm_from(temp.path(), &temp.path().join("agent"), &settings, &runner);
    let error = manager.update(Some("example")).unwrap_err();
    assert_eq!(
        error.message,
        "No matching package found for example. Did you mean npm:example?"
    );
}

/// Upstream `should not parse dot-relative paths as git` / docs examples.
#[test]
fn source_parsing_docs_examples() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(
        temp.path(),
        &temp.path().join("agent"),
        &settings,
        &FakeRunner::new(),
    );

    let parse_npm = |source: &str| match manager.parse_source(source) {
        super::ParsedSource::Npm(npm) => npm,
        other => panic!("Expected npm source: {source} ({other:?})"),
    };
    assert!(parse_npm("npm:@scope/pkg@1.2.3").pinned);
    assert!(!parse_npm("npm:@scope/pkg@^1.2.3").pinned);
    assert!(!parse_npm("npm:pkg").pinned);

    for source in [
        "git:github.com/user/repo@v1",
        "https://github.com/user/repo@v1",
        "git:git@github.com:user/repo@v1",
        "ssh://git@github.com/user/repo@v1",
    ] {
        assert!(
            matches!(manager.parse_source(source), super::ParsedSource::Git(_)),
            "{source}"
        );
    }
    for source in [
        "/absolute/path/to/package",
        "./relative/path/to/package",
        "../relative/path/to/package",
    ] {
        assert!(
            matches!(manager.parse_source(source), super::ParsedSource::Local(_)),
            "{source}"
        );
    }

    let dot_slash = manager.parse_source("./packages/agent-timers");
    match dot_slash {
        super::ParsedSource::Local(local) => assert_eq!(local.path, "./packages/agent-timers"),
        _ => panic!(),
    }
    let dot_dot_slash = manager.parse_source("../packages/agent-timers");
    match dot_dot_slash {
        super::ParsedSource::Local(local) => assert_eq!(local.path, "../packages/agent-timers"),
        _ => panic!(),
    }
}

/// Upstream `should reject paths outside git install roots` for all scopes
/// (assertion-level port; the byte oracle pins the message text).
#[test]
fn git_install_path_traversal_rejected_for_all_scopes() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(
        temp.path(),
        &temp.path().join("agent"),
        &settings,
        &FakeRunner::new(),
    );
    let git_source = crate::coding_agent::utils::git::GitSource {
        repo_type: "git",
        repo: "git@evil.example:../../victim/repo".to_string(),
        host: "evil.example".to_string(),
        path: "../../victim/repo".to_string(),
        ref_: None,
        pinned: false,
    };
    for scope in [
        SourceScope::User,
        SourceScope::Project,
        SourceScope::Temporary,
    ] {
        let error = manager
            .get_git_install_path(&git_source, scope)
            .unwrap_err();
        assert!(
            error.message.contains("outside package install root"),
            "{error:?}"
        );
    }
}

/// Upstream `should place temporary npm packages under the agent temp
/// extension folder`.
#[test]
fn temporary_npm_packages_live_under_agent_temp_folder() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let agent_dir = temp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(temp.path(), &agent_dir, &settings, &FakeRunner::new());
    let parsed = manager.parse_source("npm:left-pad");
    let super::ParsedSource::Npm(npm) = parsed else {
        unreachable!()
    };
    let install_path = manager
        .get_npm_install_path(&npm, SourceScope::Temporary)
        .unwrap();
    let temp_root = agent_dir.join("tmp").join("extensions");
    let normalized = install_path.replace('\\', "/");
    assert!(normalized.ends_with("node_modules/left-pad"));
    assert!(!path_relative(&temp_root, Path::new(&install_path)).starts_with(".."));
}

/// Upstream `should keep pi manifest entries with leading tilde package-relative`
/// plus `should handle directories with pi manifest` assertion-level ports.
#[test]
fn resolve_extension_sources_manifest_assertions() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let scenario = Scenario::new(temp.path(), "manifest-assertions");
    scenario.write(
        "my-package/package.json",
        r#"{"name":"my-package","pi":{"extensions":["./src/index.ts"],"skills":["./skills"]}}"#,
    );
    scenario.write("my-package/src/index.ts", JS);
    scenario.write("my-package/skills/my-skill/SKILL.md", &skill("my-skill"));
    let source = scenario
        .dir
        .join("my-package")
        .to_string_lossy()
        .into_owned();
    let resolved = scenario
        .pm()
        .resolve_extension_sources(std::slice::from_ref(&source), false, false)
        .unwrap();
    let index_ts = scenario
        .dir
        .join("my-package")
        .join("src")
        .join("index.ts")
        .to_string_lossy()
        .into_owned();
    let skill_md = scenario
        .dir
        .join("my-package")
        .join("skills")
        .join("my-skill")
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned();
    assert!(resolved
        .extensions
        .iter()
        .any(|entry| entry.path == index_ts && entry.enabled));
    assert!(resolved
        .skills
        .iter()
        .any(|entry| entry.path == skill_md && entry.enabled));
    assert!(!resolved
        .extensions
        .iter()
        .any(|entry| entry.path.ends_with("helper.ts")));

    // tilde entries
    scenario.write("tilde-package/~extensions/main.ts", JS);
    scenario.write("tilde-package/~/extensions/alt.ts", JS);
    scenario.write(
        "tilde-package/~skills/direct-skill/SKILL.md",
        &skill("direct-skill"),
    );
    scenario.write(
        "tilde-package/~/skills/slash-skill/SKILL.md",
        &skill("slash-skill"),
    );
    scenario.write(
        "tilde-package/package.json",
        r#"{"name":"tilde-package","pi":{"extensions":["~extensions/main.ts","~/extensions/alt.ts"],"skills":["~skills","~/skills"]}}"#,
    );
    let tilde_source = scenario
        .dir
        .join("tilde-package")
        .to_string_lossy()
        .into_owned();
    let resolved = scenario
        .pm()
        .resolve_extension_sources(&[tilde_source], false, false)
        .unwrap();
    for (rel_parts, is_extension) in [
        (vec!["tilde-package", "~extensions", "main.ts"], true),
        (vec!["tilde-package", "~", "extensions", "alt.ts"], true),
        (
            vec!["tilde-package", "~skills", "direct-skill", "SKILL.md"],
            false,
        ),
        (
            vec!["tilde-package", "~", "skills", "slash-skill", "SKILL.md"],
            false,
        ),
    ] {
        let mut path = scenario.dir.clone();
        for part in &rel_parts {
            path = path.join(part);
        }
        let list = if is_extension {
            &resolved.extensions
        } else {
            &resolved.skills
        };
        assert!(
            list.iter()
                .any(|entry| entry.path == path.to_string_lossy() && entry.enabled),
            "{}",
            path.to_string_lossy()
        );
    }
}

/// Upstream `should deduplicate git URLs with different supported formats`
/// assertion port (identity normalization).
#[test]
fn git_url_identities_are_normalized() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(
        temp.path(),
        &temp.path().join("agent"),
        &settings,
        &FakeRunner::new(),
    );
    let urls = [
        "https://github.com/user/repo",
        "https://github.com/user/repo.git",
        "ssh://git@github.com/user/repo",
        "git:https://github.com/user/repo",
        "git:github.com/user/repo",
        "git:git@github.com:user/repo",
        "git:git@github.com:user/repo.git",
    ];
    let identities: Vec<String> = urls
        .iter()
        .map(|url| manager.get_package_identity(url, None))
        .collect();
    assert!(identities
        .iter()
        .all(|identity| identity == "git:github.com/user/repo"));
    assert_ne!(
        manager.get_package_identity("https://github.com/user/repo1", None),
        manager.get_package_identity("git:git@github.com:user/repo2", None)
    );
}

/// Upstream ssh test file: protocol URLs without git: prefix + identity
/// normalization (`test/package-manager-ssh.test.ts`).
#[test]
fn ssh_source_parsing_suite() {
    let _guard = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let settings = TestSettings::new(json!({}));
    let manager = pm_from(
        temp.path(),
        &temp.path().join("agent"),
        &settings,
        &FakeRunner::new(),
    );

    let parsed = manager.parse_source("https://github.com/user/repo");
    let super::ParsedSource::Git(git) = parsed else {
        panic!("expected git")
    };
    assert_eq!(git.host, "github.com");
    assert_eq!(git.path, "user/repo");

    let parsed = manager.parse_source("ssh://git@github.com/user/repo");
    let super::ParsedSource::Git(git) = parsed else {
        panic!("expected git")
    };
    assert_eq!(git.host, "github.com");
    assert_eq!(git.path, "user/repo");
    assert_eq!(git.repo, "ssh://git@github.com/user/repo");

    let parsed = manager.parse_source("git:git@github.com:user/repo");
    let super::ParsedSource::Git(git) = parsed else {
        panic!("expected git")
    };
    assert_eq!(git.host, "github.com");
    assert_eq!(git.path, "user/repo");
    assert_eq!(git.repo, "git@github.com:user/repo");
    assert!(!git.pinned);

    let parsed = manager.parse_source("git:git@github.com:user/repo@v1.0.0");
    let super::ParsedSource::Git(git) = parsed else {
        panic!("expected git")
    };
    assert_eq!(git.ref_.as_deref(), Some("v1.0.0"));
    assert!(git.pinned);

    // Unsupported without git: prefix → local.
    for source in ["git@github.com:user/repo", "github.com/user/repo"] {
        assert!(
            matches!(manager.parse_source(source), super::ParsedSource::Local(_)),
            "{source}"
        );
    }

    let prefixed = manager.get_package_identity("git:git@github.com:user/repo", None);
    let https = manager.get_package_identity("https://github.com/user/repo", None);
    let ssh = manager.get_package_identity("ssh://git@github.com/user/repo", None);
    assert_eq!(prefixed, "git:github.com/user/repo");
    assert_eq!(prefixed, https);
    assert_eq!(prefixed, ssh);
}
