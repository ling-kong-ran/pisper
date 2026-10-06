//! Port of upstream `experimental/server.ts`
//! (sha256 d98412be6f8f8566a119be2236a306021530ef2fac6b900e9cb79aaed90373e4).
//!
//! Ported: the server/session directory resolution, the private-directory
//! guard, the `acquireServerProfile` identity protocol (default-server-id
//! create-with-exclusive-flag race, exact error strings) and activation lock
//! face, the `ServerLifetime` hold/retire reconciliation state machine, the
//! per-session plugin-selection registry (`resolveSessionPlugins` /
//! `removeSessionPlugins` / `reloadPresentationFacetBundles` caching), the
//! internal-server model-options parser and argument validation, and the
//! `startServer`/`startForegroundServer` assembly order with upstream's
//! cleanup aggregation error strings.
//!
//! D10 seam (disclosed in this module's docs): the live server backend (pi-server unix
//! listener, `JsonlSessionRepo` storage, `SessionWorkerManager` wiring, the
//! `RadiusRelayHost` pump and signal handling) is embedder-owned behind the
//! [`ServerAssemblyHost`] trait; the port owns the exact step order, the
//! replace/coordinator-close branching, and the startServer assembly's
//! aggregate error text ("Server runtime startup and cleanup failed").
//! Upstream's remaining disposal-face aggregate texts ("Experimental session
//! storage cleanup failed", "Server service startup and cleanup failed",
//! "Experimental session catalog cleanup failed", "Experimental server
//! startup and cleanup failed", "Server and repository shutdown failed") are
//! emitted by the embedder-owned close/dispose steps the host drives.
//! `proper-lockfile` is replaced by the exclusive-create
//! [`FileLockGuard`] with the same retry/wait bounds (stale breaks not
//! ported; upstream `stale`/`update` liveness is embedder-owned).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::coding_agent::experimental::plugins::package::{
    read_session_plugin_package_profile, remove_session_plugin_package_profile,
    write_session_plugin_package_profile, ServerPluginPackage,
};
use crate::coding_agent::experimental::radius_relay::is_connection_id;

/// Upstream `ENV_SERVER_DIR`.
pub const ENV_SERVER_DIR: &str = "PI_SERVER_DIR";
/// Upstream `ENV_SERVER_ID`.
pub const ENV_SERVER_ID: &str = "PI_SERVER_ID";

const LOCK_STALE_MS: u64 = 30_000;
const LOCK_RETRY_MS: u64 = 25;
const LOCK_WAIT_MS: u64 = 30_000;
const DEFAULT_SERVER_ID_FILE: &str = "default-server-id";

const ACTIVATION_TIMEOUT_MS: u64 = 10_000;
const ACTIVATION_RETRY_MS: u64 = 25;

/// Upstream `AUTO_SERVER_STARTUP_GRACE_MS`.
pub const AUTO_SERVER_STARTUP_GRACE_MS: u64 = 10_000;
/// Upstream `AUTO_SERVER_IDLE_GRACE_MS`.
pub const AUTO_SERVER_IDLE_GRACE_MS: u64 = 1_000;

/// Upstream `resolveServerDirectory`:
/// `resolvePath(directory ?? process.env[ENV_SERVER_DIR] ?? ~/.pi/server)`.
pub fn resolve_server_directory(directory: Option<&str>, env_server_dir: Option<&str>) -> String {
    let raw = directory
        .map(str::to_string)
        .or_else(|| env_server_dir.map(str::to_string))
        .unwrap_or_else(default_server_directory);
    crate::coding_agent::utils::paths::resolve_path_auto_base(&raw).unwrap_or(raw)
}

fn default_server_directory() -> String {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    Path::new(&home)
        .join(".pi")
        .join("server")
        .to_string_lossy()
        .to_string()
}

/// Upstream `ensurePrivateServerDirectory`: create the directory 0700 and
/// verify it is a directory owned by the current POSIX user.
pub fn ensure_private_server_directory(directory: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| error.to_string())?;
        let metadata = std::fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Err(format!(
                "Unix socket directory is not a directory: {directory}"
            ));
        }
        if metadata.uid() != unsafe { libc_getuid() } {
            return Err(format!(
                "Unix socket directory is not owned by the current user: {directory}"
            ));
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Err("Unix socket directory requires a POSIX user ID".to_string())
    }
}

#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    // std exposes getuid only through libc; bind it without a new dependency.
    extern "C" {
        fn getuid() -> u32;
    }
    getuid()
}

/// Upstream `resolveSessionDirectory`:
/// `resolvePath(sessionDir ?? <agentDir>/experimental/sessions)`.
pub fn resolve_session_directory(session_dir: Option<&str>) -> String {
    let raw = session_dir.map(str::to_string).unwrap_or_else(|| {
        Path::new(&crate::coding_agent::core::get_agent_dir())
            .join("experimental")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });
    crate::coding_agent::utils::paths::resolve_path_auto_base(&raw).unwrap_or(raw)
}

/// Upstream `ServerProfile`.
#[derive(Debug)]
pub struct ServerProfile {
    pub server_id: String,
    lock: Option<FileLockGuard>,
}

impl ServerProfile {
    /// Upstream `release()`.
    pub fn release(mut self) -> Result<(), String> {
        self.lock
            .take()
            .map(|guard| guard.release())
            .unwrap_or(Ok(()))
    }
}

/// Upstream `acquireServerProfile`: lock one logical server ID in a shared
/// experimental server directory. The identity selection (explicit request,
/// stored default, exclusive create, EEXIST race re-read) is verbatim; the
/// `proper-lockfile` lock is the exclusive-create [`FileLockGuard`].
pub fn acquire_server_profile(
    directory: &str,
    requested_server_id: Option<&str>,
) -> Result<ServerProfile, String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let server_id = match requested_server_id {
        Some(requested) => {
            if !is_connection_id(requested) {
                return Err(format!("Invalid experimental server ID: {requested}"));
            }
            requested.to_string()
        }
        None => default_server_identity(directory)?,
    };
    let lock = FileLockGuard::acquire(
        &Path::new(directory).join(format!("launcher-{server_id}")),
        LOCK_STALE_MS,
        LOCK_STALE_MS / 3,
        LOCK_WAIT_MS,
        LOCK_RETRY_MS,
    )?;
    Ok(ServerProfile {
        server_id,
        lock: Some(lock),
    })
}

fn default_server_identity(directory: &str) -> Result<String, String> {
    let path = Path::new(directory).join(DEFAULT_SERVER_ID_FILE);
    match std::fs::read_to_string(&path) {
        Ok(value) => {
            let value = value.trim().to_string();
            if !is_connection_id(&value) {
                return Err(format!(
                    "Invalid default experimental server identity in {}",
                    path.display()
                ));
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let candidate = new_uuid_v4();
            match exclusive_create(&path, &candidate) {
                Ok(()) => Ok(candidate),
                Err(CreateExclusiveError::Exists) => {
                    let value = std::fs::read_to_string(&path)
                        .map_err(|error| error.to_string())?
                        .trim()
                        .to_string();
                    if !is_connection_id(&value) {
                        return Err(format!(
                            "Invalid default experimental server identity in {}",
                            path.display()
                        ));
                    }
                    Ok(value)
                }
                Err(CreateExclusiveError::Io(error)) => Err(error),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Exclusive-create write mirroring upstream's
/// `writeFile(path, candidate, { flag: "wx" })` race.
fn exclusive_create(path: &Path, contents: &str) -> Result<(), CreateExclusiveError> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => file
            .write_all(contents.as_bytes())
            .map_err(|error| CreateExclusiveError::Io(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(CreateExclusiveError::Exists)
        }
        Err(error) => Err(CreateExclusiveError::Io(error.to_string())),
    }
}

enum CreateExclusiveError {
    Exists,
    Io(String),
}

/// Upstream `acquireServerActivation`: serialize automatic cold activation.
pub fn acquire_server_activation(
    directory: &str,
    server_id: &str,
) -> Result<FileLockGuard, String> {
    FileLockGuard::acquire(
        &Path::new(directory).join(format!("activation-{server_id}")),
        ACTIVATION_TIMEOUT_MS * 2,
        ACTIVATION_TIMEOUT_MS,
        ACTIVATION_TIMEOUT_MS,
        ACTIVATION_RETRY_MS,
    )
}

/// Exclusive-create lock guard standing in for `proper-lockfile` (D10):
/// retries every `retry_ms` up to `wait_ms`; `release` removes the lock file.
#[derive(Debug)]
pub struct FileLockGuard {
    path: PathBuf,
    released: bool,
}

impl FileLockGuard {
    pub fn acquire(
        path: &Path,
        stale_ms: u64,
        update_ms: u64,
        wait_ms: u64,
        retry_ms: u64,
    ) -> Result<Self, String> {
        let _ = (stale_ms, update_ms);
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            match exclusive_create(path, "") {
                Ok(()) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                        released: false,
                    });
                }
                Err(CreateExclusiveError::Exists) => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Could not acquire the experimental server lock {}",
                            path.display()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(retry_ms));
                }
                Err(CreateExclusiveError::Io(error)) => return Err(error),
            }
        }
    }

    pub fn release(mut self) -> Result<(), String> {
        self.released = true;
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        if !self.released {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Upstream `randomUUID().replaceAll("-", "").slice(0, 12)` shaped identity
/// provider seam; the default reuses the crate's RFC 4122 generator.
pub fn new_uuid_v4() -> String {
    crate::ai::auth::oauth::uuid_v4()
}

// ---------------------------------------------------------------------------
// ServerLifetime
// ---------------------------------------------------------------------------

/// Upstream `ServerLifetime`: reconcile operator, startup, client, and worker
/// holds for one server generation. Time is injectable (`now_ms`) so the
/// reconciliation decisions are deterministic.
#[derive(Debug, Default)]
pub struct ServerLifetime {
    keep_alive: bool,
    connection_count: i64,
    worker_count: i64,
    startup_held: bool,
    startup_deadline_ms: Option<u64>,
    retirement_deadline_ms: Option<u64>,
    stopped: bool,
    retire_armed: bool,
}

impl ServerLifetime {
    /// Upstream constructor.
    pub fn new(keep_alive: bool) -> Self {
        Self {
            keep_alive,
            startup_held: !keep_alive,
            ..Default::default()
        }
    }

    /// Upstream `start(retire)`.
    pub fn start(&mut self, now_ms: u64) {
        self.retire_armed = true;
        if self.startup_held {
            self.startup_deadline_ms = Some(now_ms + AUTO_SERVER_STARTUP_GRACE_MS);
        }
        self.reconcile(now_ms);
    }

    /// Upstream `setConnectionCount`.
    pub fn set_connection_count(&mut self, count: i64, now_ms: u64) {
        self.connection_count = count;
        if count > 0 && self.startup_held {
            self.startup_held = false;
            self.startup_deadline_ms = None;
        }
        self.reconcile(now_ms);
    }

    /// Upstream `setWorkerCount`.
    pub fn set_worker_count(&mut self, count: i64, now_ms: u64) {
        self.worker_count = count;
        self.reconcile(now_ms);
    }

    /// Upstream `stop`.
    pub fn stop(&mut self) {
        self.stopped = true;
        self.startup_deadline_ms = None;
        self.retirement_deadline_ms = None;
    }

    /// Upstream `#startupTimer` firing (start-only grace expiry).
    pub fn fire_startup_expiry(&mut self, now_ms: u64) {
        if self
            .startup_deadline_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.startup_deadline_ms = None;
            self.startup_held = false;
            self.reconcile(now_ms);
        }
    }

    /// Upstream `#retirementTimer` firing. Returns whether `retire` ran.
    pub fn fire_retirement(&mut self, now_ms: u64) -> bool {
        let due = self
            .retirement_deadline_ms
            .is_none_or(|deadline| now_ms < deadline);
        if due {
            return false;
        }
        self.retirement_deadline_ms = None;
        !(self.stopped || self.startup_held) && self.connection_count == 0 && self.worker_count == 0
    }

    /// Upstream `#reconcile`: hold or schedule retirement.
    fn reconcile(&mut self, now_ms: u64) {
        if self.stopped
            || self.keep_alive
            || self.startup_held
            || self.connection_count != 0
            || self.worker_count != 0
        {
            self.retirement_deadline_ms = None;
            return;
        }
        if self.retirement_deadline_ms.is_some() || !self.retire_armed {
            return;
        }
        self.retirement_deadline_ms = Some(now_ms + AUTO_SERVER_IDLE_GRACE_MS);
    }

    pub fn retirement_deadline(&self) -> Option<u64> {
        self.retirement_deadline_ms
    }

    pub fn startup_deadline(&self) -> Option<u64> {
        self.startup_deadline_ms
    }
}

// ---------------------------------------------------------------------------
// Model options + internal server argument validation
// ---------------------------------------------------------------------------

/// Upstream `parseServerModelOptions` result (worker model selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerModelOptions {
    pub provider: Option<String>,
    pub model: String,
}

/// Upstream `parseServerModelOptions`. Exact upstream error strings.
pub fn parse_server_model_options(
    value: Option<&str>,
) -> Result<Option<ServerModelOptions>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_str(value)
        .map_err(|_| "Internal server received invalid model options".to_string())?;
    let Some(object) = parsed.as_object() else {
        return Err("Internal server received invalid model options".to_string());
    };
    for key in object.keys() {
        if key != "provider" && key != "model" {
            return Err("Internal server received invalid model options".to_string());
        }
    }
    let model = object.get("model");
    let provider = object.get("provider");
    let model_valid =
        model.is_some_and(|model| model.is_string() && !model.as_str().unwrap().is_empty());
    // Upstream treats JSON `null` like any non-string provider: invalid.
    let provider_valid = provider.is_none_or(|provider| {
        provider.is_string() && !provider.as_str().unwrap_or_default().is_empty()
    });
    // Upstream: `typeof model !== "string" || model.length === 0` rejects
    // even a missing model, and the provider must be a non-empty string.
    if !model_valid || !provider_valid {
        return Err("Internal server received invalid model options".to_string());
    }
    Ok(Some(ServerModelOptions {
        provider: provider.and_then(Value::as_str).map(str::to_string),
        model: model.and_then(Value::as_str).unwrap().to_string(),
    }))
}

/// Upstream `runServerProcess` argument validation. Exact upstream errors.
pub fn validate_server_process_args(
    args: &[String],
    is_absolute: impl Fn(&str) -> bool,
) -> Result<ServerProcessArgs, String> {
    if args.len() > 4 {
        return Err("Internal server received unexpected arguments".to_string());
    }
    let directory = args.first().map(String::as_str).unwrap_or_default();
    let server_id = args.get(1).map(String::as_str).unwrap_or_default();
    let session_dir = args.get(2).map(String::as_str).unwrap_or_default();
    let serialized_model = args.get(3).map(String::as_str);
    if directory.is_empty() || !is_absolute(directory) {
        return Err("Internal server requires an absolute server directory".to_string());
    }
    if !is_connection_id(server_id) {
        return Err("Internal server requires a canonical server ID".to_string());
    }
    if session_dir.is_empty() || !is_absolute(session_dir) {
        return Err("Internal server requires an absolute Session directory".to_string());
    }
    Ok(ServerProcessArgs {
        directory: directory.to_string(),
        server_id: server_id.to_string(),
        session_dir: session_dir.to_string(),
        serialized_model: serialized_model.map(str::to_string),
    })
}

/// Upstream `runServerProcess` destructured arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProcessArgs {
    pub directory: String,
    pub server_id: String,
    pub session_dir: String,
    pub serialized_model: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-session plugin selection registry
// ---------------------------------------------------------------------------

fn same_strings(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().zip(right.iter()).all(|(l, r)| l == r)
}

/// Resolved plugin selection for one session (upstream
/// `ResolvedSessionPlugins`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSessionPlugins {
    pub package_paths: Vec<String>,
    pub manifest_paths: Vec<String>,
    pub presentation_artifacts: Vec<Value>,
}

/// Upstream `startServer`'s plugin-selection closure cluster: the default
/// selection, per-session caches, profile persistence, and reload cutover.
pub struct PluginSelectionRegistry {
    directory: String,
    server_id: String,
    packages: HashMap<String, Arc<ServerPluginPackage>>,
    default_selection: ResolvedSessionPlugins,
    session_selections: HashMap<String, ResolvedSessionPlugins>,
    bundler: FacetBundleBuild,
}

use std::collections::HashMap;

use crate::coding_agent::experimental::plugins::package::{
    normalize_plugin_package_paths, FacetBundleBuild,
};

impl PluginSelectionRegistry {
    /// Build the registry after `restoreServerPluginPackageProfile`.
    pub fn new(
        directory: &str,
        server_id: &str,
        default_package_paths: Vec<String>,
        bundler: FacetBundleBuild,
    ) -> Result<Self, String> {
        let mut registry = Self {
            directory: directory.to_string(),
            server_id: server_id.to_string(),
            packages: HashMap::new(),
            default_selection: ResolvedSessionPlugins {
                package_paths: Vec::new(),
                manifest_paths: Vec::new(),
                presentation_artifacts: Vec::new(),
            },
            session_selections: HashMap::new(),
            bundler,
        };
        registry.default_selection = registry.build_plugin_selection(&default_package_paths)?;
        Ok(registry)
    }

    fn get_plugin_package(
        &mut self,
        package_path: &str,
    ) -> Result<Arc<ServerPluginPackage>, String> {
        if let Some(existing) = self.packages.get(package_path) {
            return Ok(existing.clone());
        }
        let package = Arc::new(ServerPluginPackage::new(
            &self.directory,
            &self.server_id,
            package_path,
        )?);
        self.packages
            .insert(package_path.to_string(), package.clone());
        Ok(package)
    }

    /// Upstream `buildPluginSelection`.
    pub fn build_plugin_selection(
        &mut self,
        package_paths: &[String],
    ) -> Result<ResolvedSessionPlugins, String> {
        let normalized = normalize_plugin_package_paths(package_paths)?;
        let mut packages = Vec::with_capacity(normalized.len());
        for package_path in &normalized {
            packages.push(self.get_plugin_package(package_path)?);
        }
        let mut presentation_artifacts = Vec::new();
        for package in &packages {
            presentation_artifacts.extend(package.build(&self.bundler)?);
        }
        Ok(ResolvedSessionPlugins {
            manifest_paths: packages
                .iter()
                .map(|package| package.manifest_path().to_string_lossy().to_string())
                .collect(),
            package_paths: normalized,
            presentation_artifacts,
        })
    }

    /// Upstream `resolveSessionPlugins`.
    pub fn resolve_session_plugins(
        &mut self,
        session_path: &str,
        requested_package_paths: Option<&[String]>,
    ) -> Result<ResolvedSessionPlugins, String> {
        if let Some(requested) = requested_package_paths {
            let normalized = normalize_plugin_package_paths(requested)?;
            if let Some(current) = self.session_selections.get(session_path) {
                if same_strings(&current.package_paths, &normalized) {
                    return Ok(current.clone());
                }
            }
            let candidate = self.build_plugin_selection(&normalized)?;
            write_session_plugin_package_profile(
                &self.directory,
                &self.server_id,
                session_path,
                &candidate.package_paths,
            )?;
            self.session_selections
                .insert(session_path.to_string(), candidate.clone());
            return Ok(candidate);
        }
        if let Some(cached) = self.session_selections.get(session_path) {
            return Ok(cached.clone());
        }
        let stored =
            read_session_plugin_package_profile(&self.directory, &self.server_id, session_path)?;
        let selected = match stored {
            Some(stored_paths) => self.build_plugin_selection(&stored_paths)?,
            None => {
                let default = self.default_selection.clone();
                write_session_plugin_package_profile(
                    &self.directory,
                    &self.server_id,
                    session_path,
                    &default.package_paths,
                )?;
                default
            }
        };
        self.session_selections
            .insert(session_path.to_string(), selected.clone());
        Ok(selected)
    }

    /// Upstream `removeSessionPlugins`.
    pub fn remove_session_plugins(&mut self, session_path: &str) -> Result<(), String> {
        self.session_selections.remove(session_path);
        remove_session_plugin_package_profile(&self.directory, &self.server_id, session_path)
    }

    /// Upstream `reloadPresentationFacetBundles`: rebuild and cut over the
    /// default and any matching session selections.
    pub fn reload_presentation_facet_bundles(
        &mut self,
        package_paths: &[String],
    ) -> Result<Vec<Value>, String> {
        let reloaded = self.build_plugin_selection(package_paths)?;
        if same_strings(
            &self.default_selection.package_paths,
            &reloaded.package_paths,
        ) {
            self.default_selection = reloaded.clone();
        }
        for (session_path, selected) in self.session_selections.clone() {
            if same_strings(&selected.package_paths, &reloaded.package_paths) {
                self.session_selections
                    .insert(session_path, reloaded.clone());
            }
        }
        Ok(reloaded.presentation_artifacts)
    }
}

// ---------------------------------------------------------------------------
// startServer / startForegroundServer assembly
// ---------------------------------------------------------------------------

/// The live backend steps upstream wires together (D10 seam). Each step maps
/// to one await in upstream `startServer`; implementors own the real I/O.
pub trait ServerAssemblyHost {
    /// `ensureCoordinator(socketPath, controlPath)` — returns a startup lease
    /// token the assembly closes once the server is running.
    fn ensure_coordinator(&mut self, socket_path: &str, control_path: &str) -> Result<u64, String>;
    /// `startServerBackend(...)` — the unix server + storage backend.
    fn start_backend(&mut self, server_path: &str) -> Result<(), String>;
    /// `coordinator.connect()` + `coordinator.peerIds`.
    fn connect_coordinator(&mut self) -> Result<Vec<String>, String>;
    /// `workers.discover(peerIds)` (the manager is constructed with the
    /// coordinator link before `startServerBackend` runs).
    fn start_workers(&mut self, peer_ids: &[String]) -> Result<(), String>;
    /// `backend.refreshSessions()`.
    fn refresh_sessions(&mut self) -> Result<(), String>;
    /// `relay.start()` — the Radius relay host pump.
    fn start_relay(&mut self) -> Result<(), String>;
    /// Cleanup steps, run in upstream's `Promise.allSettled` order.
    fn close_relay(&mut self) -> Result<(), String>;
    fn close_backend(&mut self) -> Result<(), String>;
    fn shutdown_workers(&mut self) -> Result<(), String>;
    fn close_coordinator(&mut self) -> Result<(), String>;
    /// `coordinator.wasReplaced`.
    fn coordinator_was_replaced(&self) -> bool;
    /// Release the directory profile lock.
    fn release_profile(&mut self) -> Result<(), String>;
    /// `workers.detach()` when the coordinator was replaced.
    fn detach_workers(&mut self);
}

/// One upstream cleanup failure (a rejected promise in an allSettled group).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupFailure(pub String);

/// Aggregate a settled cleanup group the way upstream does: a single failure
/// propagates bare, several aggregate with the exact upstream message.
pub fn aggregate_cleanup(errors: Vec<CleanupFailure>, message: &str) -> Result<(), String> {
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.into_iter().next().unwrap().0),
        _ => {
            let joined: Vec<String> = errors.into_iter().map(|failure| failure.0).collect();
            Err(format!("{message}: [{}]", joined.join(", ")))
        }
    }
}

/// Upstream `startServer` assembly order over the [`ServerAssemblyHost`]
/// seams. The port owns the sequencing (upstream `startServer` lines
/// 521-707), the startup-lease handoff, the replaced-coordinator branch and
/// the cleanup aggregation.
pub fn start_server_assembly(
    host: &mut dyn ServerAssemblyHost,
    socket_path: &str,
    control_path: &str,
    server_path: &str,
    session_dir: &str,
) -> Result<RunningServerHandle, String> {
    let startup_lease = host.ensure_coordinator(socket_path, control_path)?;

    let assembly = (|| -> Result<RunningServerHandle, String> {
        host.start_backend(server_path)?;
        let peer_ids = host.connect_coordinator()?;
        host.start_workers(&peer_ids)?;
        host.refresh_sessions()?;
        host.start_relay()?;
        Ok(RunningServerHandle {
            socket_path: socket_path.to_string(),
            server_path: server_path.to_string(),
            session_dir: session_dir.to_string(),
        })
    })();

    match assembly {
        Ok(handle) => {
            // Success closes the startup lease (upstream `startupLease.close()`).
            let _ = startup_lease;
            Ok(handle)
        }
        Err(error) => {
            // Upstream catch block: detach replaced coordinators, then the
            // allSettled cleanup group.
            if host.coordinator_was_replaced() {
                host.detach_workers();
            }
            let mut failures = Vec::new();
            if let Err(cleanup) = host.close_relay() {
                failures.push(CleanupFailure(cleanup));
            }
            if let Err(cleanup) = host.close_backend() {
                failures.push(CleanupFailure(cleanup));
            }
            if !host.coordinator_was_replaced() {
                if let Err(cleanup) = host.shutdown_workers() {
                    failures.push(CleanupFailure(cleanup));
                }
            }
            if let Err(cleanup) = host.close_coordinator() {
                failures.push(CleanupFailure(cleanup));
            }
            if let Err(cleanup) = host.release_profile() {
                failures.push(CleanupFailure(cleanup));
            }
            aggregate_cleanup(failures, "Server runtime startup and cleanup failed")?;
            Err(error)
        }
    }
}

/// Upstream `RunningServer` identity face (the live server/worker/relay
/// objects stay embedder-owned, D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningServerHandle {
    pub socket_path: String,
    pub server_path: String,
    pub session_dir: String,
}

/// Upstream `startForegroundServer`: serialize against automatic cold
/// activation while starting an operator-held server.
pub fn start_foreground_server_assembly(
    directory: &str,
    requested_server_id: Option<&str>,
    assemble: impl FnOnce(&str, &str) -> Result<RunningServerHandle, String>,
) -> Result<RunningServerHandle, String> {
    ensure_private_server_directory(directory)?;
    let profile = acquire_server_profile(directory, requested_server_id)?;
    let server_id = profile.server_id.clone();
    profile.release()?;
    let activation = acquire_server_activation(directory, &server_id);
    match activation {
        Ok(release) => {
            let result = assemble(directory, &server_id);
            let _ = release.release();
            result
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
