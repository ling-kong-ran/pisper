//! 文件工具以工作区重叠范围串行捕获；失败或取消执行也归档已经产生的文件。
use super::store::AssetStore;
use anyhow::{bail, Context, Result};
use futures::future::BoxFuture;
use pi_rust::{agent_core::types::AgentTool, coding_agent::agent_session::AgentSession};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
struct FileVersion {
    path: PathBuf,
    size: u64,
    modified: f64,
    version: String,
}
struct Baseline {
    root: PathBuf,
    files: BTreeMap<PathBuf, FileVersion>,
    exclude: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
struct PendingFile {
    #[serde(flatten)]
    file: FileVersion,
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: PathBuf,
}
#[derive(Default)]
struct Locks {
    next: u64,
    held: HashMap<u64, Vec<String>>,
}
pub(crate) struct WorkspaceLease {
    tracker: Arc<WorkspaceAssetTracker>,
    id: u64,
}
impl Drop for WorkspaceLease {
    fn drop(&mut self) {
        if let Ok(mut locks) = self.tracker.locks.lock() {
            locks.held.remove(&self.id);
        }
        self.tracker.notify.notify_waiters();
    }
}
enum Before {
    Shell(Option<Baseline>),
    File(PathBuf, Option<FileVersion>),
}
pub struct CaptureTicket {
    tracker: Arc<WorkspaceAssetTracker>,
    _scope: WorkspaceLease,
    before: Before,
    session_id: String,
    cwd: PathBuf,
}
pub struct WorkspaceAssetTracker {
    data_dir: PathBuf,
    path: PathBuf,
    store: Arc<Mutex<AssetStore>>,
    retries: Mutex<Value>,
    locks: Mutex<Locks>,
    notify: Notify,
    active: Mutex<HashMap<(String, String), CancellationToken>>,
    active_notify: Notify,
}
pub type AssetEventSink = Arc<dyn Fn(String, Vec<Value>) -> BoxFuture<'static, ()> + Send + Sync>;
struct ActiveCall {
    tracker: Arc<WorkspaceAssetTracker>,
    key: (String, String),
}
impl Drop for ActiveCall {
    fn drop(&mut self) {
        if let Ok(mut active) = self.tracker.active.lock() {
            active.remove(&self.key);
        }
        self.tracker.active_notify.notify_waiters();
    }
}
impl WorkspaceAssetTracker {
    pub fn new(agent_dir: &Path, store: Arc<Mutex<AssetStore>>) -> Result<Arc<Self>> {
        let data_dir = resolve_real_path(agent_dir)?;
        let path = agent_dir.join("pisper-workspace-asset-retries.json");
        let mut retries = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .context("Workspace retry index is invalid JSON")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"files":[]}),
            Err(error) => return Err(error.into()),
        };
        if !retries.is_object() {
            bail!("Workspace retry index is invalid");
        }
        if retries.get("files").is_none() {
            retries["files"] = json!([]);
        }
        if !retries["files"].is_array() {
            bail!("Workspace retry file list is invalid");
        }
        Ok(Arc::new(Self {
            data_dir,
            path,
            store,
            retries: Mutex::new(retries),
            locks: Mutex::new(Locks::default()),
            notify: Notify::new(),
            active: Mutex::new(HashMap::new()),
            active_notify: Notify::new(),
        }))
    }
    pub fn cancel_session(&self, session_id: &str) {
        if let Ok(active) = self.active.lock() {
            for ((session, _), token) in active.iter() {
                if session == session_id {
                    token.cancel();
                }
            }
        }
    }
    pub async fn wait_session(&self, session_id: &str) {
        loop {
            let notified = self.active_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .active
                .lock()
                .map(|active| !active.keys().any(|(session, _)| session == session_id))
                .unwrap_or(true)
            {
                break;
            }
            notified.await;
        }
    }
    pub async fn flush(
        self: &Arc<Self>,
        session_id: &str,
        session_name: &str,
    ) -> Result<Vec<Value>> {
        self.wait_session(session_id).await;
        self.drain(session_id, session_name).await
    }
    pub async fn begin(
        self: &Arc<Self>,
        session_id: &str,
        cwd: &Path,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<Option<CaptureTicket>> {
        let Some(operation) = operation(name, args) else {
            return Ok(None);
        };
        let cwd = resolve_real_path(cwd)?;
        let path = operation.map(|path| {
            if Path::new(&path).is_absolute() {
                PathBuf::from(path)
            } else {
                cwd.join(path)
            }
        });
        let mut scopes = vec![canonical_key(&cwd)?];
        if let Some(path) = &path {
            scopes.push(canonical_key(path.parent().unwrap_or(&cwd))?);
        }
        let scope = self.acquire(scopes, cancel).await?;
        if cancel.is_cancelled() {
            bail!("Workspace asset capture cancelled");
        }
        let capture_cwd = cwd.clone();
        let excluded = self.data_dir.clone();
        let token = cancel.clone();
        let before = tokio::task::spawn_blocking(move || {
            if let Some(path) = path {
                let version = workspace_file(&path);
                Before::File(path, version)
            } else {
                Before::Shell(capture_baseline(&capture_cwd, &excluded, &token))
            }
        })
        .await
        .context("Workspace capture task failed")?;
        if cancel.is_cancelled() {
            bail!("Workspace asset capture cancelled");
        }
        Ok(Some(CaptureTicket {
            tracker: self.clone(),
            _scope: scope,
            before,
            session_id: session_id.to_owned(),
            cwd,
        }))
    }
    /// Revert and first-snapshot capture share the same overlapping workspace locks.
    pub(crate) async fn lock_workspace(
        self: &Arc<Self>,
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Result<WorkspaceLease> {
        let cwd = resolve_real_path(cwd)?;
        self.acquire(vec![canonical_key(&cwd)?], cancel).await
    }
    async fn acquire(
        self: &Arc<Self>,
        scopes: Vec<String>,
        cancel: &CancellationToken,
    ) -> Result<WorkspaceLease> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if cancel.is_cancelled() {
                bail!("Workspace capture lock cancelled");
            }
            {
                let mut locks = self
                    .locks
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Workspace capture lock failed"))?;
                if !locks.held.values().any(|held| {
                    held.iter()
                        .any(|a| scopes.iter().any(|b| nested_key(a, b) || nested_key(b, a)))
                }) {
                    let id = locks.next;
                    locks.next = locks.next.wrapping_add(1);
                    locks.held.insert(id, scopes);
                    return Ok(WorkspaceLease {
                        tracker: self.clone(),
                        id,
                    });
                }
            }
            tokio::select! {_=cancel.cancelled()=>bail!("Workspace capture lock cancelled"),_=notified=>{}}
        }
    }
    pub async fn drain(
        self: &Arc<Self>,
        session_id: &str,
        session_name: &str,
    ) -> Result<Vec<Value>> {
        let pending = self
            .pending_files()?
            .into_iter()
            .filter(|file| file.session_id == session_id)
            .collect::<Vec<_>>();
        let mut assets = Vec::new();
        for file in pending {
            let scopes = vec![
                canonical_key(&file.cwd)?,
                canonical_key(file.file.path.parent().unwrap_or(&file.cwd))?,
            ];
            let _scope = self.acquire(scopes, &CancellationToken::new()).await?;
            let tracker = self.clone();
            let name = session_name.to_owned();
            let archived =
                tokio::task::spawn_blocking(move || tracker.archive_pending(&[file], &name))
                    .await
                    .context("Workspace retry task failed")??;
            assets.extend(archived);
        }
        Ok(assets)
    }
    fn pending_files(&self) -> Result<Vec<PendingFile>> {
        let state = self
            .retries
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace retry lock failed"))?;
        Ok(state["files"]
            .as_array()
            .context("Workspace retry file list is invalid")?
            .iter()
            .filter_map(|value| serde_json::from_value(value.clone()).ok())
            .collect())
    }
    fn archive_pending(&self, files: &[PendingFile], session_name: &str) -> Result<Vec<Value>> {
        let mut state = self
            .retries
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace retry lock failed"))?;
        let mut assets = Vec::new();
        for file in files {
            let current = workspace_file(&file.file.path);
            let stale =
                current.as_ref().map(|current| &current.version) != Some(&file.file.version);
            let archive = if stale {
                Ok(None)
            } else {
                self.store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Asset store lock failed"))?
                    .archive_generated(&file.file.path, &file.session_id, session_name)
            };
            match archive {
                Ok(asset) => {
                    if let Some(asset) = asset {
                        assets.push(asset);
                    } else if !stale {
                        continue;
                    }
                    state["files"]
                        .as_array_mut()
                        .context("Workspace retry file list is invalid")?
                        .retain(|value| {
                            !(value["sessionId"] == file.session_id
                                && value["path"] == json!(file.file.path))
                        });
                }
                Err(_) => {
                    eprintln!(
                        "workspace_asset_archive_failed session={}",
                        crate::security::redact_secret_text(&file.session_id)
                    );
                }
            }
        }
        self.save(&state)?;
        Ok(assets)
    }
    fn remember_pending(&self, files: &[PendingFile]) -> Result<()> {
        let mut state = self
            .retries
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace retry lock failed"))?;
        let values = state["files"]
            .as_array_mut()
            .context("Workspace retry file list is invalid")?;
        for file in files {
            values.retain(|value| {
                !(value["sessionId"] == file.session_id && value["path"] == json!(file.file.path))
            });
            values.push(serde_json::to_value(file)?);
        }
        self.save(&state)
    }
    fn save(&self, state: &Value) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("Workspace retry path has no parent")?;
        fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".workspace-assets-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(state)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}
/// 持久 registry seam 覆盖 loadout、reload 与迟注册工具，不触碰审批 hook 链。
pub fn install(
    session: Arc<AgentSession>,
    tracker: Arc<WorkspaceAssetTracker>,
    sink: Option<AssetEventSink>,
) -> Result<()> {
    install_context(session.clone(), tracker, session.session_id(), sink)
}
/// 子 Agent 可归档到父 session_id，工作区和取消仍来自实际子执行器。
pub fn install_context(
    session: Arc<AgentSession>,
    tracker: Arc<WorkspaceAssetTracker>,
    session_id: String,
    sink: Option<AssetEventSink>,
) -> Result<()> {
    install_context_inner(session, tracker, session_id, sink, None)
}
pub(crate) fn install_context_with_file_changes(
    session: Arc<AgentSession>,
    tracker: Arc<WorkspaceAssetTracker>,
    file_changes: Arc<crate::native_file_changes::FileChangesService>,
    session_id: String,
    sink: Option<AssetEventSink>,
) -> Result<()> {
    install_context_inner(session, tracker, session_id, sink, Some(file_changes))
}
fn install_context_inner(
    session: Arc<AgentSession>,
    tracker: Arc<WorkspaceAssetTracker>,
    session_id: String,
    sink: Option<AssetEventSink>,
    file_changes: Option<Arc<crate::native_file_changes::FileChangesService>>,
) -> Result<()> {
    let cwd = PathBuf::from(
        session
            .session_manager
            .lock()
            .map_err(|_| anyhow::anyhow!("Session manager lock failed"))?
            .get_cwd(),
    );
    let session_name = session.session_name().unwrap_or_default();
    session.set_tool_execution_wrapper(Some(Arc::new(move |tool| {
        if file_changes.is_none()
            && !["write", "edit", "bash", "powershell", "call_tool"].contains(&tool.name.as_str())
        {
            return tool;
        }
        let wrapped = wrap_tool(
            &Arc::new(tool),
            tracker.clone(),
            session_id.clone(),
            session_name.clone(),
            cwd.clone(),
            sink.clone(),
            file_changes.clone(),
        );
        wrapped.as_ref().clone()
    })));
    Ok(())
}
fn wrap_tool(
    tool: &Arc<AgentTool>,
    tracker: Arc<WorkspaceAssetTracker>,
    session_id: String,
    session_name: String,
    cwd: PathBuf,
    sink: Option<AssetEventSink>,
    file_changes: Option<Arc<crate::native_file_changes::FileChangesService>>,
) -> Arc<AgentTool> {
    let mut wrapped = tool.as_ref().clone();
    let execute = tool.execute.clone();
    let name = tool.name.clone();
    wrapped.execute = Arc::new(move |call_id, args, signal, on_update| {
        let tracker = tracker.clone();
        let session_id = session_id.clone();
        let session_name = session_name.clone();
        let cwd = cwd.clone();
        let execute = execute.clone();
        let name = name.clone();
        let sink = sink.clone();
        let file_changes = file_changes.clone();
        Box::pin(async move {
            if operation(&name, &args).is_none() && file_changes.is_none() {
                return execute(call_id, args, signal, on_update).await;
            }
            let cancel = signal.unwrap_or_default().child_token();
            let key = (session_id.clone(), call_id.clone());
            tracker
                .active
                .lock()
                .map_err(|_| anyhow::anyhow!("Workspace capture task lock failed"))?
                .insert(key.clone(), cancel.clone());
            let _active = ActiveCall {
                tracker: tracker.clone(),
                key,
            };
            // 捕获失败应允许原工具继续；取得锁前的取消必须阻止副作用工具执行。
            let ticket = match tracker
                .begin(&session_id, &cwd, &name, &args, &cancel)
                .await
            {
                Ok(ticket) => ticket,
                Err(_) if cancel.is_cancelled() => bail!("Operation aborted"),
                Err(_) => {
                    eprintln!(
                        "workspace_asset_capture_failed session={}",
                        crate::security::redact_secret_text(&session_id)
                    );
                    None
                }
            };
            let _snapshot_scope = if ticket.is_none()
                && file_changes.is_some()
                && crate::native_file_changes::write_operation(&name, &args).is_some()
            {
                Some(tracker.lock_workspace(&cwd, &cancel).await?)
            } else {
                None
            };
            let file_ticket = if let Some(file_changes) = file_changes {
                file_changes
                    .before_tool(&session_id, &cwd, &name, &args)
                    .await
                    .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))?
            } else {
                None
            };
            if cancel.is_cancelled() {
                bail!("Operation aborted");
            }
            let result = execute(call_id, args, Some(cancel), on_update).await;
            let snapshot_result = if let Some(file_ticket) = file_ticket {
                file_ticket
                    .finish(result.is_ok())
                    .await
                    .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))
            } else {
                Ok(())
            };
            if let Some(ticket) = ticket {
                match ticket.finish(&session_name).await {
                    Ok(assets) => {
                        if !assets.is_empty() {
                            if let Some(sink) = sink {
                                sink(session_id, assets).await;
                            }
                        }
                    }
                    Err(_) => eprintln!(
                        "workspace_asset_archive_pending session={}",
                        crate::security::redact_secret_text(&session_id)
                    ),
                }
            }
            match (result, snapshot_result) {
                (Ok(value), Ok(())) => Ok(value),
                (Ok(_), Err(error)) => Err(error),
                (Err(error), Ok(())) => Err(error),
                (Err(error), Err(snapshot)) => {
                    Err(error.context(format!("File change persistence also failed: {snapshot}")))
                }
            }
        })
    });
    Arc::new(wrapped)
}
impl CaptureTicket {
    /// 执行失败时同样调用 finish；生成物以磁盘版本而非工具成功标记为依据。
    pub async fn finish(self, session_name: &str) -> Result<Vec<Value>> {
        let name = session_name.to_owned();
        tokio::task::spawn_blocking(move || {
            let files = match &self.before {
                Before::Shell(Some(baseline)) => {
                    scan_workspace(&baseline.root, &baseline.exclude, &CancellationToken::new())
                        .map(|current| {
                            current
                                .into_iter()
                                .filter(|(path, file)| {
                                    baseline.files.get(path).map(|before| &before.version)
                                        != Some(&file.version)
                                })
                                .map(|(_, file)| file)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                }
                Before::Shell(None) => Vec::new(),
                Before::File(path, before) => workspace_file(path)
                    .filter(|current| {
                        before.as_ref().map(|before| &before.version) != Some(&current.version)
                            && !nested_key(
                                &canonical_key(&self.tracker.data_dir).unwrap_or_default(),
                                &canonical_key(path).unwrap_or_default(),
                            )
                    })
                    .into_iter()
                    .collect(),
            }
            .into_iter()
            .map(|file| PendingFile {
                file,
                session_id: self.session_id.clone(),
                cwd: self.cwd.clone(),
            })
            .collect::<Vec<_>>();
            if !files.is_empty() {
                self.tracker.remember_pending(&files)?;
            }
            self.tracker.archive_pending(&files, &name)
        })
        .await
        .context("Workspace archive task failed")?
    }
}
fn operation(name: &str, args: &Value) -> Option<Option<String>> {
    let (name, args) = if name == "call_tool" {
        (
            args["name"].as_str().unwrap_or("").trim(),
            &args["arguments"],
        )
    } else {
        (name, args)
    };
    match name {
        "bash" | "powershell" => Some(None),
        "write" | "edit" => args["path"]
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .map(|path| Some(Some(path.to_owned())))
            .flatten(),
        _ => None,
    }
}
fn resolve_real_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut parent = absolute.clone();
    let mut suffix = Vec::new();
    loop {
        if let Ok(actual) = fs::canonicalize(&parent) {
            let mut actual = actual;
            for part in suffix.iter().rev() {
                actual.push(part);
            }
            return Ok(actual);
        }
        if let Some(name) = parent.file_name() {
            suffix.push(name.to_owned());
        }
        if !parent.pop() {
            return Ok(absolute);
        }
    }
}
fn canonical_key(path: &Path) -> Result<String> {
    let path = resolve_real_path(path)?
        .to_string_lossy()
        .replace('\\', "/");
    let path = path.trim_start_matches("//?/").trim_end_matches('/');
    Ok(if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_owned()
    })
}
fn nested_key(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|child| child.starts_with('/'))
}
fn workspace_file(path: &Path) -> Option<FileVersion> {
    let info = fs::symlink_metadata(path).ok()?;
    if !info.is_file() || info.len() > 128 * 1024 * 1024 {
        return None;
    }
    let modified = info.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    let created = info
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    Some(FileVersion {
        path: path.to_owned(),
        size: info.len(),
        modified: modified.as_secs_f64() * 1000.0,
        version: format!(
            "{}:{}:{}",
            info.len(),
            modified.as_nanos(),
            created.map(|time| time.as_nanos()).unwrap_or(0)
        ),
    })
}
fn ignored_directory(name: &str) -> bool {
    name.starts_with('.')
        || [
            "__pycache__",
            "bower_components",
            "build",
            "coverage",
            "dist",
            "node_modules",
            "out",
            "release",
            "target",
            "temp",
            "tmp",
            "vendor",
            "venv",
        ]
        .contains(&name)
}
fn ignored_file(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with('~')
        || [
            "bun.lock",
            "bun.lockb",
            "cargo.lock",
            "composer.lock",
            "gemfile.lock",
            "package-lock.json",
            "pnpm-lock.yaml",
            "podfile.lock",
            "uv.lock",
            "yarn.lock",
        ]
        .contains(&name)
        || Path::new(name).extension().is_some_and(|extension| {
            [
                "a",
                "class",
                "d",
                "dll",
                "dylib",
                "exe",
                "map",
                "o",
                "obj",
                "pdb",
                "pyc",
                "pyo",
                "so",
                "swp",
                "swo",
                "tmp",
                "tsbuildinfo",
            ]
            .contains(&extension.to_str().unwrap_or(""))
        })
}
fn scan_workspace(
    root: &Path,
    exclude: &Path,
    cancel: &CancellationToken,
) -> Option<BTreeMap<PathBuf, FileVersion>> {
    let mut pending = vec![root.to_owned()];
    let mut files = BTreeMap::new();
    let mut scanned = 0usize;
    let excluded = canonical_key(exclude).ok()?;
    while let Some(directory) = pending.pop() {
        if cancel.is_cancelled() {
            return None;
        }
        if nested_key(&excluded, &canonical_key(&directory).ok()?) {
            continue;
        }
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            scanned += 1;
            if scanned > 30000 {
                return None;
            }
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if kind.is_dir() {
                if !ignored_directory(&name) {
                    pending.push(path);
                }
                continue;
            }
            if !kind.is_file() || ignored_file(&name) {
                continue;
            }
            if let Some(file) = workspace_file(&path) {
                files.insert(path.strip_prefix(root).ok()?.to_owned(), file);
            }
        }
    }
    Some(files)
}
fn capture_baseline(cwd: &Path, exclude: &Path, cancel: &CancellationToken) -> Option<Baseline> {
    let root = fs::canonicalize(cwd).ok()?;
    if !root.is_dir() {
        return None;
    }
    let files = scan_workspace(&root, exclude, cancel)?;
    Some(Baseline {
        root,
        files,
        exclude: exclude.to_owned(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use pi_rust::{
        agent_core::{
            agent_loop::{agent_loop, AgentLoopConfig, BeforeToolCallOutcome},
            types::{AgentMessage, AgentToolResult},
            AgentContext,
        },
        ai::{
            models::{
                create_models, faux_assistant_message, faux_provider, faux_tool_call,
                CreateModelsOptions, FauxMessageOptions, FauxProviderOptions, FauxToolCallOptions,
            },
            types::{StopReason, StringOrBlocks, UserMessage},
        },
    };
    #[tokio::test]
    async fn actual_parallel_pi_preflight_finishes_then_captures_failed_tool_outputs_without_deadlock(
    ) {
        let root =
            std::env::temp_dir().join(format!("pisper-tracker-pi-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Mutex::new(AssetStore::open(&data).unwrap()));
        let tracker = WorkspaceAssetTracker::new(&data, store.clone()).unwrap();
        let file_changes = crate::native_file_changes::FileChangesService::open(&data).unwrap();
        file_changes.mark_session_tracked("s", &root).await.unwrap();
        let preflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = preflight.clone();
        let workspace = root.clone();
        let tool = Arc::new(AgentTool {
            name: "write".into(),
            label: "write".into(),
            description: "synthetic file writer".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
            execute: Arc::new(move |_, args, _, _| {
                let observed = observed.clone();
                let workspace = workspace.clone();
                Box::pin(async move {
                    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 2);
                    fs::write(
                        workspace.join(args["path"].as_str().unwrap()),
                        "synthetic output",
                    )?;
                    if args["path"] == "failed.txt" {
                        bail!("synthetic tool failure");
                    }
                    Ok(AgentToolResult::default())
                })
            }),
        });
        let wrapped = wrap_tool(
            &tool,
            tracker.clone(),
            "s".into(),
            "synthetic".into(),
            root.clone(),
            None,
            Some(file_changes.clone()),
        );
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        faux.set_responses(vec![
            faux_assistant_message(
                vec![
                    faux_tool_call(
                        "write",
                        json!({"path":"first.txt"}),
                        FauxToolCallOptions {
                            id: Some("one".into()),
                        },
                    ),
                    faux_tool_call(
                        "write",
                        json!({"path":"failed.txt"}),
                        FauxToolCallOptions {
                            id: Some("two".into()),
                        },
                    ),
                ],
                FauxMessageOptions {
                    stop_reason: Some(StopReason::ToolUse),
                    ..Default::default()
                },
            )
            .into(),
            faux_assistant_message("done", FauxMessageOptions::default()).into(),
        ]);
        let mut config = AgentLoopConfig::new(
            faux.get_model(None).unwrap(),
            Arc::new(|messages| {
                Box::pin(async move {
                    messages
                        .iter()
                        .filter_map(|message| message.to_message())
                        .collect()
                })
            }),
        );
        config.before_tool_call = Some(Arc::new(move |_| {
            let preflight = preflight.clone();
            Box::pin(async move {
                preflight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                BeforeToolCallOutcome::default()
            })
        }));
        let (_events, task) = agent_loop(
            vec![AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text("synthetic two writes".into()),
                timestamp: 1,
            })],
            AgentContext {
                messages: vec![],
                tools: vec![wrapped],
            },
            config,
            Arc::new(models),
            None,
        );
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result
            .iter()
            .any(|message| matches!(message,AgentMessage::ToolResult(result) if result.is_error)));
        tracker.wait_session("s").await;
        let changes = file_changes.list("s", &root).await.unwrap();
        assert_eq!(changes["summary"]["files"], 2);
        assert_eq!(changes["summary"]["pending"], 2);
        assert!(changes["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["canRevert"] == true));
        // A failed tool can still write. Both domains inspect actual output,
        // while snapshots retain the absence before either tool executed.
        let reverted = file_changes.revert("s", &root, None).await.unwrap();
        assert_eq!(reverted["reverted"], 2);
        assert!(!root.join("first.txt").exists());
        assert!(!root.join("failed.txt").exists());
        assert_eq!(store.lock().unwrap().generated_for_session("s").len(), 2);
        drop(tracker);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn native_sdk_wrapper_survives_loadout_reload_and_late_extension_registry_refresh() {
        use pi_rust::coding_agent::{
            core::{
                model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
                resource_loader::{
                    DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
                },
                sdk::{create_agent_session, CreateAgentSessionOptions},
            },
            extensions::{loader::ExtensionApi, types::ToolDefinition},
        };
        let root = std::env::temp_dir().join(format!(
            "pisper-tracker-registry-test-{}",
            uuid::Uuid::new_v4()
        ));
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        let api = Arc::new(Mutex::new(None::<ExtensionApi>));
        let saved = api.clone();
        let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
            cwd: root.to_string_lossy().into_owned(),
            agent_dir: data.to_string_lossy().into_owned(),
            no_extensions: true,
            no_skills: true,
            no_prompt_templates: true,
            no_themes: true,
            no_context_files: true,
            extension_factories: vec![InlineExtension::Named {
                factory: Arc::new(move |api| {
                    *saved.lock().unwrap() = Some(api.clone());
                    Ok(())
                }),
                name: "builtin:asset-registry-fixture".into(),
                hidden: false,
            }],
            ..Default::default()
        });
        loader.reload_without_trust().unwrap();
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: Some(data.join("auth.json").to_string_lossy().into_owned()),
            models_path: Some(None),
            models_store_path: Some(
                data.join("models-store.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            refresh_on_create: Some(false),
            allow_model_network: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let session = create_agent_session(CreateAgentSessionOptions {
            cwd: Some(root.to_string_lossy().into_owned()),
            agent_dir: Some(data.to_string_lossy().into_owned()),
            model_runtime: Some(runtime),
            resource_loader: Some(Arc::new(Mutex::new(loader))),
            ..Default::default()
        })
        .await
        .unwrap()
        .session;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        session.set_tool_execution_wrapper(Some(Arc::new(move |mut tool| {
            let execute = tool.execute.clone();
            let counter = counter.clone();
            tool.execute = Arc::new(move |id, args, signal, update| {
                let execute = execute.clone();
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    execute(id, args, signal, update).await
                })
            });
            tool
        })));
        session.set_active_tools_by_name(vec![]);
        session.set_active_tools_by_name(vec!["write".into()]);
        let write = session.agent.state().tools[0].clone();
        (write.execute)(
            "one".into(),
            json!({"path":root.join("one.txt"),"content":"synthetic"}),
            None,
            None,
        )
        .await
        .unwrap();
        let mut late = ToolDefinition::new(
            "mcp__fixture__echo",
            "late native tool",
            "synthetic late registry tool",
            json!({"type":"object","properties":{}}),
        );
        late.execute = Some(Arc::new(|_, _, _, _, _| {
            Ok(json!({"content":[],"details":{}}))
        }));
        api.lock()
            .unwrap()
            .clone()
            .unwrap()
            .register_tool(late)
            .unwrap();
        session.set_active_tools_by_name(vec!["mcp__fixture__echo".into()]);
        let late = session.agent.state().tools[0].clone();
        (late.execute)("two".into(), json!({}), None, None)
            .await
            .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        session.reload(None).await.unwrap();
        session.set_active_tools_by_name(vec!["write".into()]);
        let write = session.agent.state().tools[0].clone();
        (write.execute)(
            "three".into(),
            json!({"path":root.join("three.txt"),"content":"synthetic"}),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        session.set_tool_execution_wrapper(None);
        let write = session.agent.state().tools[0].clone();
        (write.execute)(
            "four".into(),
            json!({"path":root.join("four.txt"),"content":"synthetic"}),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        session.dispose();
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn capture_and_restart_retry_track_real_changes_and_skip_runtime_data() {
        let root =
            std::env::temp_dir().join(format!("pisper-tracker-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("agent-data");
        fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Mutex::new(AssetStore::open(&data).unwrap()));
        let tracker = WorkspaceAssetTracker::new(&data, store.clone()).unwrap();
        let token = CancellationToken::new();
        fs::write(root.join("existing.txt"), "old").unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        let ticket = tracker
            .begin("session", &root, "bash", &json!({}), &token)
            .await
            .unwrap()
            .unwrap();
        fs::write(root.join("existing.txt"), "new content").unwrap();
        fs::write(root.join("new.rs"), "synthetic source").unwrap();
        fs::write(root.join("node_modules/ignored.txt"), "ignored").unwrap();
        fs::write(data.join("internal.txt"), "internal").unwrap();
        let assets = ticket.finish("Synthetic session").await.unwrap();
        assert_eq!(assets.len(), 2);
        let reopened = WorkspaceAssetTracker::new(&data, store.clone()).unwrap();
        assert!(reopened
            .drain("session", "Synthetic session")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store.lock().unwrap().generated_for_session("session").len(),
            2
        );
        drop(reopened);
        drop(tracker);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn overlap_lock_is_cancellable_and_independent_workspaces_can_proceed() {
        let root =
            std::env::temp_dir().join(format!("pisper-tracker-lock-test-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let tracker = WorkspaceAssetTracker::new(
            &data,
            Arc::new(Mutex::new(AssetStore::open(&data).unwrap())),
        )
        .unwrap();
        let held = tracker
            .begin(
                "s",
                &a,
                "write",
                &json!({"path":"one.txt"}),
                &CancellationToken::new(),
            )
            .await
            .unwrap()
            .unwrap();
        let independent = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tracker.begin("t", &b, "bash", &json!({}), &CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(independent.is_some());
        drop(independent);
        let cancelled = CancellationToken::new();
        let next = cancelled.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            next.cancel();
        });
        assert!(tracker
            .begin(
                "s",
                &a,
                "call_tool",
                &json!({"name":" edit ","arguments":{"path":"one.txt"}}),
                &cancelled
            )
            .await
            .is_err());
        drop(held);
        assert!(tracker
            .begin(
                "s",
                &a,
                "write",
                &json!({"path":"one.txt"}),
                &CancellationToken::new()
            )
            .await
            .unwrap()
            .is_some());
        drop(tracker);
        fs::remove_dir_all(root).unwrap();
    }
}
