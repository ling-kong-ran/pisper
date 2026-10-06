//! 会话文件变更领域：首次原文、覆盖状态和索引的唯一所有者。
//! 工作区执行锁由装配层提供；本服务仅持会话捕获锁，不安装 Pi 工具包装。
pub(crate) mod diff;
pub(crate) mod store;
#[cfg(test)]
mod tests;
pub(crate) mod tracker;

use crate::ApiError;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Mutex as AsyncMutex;
pub(crate) use tracker::write_operation;
pub(crate) type Result<T> = std::result::Result<T, ApiError>;

pub(crate) struct SessionSlot {
    lease: Arc<AsyncMutex<()>>,
    cached: Mutex<Option<Value>>,
    deleted: AtomicBool,
}
pub(crate) struct FileChangesService {
    root: PathBuf,
    sessions: Mutex<HashMap<String, Arc<SessionSlot>>>,
    closed: AtomicBool,
}
impl FileChangesService {
    pub(crate) fn open(data_dir: &Path) -> Result<Arc<Self>> {
        let root = store::lexical(&if data_dir.is_absolute() {
            data_dir.to_owned()
        } else {
            std::env::current_dir()
                .map_err(store::io_error)?
                .join(data_dir)
        })?
        .join("file-change-snapshots");
        store::prune(&root)?;
        Ok(Arc::new(Self {
            root,
            sessions: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }))
    }
    pub(crate) fn session_dir(&self, id: &str) -> PathBuf {
        self.root.join(store::key(id, 32))
    }
    pub(crate) fn index_path(&self, id: &str) -> PathBuf {
        self.session_dir(id).join("index.json")
    }
    fn snapshot_path(&self, id: &str, key: &str) -> Result<PathBuf> {
        if !store::valid_key(key) {
            return Err(ApiError::bad_request("文件快照键无效。"));
        }
        Ok(self.session_dir(id).join(format!("{key}.before")))
    }
    fn slot(&self, id: &str) -> Result<Arc<SessionSlot>> {
        if id.is_empty() {
            return Err(ApiError::bad_request("会话标识不能为空。"));
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| ApiError::internal("文件快照会话锁无效。"))?;
        Ok(sessions
            .entry(id.into())
            .or_insert_with(|| {
                Arc::new(SessionSlot {
                    lease: Arc::new(AsyncMutex::new(())),
                    cached: Mutex::new(None),
                    deleted: AtomicBool::new(false),
                })
            })
            .clone())
    }
    fn ensure_live(&self, slot: &SessionSlot) -> Result<()> {
        if self.closed.load(Ordering::Acquire) || slot.deleted.load(Ordering::Acquire) {
            return Err(ApiError::new(
                axum::http::StatusCode::GONE,
                "file_changes_closed",
                "文件快照会话已关闭。",
            ));
        }
        Ok(())
    }
    async fn read_operation<T: Send + 'static>(
        self: &Arc<Self>,
        id: &str,
        task: impl FnOnce(&Self, &SessionSlot, &str) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let slot = self.slot(id)?;
        let guard = slot.lease.clone().lock_owned().await;
        let service = self.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            task(&service, &slot, &id)
        })
        .await
        .map_err(|error| ApiError::internal(format!("文件快照任务失败：{error}")))?
    }
    fn load_locked(&self, id: &str, slot: &SessionSlot) -> Result<Value> {
        let mut cache = slot
            .cached
            .lock()
            .map_err(|_| ApiError::internal("文件快照缓存锁无效。"))?;
        if let Some(value) = &*cache {
            return Ok(value.clone());
        }
        let index = store::read_index(&self.index_path(id))?.unwrap_or_else(store::empty);
        if !index.is_object() || !index["entries"].is_array() {
            return Err(ApiError::internal("文件快照索引无效。"));
        }
        *cache = Some(index.clone());
        Ok(index)
    }
    fn save_locked(&self, id: &str, slot: &SessionSlot, index: &Value) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(index)
            .map_err(|_| ApiError::internal("文件快照索引无法序列化。"))?;
        store::atomic(&self.index_path(id), &bytes)?;
        *slot
            .cached
            .lock()
            .map_err(|_| ApiError::internal("文件快照缓存锁无效。"))? = Some(index.clone());
        Ok(())
    }
    fn partial_locked(&self, id: &str, cwd: &Path, slot: &SessionSlot) -> Result<()> {
        let mut index = self.load_locked(id, slot)?;
        if index["coverage"] == "partial" && index["version"] == 2 {
            return Ok(());
        }
        index["version"] = json!(2);
        if index["cwd"].as_str().is_none_or(str::is_empty) {
            index["cwd"] = json!(store::real(cwd)?.to_string_lossy());
        }
        index["coverage"] = json!("partial");
        self.save_locked(id, slot, &index)
    }
    pub(crate) async fn mark_session_tracked(self: &Arc<Self>, id: &str, cwd: &Path) -> Result<()> {
        let cwd = cwd.to_owned();
        self.read_operation(id, move |service, slot, id| {
            service.ensure_live(slot)?;
            if store::read_index(&service.index_path(id))?.is_some() { return Ok(()); }
            service.save_locked(id, slot, &json!({"version":2,"cwd":store::real(&cwd)?.to_string_lossy(),"coverage":"complete","entries":[]}))
        }).await
    }
    pub(crate) async fn mark_coverage_partial(
        self: &Arc<Self>,
        id: &str,
        cwd: &Path,
    ) -> Result<()> {
        let cwd = cwd.to_owned();
        self.read_operation(id, move |service, slot, id| {
            service.ensure_live(slot)?;
            service.partial_locked(id, &cwd, slot)
        })
        .await
    }
    pub(crate) async fn wait_session(&self, id: &str) -> Result<()> {
        let slot = self.slot(id)?;
        let _guard = slot.lease.lock().await;
        Ok(())
    }
    /// 删除标记先于等待，阻止队列中的捕获任务在清理后重建目录。
    pub(crate) async fn clear(self: &Arc<Self>, id: &str) -> Result<()> {
        let slot = self.slot(id)?;
        slot.deleted.store(true, Ordering::Release);
        let guard = slot.lease.clone().lock_owned().await;
        let path = self.session_dir(id);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            store::check_storage(&path)?;
            if path.exists() {
                fs::remove_dir_all(&path).map_err(store::io_error)?;
            }
            *slot
                .cached
                .lock()
                .map_err(|_| ApiError::internal("文件快照缓存锁无效。"))? = None;
            Ok(())
        })
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
    }
    pub(crate) async fn close(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        let slots = self
            .sessions
            .lock()
            .map_err(|_| ApiError::internal("文件快照会话锁无效。"))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for slot in slots {
            let _guard = slot.lease.lock().await;
        }
        Ok(())
    }
    fn snapshot_locked(&self, id: &str, entry: &Value) -> Result<Option<String>> {
        if entry["snapshot"] != true {
            return Ok(None);
        }
        let path = self.snapshot_path(id, entry["key"].as_str().unwrap_or(""))?;
        Ok(store::bounded(&path, store::MAX_SNAPSHOT_BYTES, true)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }
    fn current(cwd: &Path, rel: &str) -> Result<(bool, String)> {
        let path = store::target(cwd, rel)?;
        match fs::metadata(&path) {
            Ok(info) if info.is_dir() => return Ok((false, String::new())),
            Ok(info) if !info.is_file() => return Err(ApiError::bad_request("文件不是普通文件。")),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((false, String::new()))
            }
            Err(error) => return Err(store::io_error(error)),
        }
        match fs::read(&path) {
            Ok(bytes) => {
                store::target(cwd, rel)?;
                Ok((true, diff::normalized(&String::from_utf8_lossy(&bytes))))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound || path.is_dir() => {
                Ok((false, String::new()))
            }
            Err(error) => Err(store::io_error(error)),
        }
    }
    fn list_locked(&self, id: &str, cwd: &Path, slot: &SessionSlot) -> Result<Value> {
        let index = self.load_locked(id, slot)?;
        let mut files = Vec::new();
        let mut added = 0;
        let mut removed = 0;
        for entry in index["entries"].as_array().unwrap() {
            let rel = entry["path"]
                .as_str()
                .ok_or_else(|| ApiError::bad_request("文件快照路径无效。"))?;
            let (exists, current) = Self::current(cwd, rel)?;
            let before = self.snapshot_locked(id, entry)?;
            let before_exists = entry["beforeExists"]
                .as_bool()
                .ok_or_else(|| ApiError::bad_request("文件快照基线无效。"))?;
            let (entry_added, entry_removed, current_equals) = if let Some(before) = &before {
                let (added, removed) = diff::stats(rel, before, &current);
                (
                    added,
                    removed,
                    exists == before_exists && &current == before,
                )
            } else if !before_exists {
                (
                    if exists {
                        current.split('\n').count()
                    } else {
                        0
                    },
                    0,
                    !exists,
                )
            } else {
                (0, 0, false)
            };
            let reverted = entry["reverted"] == true || current_equals;
            let approved = entry["approved"] == true;
            let pending = !reverted
                && !approved
                && (entry_added > 0 || entry_removed > 0 || before.is_none());
            added += entry_added;
            removed += entry_removed;
            files.push(json!({"path":rel,"status":if !before_exists && exists {"created"} else if before_exists && !exists {"deleted"} else {"modified"},"added":entry_added,"removed":entry_removed,"changeCount":entry.get("changeCount").cloned().unwrap_or(json!(0)),"snapshot":before.is_some(),"canRevert":before.is_some() || !before_exists,"approved":approved,"reverted":reverted,"pending":pending,"changedAt":entry.get("changedAt").cloned().unwrap_or(json!("")),"currentEqualsBefore":current_equals}));
        }
        files.sort_by(|a, b| {
            b["changedAt"]
                .as_str()
                .unwrap_or("")
                .cmp(a["changedAt"].as_str().unwrap_or(""))
        });
        Ok(
            json!({"summary":{"files":files.len(),"pending":files.iter().filter(|file|file["pending"] == true).count(),"added":added,"removed":removed},"files":files}),
        )
    }
    pub(crate) async fn list(self: &Arc<Self>, id: &str, cwd: &Path) -> Result<Value> {
        let cwd = cwd.to_owned();
        self.read_operation(id, move |service, slot, id| {
            service.list_locked(id, &cwd, slot)
        })
        .await
    }
    pub(crate) async fn diff(self: &Arc<Self>, id: &str, cwd: &Path, path: &str) -> Result<Value> {
        let cwd = cwd.to_owned();
        let path = store::relative(&cwd, path, true)?;
        self.read_operation(id, move |service, slot, id| {
            let index = service.load_locked(id, slot)?;
            let Some(entry) = index["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["path"] == path)
            else {
                return Ok(
                    json!({"diff":"","diffTruncated":false,"source":"snapshot","found":false}),
                );
            };
            let (_, current) = Self::current(&cwd, &path)?;
            let mut before = service.snapshot_locked(id, entry)?;
            if before.is_none() && entry["beforeExists"] == false {
                before = Some(String::new());
            }
            let (diff, truncated) = before
                .map(|before| {
                    diff::preview(&path, &before, &current, entry["beforeExists"] == false)
                })
                .unwrap_or_default();
            Ok(json!({"diff":diff,"diffTruncated":truncated,"source":"snapshot","found":true}))
        })
        .await
    }
    pub(crate) async fn approve(
        self: &Arc<Self>,
        id: &str,
        cwd: &Path,
        path: Option<&str>,
    ) -> Result<Value> {
        let cwd = cwd.to_owned();
        let path = path
            .filter(|path| !path.is_empty())
            .map(|path| store::relative(&cwd, path, true))
            .transpose()?;
        self.read_operation(id, move |service, slot, id| {
            service.ensure_live(slot)?;
            let mut index = service.load_locked(id, slot)?;
            let mut count = 0;
            for entry in index["entries"].as_array_mut().unwrap() {
                if path.as_ref().is_none_or(|path| entry["path"] == *path) {
                    entry["approved"] = json!(true);
                    count += 1;
                }
            }
            if count > 0 {
                service.save_locked(id, slot, &index)?;
            }
            service.list_locked(id, &cwd, slot)
        })
        .await
    }
    /// 调用者先保留会话运行锁与共享工作区锁，避免新工具与恢复写入交错。
    pub(crate) async fn revert(
        self: &Arc<Self>,
        id: &str,
        cwd: &Path,
        path: Option<&str>,
    ) -> Result<Value> {
        let cwd = cwd.to_owned();
        let path = path
            .filter(|path| !path.is_empty())
            .map(|path| store::relative(&cwd, path, true))
            .transpose()?;
        self.read_operation(id, move |service, slot, id| {
            service.ensure_live(slot)?;
            let mut index = service.load_locked(id, slot)?;
            let mut reverted = 0;
            let mut targets = 0;
            for entry in index["entries"].as_array_mut().unwrap() {
                if path.as_ref().is_some_and(|path| entry["path"] != *path) {
                    continue;
                }
                targets += 1;
                let rel = entry["path"]
                    .as_str()
                    .ok_or_else(|| ApiError::bad_request("文件快照路径无效。"))?;
                store::target(&cwd, rel)?;
                let before = service.snapshot_locked(id, entry)?;
                if let Some(before) = before {
                    let physical = store::mutation_target(&cwd, rel, true)?;
                    store::atomic(&physical, before.as_bytes())?;
                    reverted += 1;
                } else if entry["beforeExists"] == false {
                    // 不允许目录或最终符号链接借“新文件”记录删除工作区其他内容。
                    let physical = store::mutation_target(&cwd, rel, false)?;
                    store::check_storage(
                        physical
                            .parent()
                            .ok_or_else(|| ApiError::bad_request("文件路径无效。"))?,
                    )?;
                    match fs::remove_file(&physical) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(store::io_error(error)),
                    }
                    reverted += 1;
                } else {
                    continue;
                }
                entry["reverted"] = json!(true);
                entry["approved"] = json!(true);
            }
            if targets > 0 {
                service.save_locked(id, slot, &index)?;
            }
            let mut list = service.list_locked(id, &cwd, slot)?;
            list["reverted"] = json!(reverted);
            Ok(list)
        })
        .await
    }
    pub(crate) async fn summary(self: &Arc<Self>, id: &str, cwd: &Path) -> Result<Value> {
        let cwd = cwd.to_owned();
        self.read_operation(id, move |service, slot, id| {
            service.summary_locked(id, &cwd, slot)
        })
        .await
    }
    fn summary_locked(&self, id: &str, cwd: &Path, slot: &SessionSlot) -> Result<Value> {
        let Some(index) = store::bounded(&self.index_path(id), store::MAX_INDEX_BYTES, true)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            return Ok(summary_unknown("unavailable", 0, false));
        };
        let Some(entries) = index["entries"].as_array() else {
            return Ok(summary_unknown("unavailable", 0, false));
        };
        let capped = entries.len() >= store::MAX_ENTRIES;
        if let Some(cached) = &*slot
            .cached
            .lock()
            .map_err(|_| ApiError::internal("文件快照缓存锁无效。"))?
        {
            if cached["entries"] != index["entries"]
                || ["version", "cwd", "coverage"]
                    .iter()
                    .any(|key| cached[*key] != index[*key])
            {
                return Ok(summary_unknown(
                    "partial",
                    entries
                        .len()
                        .max(cached["entries"].as_array().map_or(0, Vec::len)),
                    capped,
                ));
            }
        }
        let root = store::real(cwd).ok();
        if index["version"] != 2
            || !["complete", "partial"].contains(&index["coverage"].as_str().unwrap_or(""))
            || index["coverage"] == "partial"
            || root.as_ref().is_none_or(|root| {
                index["cwd"]
                    .as_str()
                    .is_none_or(|prior| !store::same_path(root, Path::new(prior)))
            })
        {
            return Ok(summary_unknown("partial", entries.len(), capped));
        }
        if entries.is_empty() {
            return Ok(
                json!({"status":"known","changedFiles":0,"pendingFiles":0,"added":0,"removed":0,"unknownFiles":0,"capped":false}),
            );
        }
        if store::real(&self.session_dir(id)).is_err() {
            return Ok(summary_unknown("partial", entries.len(), capped));
        }
        let mut remaining = store::MAX_SUMMARY_TOTAL_BYTES;
        let mut seen = HashSet::new();
        let (mut changed, mut pending, mut added, mut removed, mut unknown) =
            (0, 0, 0, 0, entries.len().saturating_sub(store::MAX_ENTRIES));
        for entry in entries.iter().take(store::MAX_ENTRIES) {
            let Some(path) = entry["path"].as_str().filter(|path| !path.is_empty()) else {
                unknown += 1;
                continue;
            };
            let Some(key) = entry["key"].as_str().filter(|key| store::valid_key(key)) else {
                unknown += 1;
                continue;
            };
            let Some(before_exists) = entry["beforeExists"].as_bool() else {
                unknown += 1;
                continue;
            };
            if !seen.insert(path) {
                unknown += 1;
                continue;
            }
            let current = store::target(cwd, path)
                .map(|path| summary_text(&path, false, &mut remaining))
                .unwrap_or(SummaryText::Unknown);
            let before = if before_exists {
                if entry["snapshot"] == true {
                    self.snapshot_path(id, key)
                        .map(|path| summary_text(&path, true, &mut remaining))
                        .unwrap_or(SummaryText::Unknown)
                } else {
                    SummaryText::Unknown
                }
            } else {
                SummaryText::Missing
            };
            if matches!(current, SummaryText::Unknown)
                || (before_exists && !matches!(before, SummaryText::File(_)))
            {
                unknown += 1;
                continue;
            }
            if !before_exists && matches!(current, SummaryText::Missing) {
                continue;
            }
            let prior = match &before {
                SummaryText::File(text) => text.as_str(),
                _ => "",
            };
            let after = match &current {
                SummaryText::File(text) => text.as_str(),
                _ => "",
            };
            if before_exists && matches!(current, SummaryText::File(_)) && prior == after {
                continue;
            }
            if prior.split('\n').count() > store::MAX_SUMMARY_LINES
                || after.split('\n').count() > store::MAX_SUMMARY_LINES
            {
                unknown += 1;
                continue;
            }
            let (file_added, file_removed) = diff::stats(path, prior, after);
            changed += 1;
            if entry["approved"] != true {
                pending += 1;
            }
            added += file_added;
            removed += file_removed;
        }
        if unknown > 0 || capped {
            return Ok(summary_unknown("partial", unknown, capped));
        }
        Ok(
            json!({"status":"known","changedFiles":changed,"pendingFiles":pending,"added":added,"removed":removed,"unknownFiles":0,"capped":capped}),
        )
    }
}
enum SummaryText {
    Missing,
    Unknown,
    File(String),
}
fn summary_text(path: &Path, storage: bool, remaining: &mut usize) -> SummaryText {
    match store::bounded(path, store::MAX_SUMMARY_FILE_BYTES.min(*remaining), storage) {
        Ok(None) => SummaryText::Missing,
        Ok(Some(bytes)) => {
            *remaining -= bytes.len();
            if bytes.contains(&0) {
                SummaryText::Unknown
            } else {
                SummaryText::File(diff::normalized(&String::from_utf8_lossy(&bytes)))
            }
        }
        Err(_) => SummaryText::Unknown,
    }
}
fn summary_unknown(status: &str, unknown: usize, capped: bool) -> Value {
    json!({"status":status,"changedFiles":null,"pendingFiles":null,"added":null,"removed":null,"unknownFiles":unknown,"capped":capped})
}
