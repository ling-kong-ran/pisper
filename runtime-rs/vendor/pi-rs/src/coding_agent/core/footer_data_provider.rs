//! Port of upstream `coding-agent/src/core/footer-data-provider.ts`.
//!
//! Provides git branch and extension statuses for the status footer, with
//! debounced async refreshes driven by watchers on the git metadata files.
//!
//! Disclosed substitutions (node runtime → std/tokio, no new dependencies):
//! - `child_process.execFile/spawnSync` git calls keep their exact argv
//!   (`git --no-optional-locks symbolic-ref --quiet --short HEAD`, cwd =
//!   repoDir); tests inject a resolver seam ([`FooterDataProvider::with_branch_resolvers`])
//!   the way the upstream suite mocks `child_process`.
//! - Watchers build on the utils polling `fs_watch` port (upstream
//!   `fs.watch`): the HEAD *directory* watcher surfaces git's atomic
//!   HEAD replacement only when the directory metadata changes, so the
//!   upstream `!filename || filename === "HEAD"` filter is subsumed by
//!   unconditional scheduling (the polling watcher has no per-entry names
//!   for directory watches; already a disclosed fs_watch divergence).
//! - `watchFile(path, { interval })` polls map to the shared polling
//!   interval; the 500ms debounce and 5s watcher-retry delays use tokio
//!   timers, so tests reproduce the upstream fake-timer scenarios with
//!   tokio's paused clock.
//! - The upstream tests' private-field pokes (`reftableWatcher.emit`,
//!   `headWatcher` inspection) map to `#[cfg(test)]` seams on the provider.
//!
//! Behaviors carried over exactly: HEAD parsing (`ref: refs/heads/…`, the
//? `.invalid` reftable fallback to git, `detached`), caching per cwd,
//! debounce/in-flight/pending refresh choreography, branch-change
//! subscriptions, WSL `/mnt/<drive>` polling heuristic, and watcher retry.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::coding_agent::utils::fs_watch::{
    close_watcher, watch_with_error_handler, FsWatcher, FS_WATCH_RETRY_DELAY_MS,
};

/// Upstream `GitPaths`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPaths {
    pub repo_dir: String,
    pub common_git_dir: String,
    pub head_path: String,
}

fn path_join(base: &str, segment: &str) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&[base, segment])
    } else {
        crate::coding_agent::utils::node_path::posix_join(&[base, segment])
    }
}

fn path_dirname(path: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    match path.rfind(separators) {
        Some(index) if index > 0 => path[..index].to_string(),
        Some(0) => path[..1].to_string(),
        _ => {
            // Windows drive roots like "C:" keep their dirname "." like node.
            ".".to_string()
        }
    }
}

fn path_resolve(path: &str, base: &str) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_resolve(&[path], base)
    } else {
        crate::coding_agent::utils::node_path::posix_resolve(&[path], base)
    }
}

/// Find git metadata paths by walking up from cwd. Handles both regular git
/// repos (`.git` is a directory) and worktrees (`.git` is a file).
pub fn find_git_paths(cwd: &str) -> Option<GitPaths> {
    let mut dir = cwd.to_string();
    loop {
        let git_path = path_join(&dir, ".git");
        if std::path::Path::new(&git_path).exists() {
            let Ok(stat) = std::fs::metadata(&git_path) else {
                return None;
            };
            if stat.is_file() {
                let Ok(content) = std::fs::read_to_string(&git_path) else {
                    return None;
                };
                let content = content.trim();
                if let Some(gitdir_ref) = content.strip_prefix("gitdir: ") {
                    let git_dir = path_resolve(gitdir_ref.trim(), &dir);
                    let head_path = path_join(&git_dir, "HEAD");
                    if !std::path::Path::new(&head_path).exists() {
                        return None;
                    }
                    let common_dir_path = path_join(&git_dir, "commondir");
                    let common_git_dir = if std::path::Path::new(&common_dir_path).exists() {
                        let Ok(commondir) = std::fs::read_to_string(&common_dir_path) else {
                            return None;
                        };
                        path_resolve(commondir.trim(), &git_dir)
                    } else {
                        git_dir
                    };
                    return Some(GitPaths {
                        repo_dir: dir,
                        common_git_dir,
                        head_path,
                    });
                }
            } else if stat.is_dir() {
                let head_path = path_join(&git_path, "HEAD");
                if !std::path::Path::new(&head_path).exists() {
                    return None;
                }
                return Some(GitPaths {
                    repo_dir: dir,
                    common_git_dir: git_path,
                    head_path,
                });
            }
        }
        let parent = path_dirname(&dir);
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// Ask git for the current branch synchronously. Returns None on detached
/// HEAD or if git is unavailable.
pub fn resolve_branch_with_git_sync(repo_dir: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let branch = if output.status.code() == Some(0) {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        String::new()
    };
    (!branch.is_empty()).then_some(branch)
}

/// Ask git for the current branch asynchronously. Returns None on detached
/// HEAD or if git is unavailable.
pub async fn resolve_branch_with_git_async(repo_dir: &str) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let branch = if output.status.code() == Some(0) {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        String::new()
    };
    (!branch.is_empty()).then_some(branch)
}

fn is_wsl_environment() -> bool {
    std::env::consts::OS == "linux"
        && (non_empty_env("WSL_DISTRO_NAME") || non_empty_env("WSL_INTEROP"))
}

fn non_empty_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

fn is_windows_mounted_repo_path(repo_dir: &str) -> bool {
    // /^\/mnt\/[a-z](?:\/|$)/i
    let Some(rest) = repo_dir.strip_prefix("/mnt/") else {
        return false;
    };
    let Some(drive) = rest.chars().next() else {
        return false;
    };
    if !drive.is_ascii_alphabetic() {
        return false;
    }
    match rest.as_bytes().get(1) {
        None => true,
        Some(b'/') => true,
        Some(_) => false,
    }
}

fn should_poll_git_head(repo_dir: &str) -> bool {
    is_wsl_environment() && is_windows_mounted_repo_path(repo_dir)
}

type SyncBranchResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type AsyncBranchResolver = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;
type BranchChangeCallback = Box<dyn FnMut() + Send>;

fn default_sync_resolver() -> SyncBranchResolver {
    Arc::new(resolve_branch_with_git_sync)
}

fn default_async_resolver() -> AsyncBranchResolver {
    Arc::new(move |repo_dir: String| {
        Box::pin(async move { resolve_branch_with_git_async(&repo_dir).await })
    })
}

struct Inner {
    cwd: Mutex<String>,
    extension_statuses: Mutex<BTreeMap<String, String>>,
    cached_branch: Mutex<Option<Option<String>>>,
    git_paths: Mutex<Option<GitPaths>>,
    branch_change_callbacks: Mutex<Vec<(u64, BranchChangeCallback)>>,
    next_callback_id: AtomicUsize,
    available_provider_count: AtomicUsize,
    refresh_timer_active: AtomicBool,
    git_watcher_retry_active: AtomicBool,
    refresh_in_flight: AtomicBool,
    refresh_pending: AtomicBool,
    disposed: AtomicBool,
    watchers: Mutex<Vec<FsWatcher>>,
    runtime: Mutex<Option<Handle>>,
    sync_resolver: Mutex<SyncBranchResolver>,
    async_resolver: Mutex<AsyncBranchResolver>,
    /// Test-only completion signal standing in for the upstream fake timers:
    /// awaited instead of `tokio::time::advance` (explicit advance does not
    /// drive this runtime's timers; auto-advance does).
    #[cfg(test)]
    refresh_completed: tokio::sync::Notify,
    /// Test-only signal re-fired after the watcher set is (re)installed.
    #[cfg(test)]
    watchers_reinstalled: tokio::sync::Notify,
}

/// Provides git branch and extension statuses — data not otherwise accessible
/// to extensions (upstream `FooterDataProvider`).
pub struct FooterDataProvider {
    inner: Arc<Inner>,
}

/// Upstream `ReadonlyFooterDataProvider`: the read-only subset for
/// extensions.
pub struct ReadonlyFooterDataProvider {
    inner: Arc<Inner>,
}

impl ReadonlyFooterDataProvider {
    /// Current git branch, None if not in repo, Some("detached") if detached
    /// HEAD.
    pub fn get_git_branch(&self) -> Option<String> {
        let mut cached = self
            .inner
            .cached_branch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cached
            .get_or_insert_with(|| resolve_sync(&self.inner))
            .clone()
    }

    /// Extension status texts set via ctx.ui.setStatus().
    pub fn get_extension_statuses(&self) -> BTreeMap<String, String> {
        self.inner
            .extension_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Number of unique providers with available models (for footer display).
    pub fn get_available_provider_count(&self) -> usize {
        self.inner.available_provider_count.load(Ordering::SeqCst)
    }

    /// Subscribe to git branch changes; the returned handle unsubscribes.
    pub fn on_branch_change(&self, callback: Box<dyn FnMut() + Send>) -> BranchChangeUnsubscribe {
        subscribe(&self.inner, callback)
    }
}

impl Clone for ReadonlyFooterDataProvider {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// The upstream unsubscribe closure returned by `onBranchChange`.
pub struct BranchChangeUnsubscribe {
    inner: Arc<Inner>,
    id: u64,
}

impl BranchChangeUnsubscribe {
    pub fn unsubscribe(self) {
        let mut callbacks = self
            .inner
            .branch_change_callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        callbacks.retain(|(existing_id, _)| *existing_id != self.id);
    }
}

fn resolve_sync(inner: &Arc<Inner>) -> Option<String> {
    let git_paths = inner
        .git_paths
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let git_paths = git_paths?;
    match std::fs::read_to_string(&git_paths.head_path) {
        Ok(content) => {
            let content = content.trim();
            if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
                if branch == ".invalid" {
                    let resolver = inner
                        .sync_resolver
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    return Some(
                        resolver(&git_paths.repo_dir).unwrap_or_else(|| "detached".to_string()),
                    );
                }
                return Some(branch.to_string());
            }
            Some("detached".to_string())
        }
        Err(_) => None,
    }
}

async fn resolve_async(inner: &Arc<Inner>) -> Option<String> {
    let git_paths = inner
        .git_paths
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let git_paths = git_paths?;
    let content = std::fs::read_to_string(&git_paths.head_path).ok()?;
    let content = content.trim();
    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        if branch == ".invalid" {
            // Clone the resolver out of the lock before awaiting (Send).
            let resolver: AsyncBranchResolver = {
                let guard = inner
                    .async_resolver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Arc::clone(&guard)
            };
            let repo_dir = git_paths.repo_dir.clone();
            let branch = resolver(repo_dir).await;
            return Some(branch.unwrap_or_else(|| "detached".to_string()));
        }
        return Some(branch.to_string());
    }
    Some("detached".to_string())
}

fn subscribe(inner: &Arc<Inner>, callback: Box<dyn FnMut() + Send>) -> BranchChangeUnsubscribe {
    let id = inner.next_callback_id.fetch_add(1, Ordering::SeqCst) as u64;
    inner
        .branch_change_callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((id, callback));
    BranchChangeUnsubscribe {
        inner: Arc::clone(inner),
        id,
    }
}

/// Upstream `FooterDataProvider.WATCH_DEBOUNCE_MS`.
const WATCH_DEBOUNCE_MS: u64 = 500;

impl FooterDataProvider {
    /// Upstream `new FooterDataProvider(cwd)`. Must be constructed where a
    /// tokio reactor is available for the debounced refresh timers (a handle
    /// is captured at construction; without one, timers fall back to a
    /// dedicated thread).
    pub fn new(cwd: &str) -> Self {
        let git_paths = find_git_paths(cwd);
        let inner = Arc::new(Inner {
            cwd: Mutex::new(cwd.to_string()),
            extension_statuses: Mutex::new(BTreeMap::new()),
            cached_branch: Mutex::new(None),
            git_paths: Mutex::new(git_paths),
            branch_change_callbacks: Mutex::new(Vec::new()),
            next_callback_id: AtomicUsize::new(0),
            available_provider_count: AtomicUsize::new(0),
            refresh_timer_active: AtomicBool::new(false),
            git_watcher_retry_active: AtomicBool::new(false),
            refresh_in_flight: AtomicBool::new(false),
            refresh_pending: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
            watchers: Mutex::new(Vec::new()),
            runtime: Mutex::new(Handle::try_current().ok()),
            sync_resolver: Mutex::new(default_sync_resolver()),
            async_resolver: Mutex::new(default_async_resolver()),
            #[cfg(test)]
            refresh_completed: tokio::sync::Notify::new(),
            #[cfg(test)]
            watchers_reinstalled: tokio::sync::Notify::new(),
        });
        let provider = Self { inner };
        provider.setup_git_watcher();
        provider
    }

    /// Test seam standing in for the upstream suite's `child_process` mock:
    /// replace both branch resolvers.
    #[cfg(test)]
    pub fn with_branch_resolvers(
        self,
        sync_resolver: SyncBranchResolver,
        async_resolver: AsyncBranchResolver,
    ) -> Self {
        *self
            .inner
            .sync_resolver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = sync_resolver;
        *self
            .inner
            .async_resolver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = async_resolver;
        self
    }

    /// Current git branch, None if not in repo, Some("detached") if detached
    /// HEAD.
    pub fn get_git_branch(&self) -> Option<String> {
        let mut cached = self
            .inner
            .cached_branch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cached
            .get_or_insert_with(|| resolve_sync(&self.inner))
            .clone()
    }

    /// Extension status texts set via ctx.ui.setStatus().
    pub fn get_extension_statuses(&self) -> BTreeMap<String, String> {
        self.inner
            .extension_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Subscribe to git branch changes. Returns the unsubscribe handle.
    pub fn on_branch_change(&self, callback: Box<dyn FnMut() + Send>) -> BranchChangeUnsubscribe {
        subscribe(&self.inner, callback)
    }

    /// Internal: set extension status (`ctx.ui.setStatus`).
    pub fn set_extension_status(&self, key: &str, text: Option<&str>) {
        let mut statuses = self
            .inner
            .extension_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match text {
            None => {
                statuses.remove(key);
            }
            Some(text) => {
                statuses.insert(key.to_string(), text.to_string());
            }
        }
    }

    /// Internal: clear extension statuses.
    pub fn clear_extension_statuses(&self) {
        self.inner
            .extension_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Number of unique providers with available models (for footer display).
    pub fn get_available_provider_count(&self) -> usize {
        self.inner.available_provider_count.load(Ordering::SeqCst)
    }

    /// Internal: update available provider count.
    pub fn set_available_provider_count(&self, count: usize) {
        self.inner
            .available_provider_count
            .store(count, Ordering::SeqCst);
    }

    pub fn set_cwd(&self, cwd: &str) {
        {
            let mut current = self
                .inner
                .cwd
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *current == cwd {
                return;
            }
            *current = cwd.to_string();
        }

        self.inner
            .refresh_timer_active
            .store(false, Ordering::SeqCst);
        self.clear_git_watchers();
        *self
            .inner
            .cached_branch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .inner
            .git_paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = find_git_paths(cwd);
        self.setup_git_watcher();
        self.notify_branch_change();
    }

    /// Internal: cleanup.
    pub fn dispose(&self) {
        self.inner.disposed.store(true, Ordering::SeqCst);
        self.inner
            .refresh_timer_active
            .store(false, Ordering::SeqCst);
        self.clear_git_watchers();
        self.inner
            .branch_change_callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// The read-only view for extensions.
    pub fn readonly(&self) -> ReadonlyFooterDataProvider {
        ReadonlyFooterDataProvider {
            inner: Arc::clone(&self.inner),
        }
    }

    fn notify_branch_change(&self) {
        let mut callbacks = self
            .inner
            .branch_change_callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (_, callback) in callbacks.iter_mut() {
            callback();
        }
    }

    fn schedule_refresh(&self) {
        if self.inner.disposed.load(Ordering::SeqCst)
            || self.inner.refresh_timer_active.load(Ordering::SeqCst)
        {
            return;
        }
        if self.inner.refresh_in_flight.load(Ordering::SeqCst) {
            self.inner.refresh_pending.store(true, Ordering::SeqCst);
            return;
        }
        self.inner
            .refresh_timer_active
            .store(true, Ordering::SeqCst);
        let inner = Arc::clone(&self.inner);
        let run = move || {
            inner.refresh_timer_active.store(false, Ordering::SeqCst);
            let provider = FooterDataProvider { inner };
            tokio::spawn(provider.refresh_git_branch_async());
        };
        self.spawn_timer(WATCH_DEBOUNCE_MS, run);
    }

    /// Run `run` after `delay_ms`, preferring the captured tokio runtime and
    /// falling back to a plain thread when no reactor was captured.
    fn spawn_timer(&self, delay_ms: u64, run: impl FnOnce() + Send + 'static) {
        let handle = self
            .inner
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match handle {
            Some(handle) => {
                handle.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    run();
                });
            }
            None => {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(delay_ms));
                    run();
                });
            }
        }
    }

    async fn refresh_git_branch_async(self) {
        if self.inner.disposed.load(Ordering::SeqCst) {
            return;
        }
        if self.inner.refresh_in_flight.load(Ordering::SeqCst) {
            self.inner.refresh_pending.store(true, Ordering::SeqCst);
            return;
        }

        // The upstream body is `try { … } finally { inFlight = false; chain
        // pending refresh }` — every path below falls through to the shared
        // finally logic at the end.
        self.inner.refresh_in_flight.store(true, Ordering::SeqCst);
        let next_branch = resolve_async(&self.inner).await;
        if !self.inner.disposed.load(Ordering::SeqCst) {
            let mut cached = self
                .inner
                .cached_branch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cached.is_some() && *cached != Some(next_branch.clone()) {
                *cached = Some(next_branch);
                drop(cached);
                self.notify_branch_change();
            } else {
                *cached = Some(next_branch);
            }
        }
        self.inner.refresh_in_flight.store(false, Ordering::SeqCst);
        #[cfg(test)]
        self.inner.refresh_completed.notify_waiters();
        if self.inner.refresh_pending.load(Ordering::SeqCst)
            && !self.inner.disposed.load(Ordering::SeqCst)
        {
            self.inner.refresh_pending.store(false, Ordering::SeqCst);
            self.schedule_refresh();
        }
    }

    fn clear_git_watchers(&self) {
        let mut watchers = self
            .inner
            .watchers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for watcher in watchers.drain(..) {
            close_watcher(Some(watcher));
        }
        self.inner
            .git_watcher_retry_active
            .store(false, Ordering::SeqCst);
    }

    fn schedule_git_watcher_retry(&self) {
        if self.inner.disposed.load(Ordering::SeqCst)
            || self.inner.git_watcher_retry_active.load(Ordering::SeqCst)
        {
            return;
        }
        self.inner
            .git_watcher_retry_active
            .store(true, Ordering::SeqCst);
        let inner = Arc::clone(&self.inner);
        self.spawn_timer(FS_WATCH_RETRY_DELAY_MS, move || {
            // A cleared/disposed timer must not re-install (upstream clears
            // the setTimeout in clearGitWatchers/dispose; the flag stands in
            // for that cancellation).
            if inner.disposed.load(Ordering::SeqCst)
                || !inner.git_watcher_retry_active.load(Ordering::SeqCst)
            {
                return;
            }
            inner
                .git_watcher_retry_active
                .store(false, Ordering::SeqCst);
            let provider = FooterDataProvider { inner };
            provider.setup_git_watcher();
        });
    }

    fn handle_git_watcher_error(&self) {
        self.clear_git_watchers();
        self.schedule_git_watcher_retry();
    }

    fn setup_git_watcher(&self) {
        self.clear_git_watchers();
        let git_paths = self
            .inner
            .git_paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(git_paths) = git_paths else {
            return;
        };

        let poll_git_head = should_poll_git_head(&git_paths.repo_dir);

        // Watch the directory containing HEAD, not HEAD itself: git uses
        // atomic writes (write temp, rename over HEAD), which changes the
        // inode. (The polling port watches directory metadata; see the module
        // docs for the filename-filter disclosure.)
        {
            let head_dir = path_dirname(&git_paths.head_path);
            let inner_for_events = Arc::clone(&self.inner);
            let inner_for_errors = Arc::clone(&self.inner);
            let listener = move |_event: &str, _filename: Option<&str>| {
                let provider = FooterDataProvider {
                    inner: Arc::clone(&inner_for_events),
                };
                provider.schedule_refresh();
            };
            let on_error = move || {
                let provider = FooterDataProvider {
                    inner: Arc::clone(&inner_for_errors),
                };
                provider.handle_git_watcher_error();
            };
            let watcher =
                watch_with_error_handler(&head_dir, Box::new(listener), &mut { on_error });
            if let Some(watcher) = watcher {
                self.inner
                    .watchers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(watcher);
            }
        }
        if poll_git_head {
            // Upstream `watchFile(headPath, { interval: 1000 })`: the polling
            // watcher on the file stands in for stat-interval polling.
            let inner_for_events = Arc::clone(&self.inner);
            let listener = move |_event: &str, _filename: Option<&str>| {
                let provider = FooterDataProvider {
                    inner: Arc::clone(&inner_for_events),
                };
                provider.schedule_refresh();
            };
            if let Some(watcher) =
                watch_with_error_handler(&git_paths.head_path, Box::new(listener), &mut || {})
            {
                self.inner
                    .watchers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(watcher);
            }
        } else if self
            .inner
            .watchers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return;
        }

        // In reftable repos, branch switches update files in the reftable
        // directory instead of HEAD. Watch it (and its tables.list) separately
        // so the footer picks up those changes.
        let reftable_dir = path_join(&git_paths.common_git_dir, "reftable");
        if std::path::Path::new(&reftable_dir).exists() {
            let inner_for_events = Arc::clone(&self.inner);
            let inner_for_errors = Arc::clone(&self.inner);
            let listener = move |_event: &str, _filename: Option<&str>| {
                let provider = FooterDataProvider {
                    inner: Arc::clone(&inner_for_events),
                };
                provider.schedule_refresh();
            };
            let on_error = move || {
                let provider = FooterDataProvider {
                    inner: Arc::clone(&inner_for_errors),
                };
                provider.handle_git_watcher_error();
            };
            let Some(reftable_watcher) =
                watch_with_error_handler(&reftable_dir, Box::new(listener), &mut { on_error })
            else {
                return;
            };
            self.inner
                .watchers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(reftable_watcher);

            let tables_list_path = path_join(&reftable_dir, "tables.list");
            if std::path::Path::new(&tables_list_path).exists() {
                let inner_for_events = Arc::clone(&self.inner);
                let inner_for_errors = Arc::clone(&self.inner);
                let listener = move |_event: &str, _filename: Option<&str>| {
                    let provider = FooterDataProvider {
                        inner: Arc::clone(&inner_for_events),
                    };
                    provider.schedule_refresh();
                };
                let on_error = move || {
                    let provider = FooterDataProvider {
                        inner: Arc::clone(&inner_for_errors),
                    };
                    provider.handle_git_watcher_error();
                };
                let Some(tables_watcher) =
                    watch_with_error_handler(&tables_list_path, Box::new(listener), &mut {
                        on_error
                    })
                else {
                    return;
                };
                self.inner
                    .watchers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(tables_watcher);
                // Upstream also `watchFile(tablesListPath, { interval: 250 })`;
                // the file watcher above already polls that file.
            }
        }
        #[cfg(test)]
        self.inner.watchers_reinstalled.notify_waiters();
    }

    // -- test seams (upstream pokes private FSWatcher fields directly) --

    /// Upstream `reftableWatcher.emit("change", …)` → scheduleRefresh().
    #[cfg(test)]
    pub fn inject_reftable_change(&self) {
        self.schedule_refresh();
    }

    /// Test-only: resolves when the current refresh pipeline settles.
    #[cfg(test)]
    pub fn refresh_completed(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.inner.refresh_completed.notified()
    }

    /// Test-only: resolves when the watcher set is (re)installed.
    #[cfg(test)]
    pub fn watchers_reinstalled(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.inner.watchers_reinstalled.notified()
    }

    /// Whether the HEAD directory watcher is installed.
    #[cfg(test)]
    pub fn head_watcher_is_active(&self) -> bool {
        !self
            .inner
            .watchers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Upstream `headWatcher.emit("error", …)` → handleGitWatcherError().
    #[cfg(test)]
    pub fn simulate_head_watcher_error(&self) {
        self.handle_git_watcher_error();
    }

    /// Test helper: clear the watcher-retry timer flag (paused-clock tests
    /// advance exactly across the delay).
    #[cfg(test)]
    pub fn watcher_retry_pending(&self) -> bool {
        self.inner.git_watcher_retry_active.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "footer_data_provider_tests.rs"]
mod tests;
