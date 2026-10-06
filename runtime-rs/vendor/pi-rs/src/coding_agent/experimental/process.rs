//! Port of upstream `experimental/process.ts`
//! (sha256 9f28b373093bb82fae889fda6fc2cb8c747a5112e3a27b2936cbd27c376a178c).
//!
//! Ported: `INTERNAL_PROCESS_ENV`, `InternalProcessRole`, role read/consume
//! validation, `MAX_CONTROL_LINE_BYTES`, `encodeControlLine` (byte oracle),
//! and the spawn/terminate surface over the D4 seam (see mod.rs docs).

use std::ffi::OsString;
use std::fmt;

use serde::Serialize;

/// Upstream `INTERNAL_PROCESS_ENV`.
pub const INTERNAL_PROCESS_ENV: &str = "__PI_INTERNAL_SPAWN";

/// Upstream `MAX_CONTROL_LINE_BYTES` (128 MiB).
pub const MAX_CONTROL_LINE_BYTES: u64 = 128 * 1024 * 1024;

/// Upstream `InternalProcessRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalProcessRole {
    Coordinator,
    Server,
    SessionWorker,
}

impl InternalProcessRole {
    pub fn as_str(self) -> &'static str {
        match self {
            InternalProcessRole::Coordinator => "coordinator",
            InternalProcessRole::Server => "server",
            InternalProcessRole::SessionWorker => "session-worker",
        }
    }

    fn from_str(role: &str) -> Option<Self> {
        match role {
            "coordinator" => Some(InternalProcessRole::Coordinator),
            "server" => Some(InternalProcessRole::Server),
            "session-worker" => Some(InternalProcessRole::SessionWorker),
            _ => None,
        }
    }
}

impl fmt::Display for InternalProcessRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream `getInternalProcessRole`, parameterized over the env lookup
/// (upstream reads `process.env` directly; tests pass a closure).
pub fn get_internal_process_role_from(
    lookup: impl FnOnce(&str) -> Option<OsString>,
) -> Result<Option<InternalProcessRole>, String> {
    let Some(raw) = lookup(INTERNAL_PROCESS_ENV) else {
        return Ok(None);
    };
    let role = raw.to_string_lossy();
    match InternalProcessRole::from_str(&role) {
        Some(parsed) => Ok(Some(parsed)),
        None => Err(format!("Unsupported internal process role: {role}")),
    }
}

/// Upstream `getInternalProcessRole` against the real process environment.
pub fn get_internal_process_role() -> Result<Option<InternalProcessRole>, String> {
    get_internal_process_role_from(|key| std::env::var_os(key))
}

/// Upstream `consumeInternalProcessRole`: read, validate, then remove the
/// variable so descendants do not inherit it.
pub fn consume_internal_process_role() -> Result<Option<InternalProcessRole>, String> {
    let role = get_internal_process_role()?;
    std::env::remove_var(INTERNAL_PROCESS_ENV);
    Ok(role)
}

/// Upstream entrypoint guards (`coordinator.ts` / `session-worker.ts` direct
/// entry blocks): consume the internal role and require the exact match.
/// Upstream error strings are preserved byte-for-byte (both the absent and
/// mismatched cases report the expected role).
pub fn require_internal_process_role(
    expected: InternalProcessRole,
) -> Result<InternalProcessRole, String> {
    let role = consume_internal_process_role()?;
    match role {
        Some(role) if role == expected => Ok(role),
        _ => Err(format!(
            "{} entrypoint requires an internal {} invocation",
            expected.display_name(),
            expected.as_str()
        )),
    }
}

impl InternalProcessRole {
    /// Upstream display name used in the entrypoint guard errors.
    pub fn display_name(self) -> &'static str {
        match self {
            InternalProcessRole::Coordinator => "Coordinator",
            InternalProcessRole::Server => "Server",
            InternalProcessRole::SessionWorker => "Session worker",
        }
    }
}

/// Upstream `encodeControlLine`: a single JSON line terminated by `\n`,
/// rejecting lines beyond `MAX_CONTROL_LINE_BYTES` with the exact upstream
/// error text.
pub fn encode_control_line(message: &impl Serialize) -> Result<String, String> {
    encode_control_line_with_limit(message, MAX_CONTROL_LINE_BYTES)
}

/// Limit-parameterized core of [`encode_control_line`]. The production
/// wrapper pins the upstream 128 MiB constant; the parameterization exists so
/// the overflow path can be tested without allocating 128 MiB in CI.
pub fn encode_control_line_with_limit(
    message: &impl Serialize,
    limit: u64,
) -> Result<String, String> {
    let line = match serde_json::to_string(message) {
        Ok(body) => format!("{body}\n"),
        Err(error) => return Err(error.to_string()),
    };
    if line.len() as u64 > limit {
        return Err("Internal control message is too large".to_string());
    }
    Ok(line)
}

/// D4 seam: a spawned internal-process child handle (upstream `ChildProcess`).
pub trait InternalProcessChild: Send + Sync {
    fn pid(&self) -> Option<u32>;
    /// Upstream `child.kill("SIGKILL")`.
    fn kill(&self);
    /// Upstream `child.exitCode !== null || child.signalCode !== null`.
    fn has_exited(&self) -> bool;
    /// Resolves once the child can no longer take ownership (upstream awaits
    /// the `exit`/`error` events after SIGKILL).
    fn wait_exit(&self) -> futures::future::BoxFuture<'_, ()>;
}

/// D4 seam: spawn side of upstream `spawnInternalProcess`'s Node `spawn` call.
pub trait ProcessSpawner: Send + Sync {
    fn spawn(
        &self,
        role: InternalProcessRole,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> std::io::Result<Box<dyn InternalProcessChild>>;
}

/// Real-process implementation of [`ProcessSpawner`]: spawns the current
/// executable detached with stdio ignored, mirroring the upstream spawn
/// options (`detached: true`, `stdio: "ignore"`, `windowsHide: true`).
///
/// Platform face of the upstream spawn options (both are safe std APIs):
/// - `detached: true` becomes `process_group(0)` on Unix and
///   `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows (node's
///   `spawn` adds exactly these for a detached child);
/// - `windowsHide: true` becomes `CREATE_NO_WINDOW` on Windows.
///
/// The default entry-selection logic (dist bundles versus source entrypoints)
/// is Node-runtime-specific and intentionally reduced to the role-prefixed
/// argument convention of this binary (`<role> <args...>`); the role still
/// travels in `__PI_INTERNAL_SPAWN` exactly as upstream. Upstream's
/// `child.unref()` needs no equivalent: the handle is not awaited unless the
/// embedder asks for [`terminate_internal_process`].
pub struct StdProcessSpawner;

impl StdProcessSpawner {
    /// D4 test/embedder face: spawn an explicit executable instead of the
    /// current binary (upstream picks a dist bundle or source entrypoint;
    /// tests pin a benign stub executable).
    pub fn with_exe(exe: std::path::PathBuf) -> impl ProcessSpawner {
        ExeOverrideSpawner { exe }
    }
}

struct ExeOverrideSpawner {
    exe: std::path::PathBuf,
}

impl ProcessSpawner for ExeOverrideSpawner {
    fn spawn(
        &self,
        role: InternalProcessRole,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> std::io::Result<Box<dyn InternalProcessChild>> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(&self.exe);
        command
            .arg(role.as_str())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_detached_spawn_options(&mut command);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.env(INTERNAL_PROCESS_ENV, role.as_str());
        let child = command.spawn()?;
        let pid = child.id();
        Ok(Box::new(StdInternalProcessChild {
            pid: Some(pid),
            child: std::sync::Mutex::new(child),
        }))
    }
}

impl ProcessSpawner for StdProcessSpawner {
    fn spawn(
        &self,
        role: InternalProcessRole,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> std::io::Result<Box<dyn InternalProcessChild>> {
        use std::process::{Command, Stdio};
        let exe = std::env::current_exe()?;
        let mut command = Command::new(exe);
        command
            .arg(role.as_str())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_detached_spawn_options(&mut command);
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.env(INTERNAL_PROCESS_ENV, role.as_str());
        let child = command.spawn()?;
        let pid = child.id();
        Ok(Box::new(StdInternalProcessChild {
            pid: Some(pid),
            child: std::sync::Mutex::new(child),
        }))
    }
}

/// Upstream spawn options shared by both real spawner impls: a detached child
/// (`detached: true`) that never surfaces a console window
/// (`windowsHide: true`). See the [`StdProcessSpawner`] docs for the exact
/// flag mapping.
fn apply_detached_spawn_options(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = command;
    }
}

struct StdInternalProcessChild {
    pid: Option<u32>,
    child: std::sync::Mutex<std::process::Child>,
}

impl InternalProcessChild for StdInternalProcessChild {
    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn kill(&self) {
        // Upstream sends SIGKILL; `Child::kill` is the closest portable
        // equivalent on this seam's default implementation.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    fn has_exited(&self) -> bool {
        match self.child.lock() {
            Ok(mut child) => matches!(child.try_wait(), Ok(Some(_))),
            Err(_) => false,
        }
    }

    fn wait_exit(&self) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(async move {
            loop {
                if self.has_exited() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
    }
}

/// Port of upstream `terminateInternalProcess`: force the child to exit and
/// wait until it can no longer take ownership. Skips when there is no pid or
/// the child already exited.
pub async fn terminate_internal_process(child: &dyn InternalProcessChild) {
    if child.pid().is_none() || child.has_exited() {
        return;
    }
    child.kill();
    child.wait_exit().await;
}

#[cfg(test)]
mod tests;
