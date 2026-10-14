use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::embedded_runtime::EmbeddedRuntime;
use super::runtime_status::OnDeviceRuntimeStatus;

pub struct OnDeviceRuntime {
    embedded: EmbeddedRuntime,
    lifecycle: Mutex<()>,
}

impl OnDeviceRuntime {
    pub fn new(
        runtime_root: PathBuf,
        data_root: PathBuf,
        app_version: String,
        embedded_resource: Option<PathBuf>,
    ) -> Self {
        Self {
            embedded: EmbeddedRuntime::new(
                runtime_root.join("embedded"),
                data_root,
                app_version,
                embedded_resource,
            ),
            lifecycle: Mutex::new(()),
        }
    }

    pub fn status(&self) -> OnDeviceRuntimeStatus {
        public_status(self.embedded.status())
    }

    pub(super) fn import_workspace_path(
        &self,
        host_root: &Path,
        imported: &Path,
    ) -> Result<PathBuf, String> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| "本机 Runtime 生命周期锁已损坏。".to_string())?;
        if !self.embedded.status().running {
            return Err("本机 Runtime 尚未运行。".into());
        }
        super::workspace_import::resolve_imported_workspace(host_root, imported)
    }

    pub fn ensure_started(&self) -> Result<OnDeviceRuntimeStatus, String> {
        // Bridge 命令可能并发到达，安装和启动必须串行，避免重复启动同进程 Node。
        let _lifecycle = self
            .lifecycle
            .lock()
            .expect("on-device Runtime lifecycle mutex poisoned");
        self.embedded.ensure_started().map(public_status)
    }
}

fn public_status(mut status: OnDeviceRuntimeStatus) -> OnDeviceRuntimeStatus {
    status.runtime_kind = "node".into();
    status
}
