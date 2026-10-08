//! 只提供执行前/后的窄捕获接口；共享工具注册与工作区锁由已有资产包装负责。
use super::{store, FileChangesService, Result, SessionSlot};
use crate::ApiError;
use serde_json::{json, Value};
use std::{fs, path::Path, sync::Arc};
use tokio::sync::OwnedMutexGuard;

pub(crate) fn write_operation(name: &str, args: &Value) -> Option<String> {
    let (name, args) = if name == "call_tool" {
        (
            args["name"].as_str().unwrap_or("").trim(),
            &args["arguments"],
        )
    } else {
        (name, args)
    };
    if !["write", "edit"].contains(&name) {
        return None;
    }
    args["path"]
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}
pub(crate) struct FileChangeTicket {
    service: Arc<FileChangesService>,
    slot: Arc<SessionSlot>,
    id: String,
    path: String,
    _lease: OwnedMutexGuard<()>,
}
impl FileChangesService {
    /// 调用时必须已放行审批；write/edit 必须已持资产服务的 workspace lease。
    pub(crate) async fn before_tool(
        self: &Arc<Self>,
        id: &str,
        cwd: &Path,
        name: &str,
        args: &Value,
    ) -> Result<Option<FileChangeTicket>> {
        let Some(path) = write_operation(name, args) else {
            if name == "call_tool" || !["read", "grep", "find", "ls"].contains(&name) {
                self.mark_coverage_partial(id, cwd).await?;
            }
            return Ok(None);
        };
        let resolved = pi_rust::coding_agent::core::tools::path_utils::resolve_to_cwd(
            &path,
            &cwd.to_string_lossy(),
        )
        .ok();
        let rel = resolved
            .as_deref()
            .and_then(|path| store::relative(cwd, path, false).ok());
        let Some(path) = rel else {
            self.mark_coverage_partial(id, cwd).await?;
            return Ok(None);
        };
        let slot = self.slot(id)?;
        let lease = slot.lease.clone().lock_owned().await;
        let service = self.clone();
        let id = id.to_owned();
        let cwd = cwd.to_owned();
        tokio::task::spawn_blocking(move || {
            service.ensure_live(&slot)?;
            if let Err(error) = service.capture_before(&id, &cwd, &path, &slot) {
                tracing::warn!(error=%crate::security::redact_secret_text(&error.message), "文件原始快照捕获失败，覆盖状态降级");
                // 若降级标记也保存失败，调用者必须阻止实际工具执行，不能声称零改动。
                service.partial_locked(&id, &cwd, &slot)?;
            }
            Ok(Some(FileChangeTicket { service, slot, id, path, _lease: lease }))
        }).await.map_err(|error|ApiError::internal(format!("文件快照捕获任务失败：{error}")))?
    }
    fn capture_before(&self, id: &str, cwd: &Path, path: &str, slot: &SessionSlot) -> Result<()> {
        let index = self.load_locked(id, slot)?;
        let root = store::real(cwd)?;
        if index["version"] != 2
            || index["cwd"]
                .as_str()
                .is_none_or(|prior| !store::same_path(&root, Path::new(prior)))
        {
            self.partial_locked(id, cwd, slot)?;
        }
        let mut index = self.load_locked(id, slot)?;
        let entries = index["entries"].as_array().unwrap();
        if entries.iter().any(|entry| entry["path"] == path) || entries.len() >= store::MAX_ENTRIES
        {
            return Ok(());
        }
        let absolute = store::target(cwd, path)?;
        let key = store::key(path, 24);
        let (mut before_exists, mut snapshot) = (false, false);
        match fs::metadata(&absolute) {
            Ok(info) => {
                before_exists = true;
                if info.is_file() && info.len() <= store::MAX_SNAPSHOT_BYTES as u64 {
                    if let Some(bytes) =
                        store::bounded(&absolute, store::MAX_SNAPSHOT_BYTES, false)?
                    {
                        store::target(cwd, path)?;
                        if !bytes.iter().take(8192).any(|byte| *byte == 0) {
                            let text = String::from_utf8_lossy(&bytes);
                            store::atomic(&self.snapshot_path(id, &key)?, text.as_bytes())?;
                            snapshot = true;
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(store::io_error(error)),
        }
        index["entries"].as_array_mut().unwrap().push(json!({"path":path,"key":key,"beforeExists":before_exists,"snapshot":snapshot,"changeCount":0,"approved":false,"reverted":false,"changedAt":""}));
        self.save_locked(id, slot, &index)
    }
}
impl FileChangeTicket {
    /// 工具抛出错误时保留最初快照但不增加次数，与 release run() 的 finally 顺序一致。
    pub(crate) async fn finish(self, succeeded: bool) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            // 显式捕获整个 ticket，避免 Rust 2021 字段捕获在等待任务时提前释放 lease。
            let _ticket = &self;
            if !succeeded {
                return Ok(());
            }
            let mut index = self.service.load_locked(&self.id, &self.slot)?;
            if let Some(entry) = index["entries"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|entry| entry["path"] == self.path)
            {
                entry["changeCount"] =
                    json!(entry["changeCount"].as_u64().unwrap_or(0).saturating_add(1));
                entry["approved"] = json!(false);
                entry["reverted"] = json!(false);
                entry["changedAt"] =
                    json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
                if let Err(error) = self.service.save_locked(&self.id, &self.slot, &index) {
                    let cwd = index["cwd"].as_str().map(std::path::PathBuf::from);
                    if let Some(cwd) = cwd {
                        let _ = self.service.partial_locked(&self.id, &cwd, &self.slot);
                    }
                    return Err(error);
                }
            }
            Ok(())
        })
        .await
        .map_err(|error| ApiError::internal(format!("文件快照记录任务失败：{error}")))?
    }
}
