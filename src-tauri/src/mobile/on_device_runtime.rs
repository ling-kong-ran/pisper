use std::path::PathBuf;
use std::sync::Mutex;

use super::embedded_runtime::EmbeddedRuntime;
#[cfg(not(feature = "mobile-store"))]
use super::root_runtime::RootRuntime;
use super::runtime_status::RootRuntimeStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carrier {
    #[cfg(not(feature = "mobile-store"))]
    Root,
    Embedded,
}

pub struct OnDeviceRuntime {
    #[cfg(not(feature = "mobile-store"))]
    root: RootRuntime,
    embedded: EmbeddedRuntime,
    active: Mutex<Option<Carrier>>,
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
            #[cfg(not(feature = "mobile-store"))]
            root: RootRuntime::new(
                runtime_root.join("root"),
                data_root.clone(),
                app_version.clone(),
            ),
            embedded: EmbeddedRuntime::new(
                runtime_root.join("embedded"),
                data_root,
                app_version,
                embedded_resource,
            ),
            active: Mutex::new(None),
            lifecycle: Mutex::new(()),
        }
    }

    pub fn status(&self) -> RootRuntimeStatus {
        let active = *self
            .active
            .lock()
            .expect("on-device Runtime carrier mutex poisoned");
        #[cfg(not(feature = "mobile-store"))]
        if active == Some(Carrier::Root) {
            let status = self.root.status();
            if status.running {
                return public_status(status);
            }
        }
        if active == Some(Carrier::Embedded) {
            let status = self.embedded.status();
            if status.running || status.state == "starting" || status.state == "error" {
                return public_status(status);
            }
        }

        let embedded = self.embedded.status();
        #[cfg(feature = "mobile-store")]
        return public_status(embedded);

        #[cfg(not(feature = "mobile-store"))]
        {
            let root = self.root.status();
            let supported = root.supported || embedded.supported;
            let packaged = root.packaged || embedded.packaged;
            let installed = root.installed || embedded.installed;
            RootRuntimeStatus {
                supported,
                packaged,
                installed,
                running: false,
                state: if !supported {
                    "unsupported".into()
                } else if installed {
                    "installed".into()
                } else if packaged {
                    "available".into()
                } else {
                    "unavailable".into()
                },
                message: if supported && (packaged || installed) {
                    String::new()
                } else {
                    "安装包未包含当前设备可用的本机 Runtime。".into()
                },
                url: String::new(),
                runtime_kind: "node".into(),
            }
        }
    }

    pub fn ensure_started(&self) -> Result<RootRuntimeStatus, String> {
        // Bridge 命令可能并发到达；安装、启动和 carrier 选择必须作为一个事务串行化。
        let _lifecycle = self
            .lifecycle
            .lock()
            .expect("on-device Runtime lifecycle mutex poisoned");
        if let Some(active) = *self
            .active
            .lock()
            .expect("on-device Runtime carrier mutex poisoned")
        {
            let result = match active {
                #[cfg(not(feature = "mobile-store"))]
                Carrier::Root => self.root.ensure_started(),
                Carrier::Embedded => self.embedded.ensure_started(),
            };
            return result.map(public_status);
        }

        #[cfg(not(feature = "mobile-store"))]
        let candidates = {
            let root = self.root.status();
            let embedded = self.embedded.status();
            carrier_candidates(
                root.supported && (root.packaged || root.installed),
                embedded.supported && (embedded.packaged || embedded.installed),
            )
        };
        #[cfg(feature = "mobile-store")]
        let candidates = vec![Carrier::Embedded];

        let mut errors = Vec::new();
        for carrier in candidates {
            let result = match carrier {
                #[cfg(not(feature = "mobile-store"))]
                Carrier::Root => self.root.ensure_started(),
                Carrier::Embedded => self.embedded.ensure_started(),
            };
            match result {
                Ok(status) => {
                    *self
                        .active
                        .lock()
                        .expect("on-device Runtime carrier mutex poisoned") = Some(carrier);
                    return Ok(public_status(status));
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Err("安装包未包含当前设备可用的本机 Runtime。".into())
        } else {
            Err(format!("本机 Runtime 启动失败：{}", errors.join("；")))
        }
    }

    /// 远程模式不终止同进程 embedded Node；它不能在同一 App 进程中安全重启。
    pub fn deactivate(&self) {
        #[cfg(feature = "mobile-store")]
        return;

        #[cfg(not(feature = "mobile-store"))]
        {
            let _lifecycle = self
                .lifecycle
                .lock()
                .expect("on-device Runtime lifecycle mutex poisoned");
            let mut active = self
                .active
                .lock()
                .expect("on-device Runtime carrier mutex poisoned");
            if *active == Some(Carrier::Root) {
                self.root.stop();
                *active = None;
            }
        }
    }

    pub fn shutdown(&self) {
        #[cfg(not(feature = "mobile-store"))]
        {
            let _lifecycle = self
                .lifecycle
                .lock()
                .expect("on-device Runtime lifecycle mutex poisoned");
            self.root.stop();
        }
    }
}

fn public_status(mut status: RootRuntimeStatus) -> RootRuntimeStatus {
    status.runtime_kind = "node".into();
    status
}

#[cfg(not(feature = "mobile-store"))]
fn carrier_candidates(root: bool, embedded: bool) -> Vec<Carrier> {
    let mut carriers = Vec::with_capacity(2);
    if root {
        carriers.push(Carrier::Root);
    }
    if embedded {
        carriers.push(Carrier::Embedded);
    }
    carriers
}

#[cfg(all(test, not(feature = "mobile-store")))]
mod tests {
    use super::{carrier_candidates, Carrier};

    #[test]
    fn rooted_carrier_is_preferred_and_embedded_is_the_fallback() {
        assert_eq!(
            carrier_candidates(true, true),
            vec![Carrier::Root, Carrier::Embedded]
        );
        assert_eq!(carrier_candidates(false, true), vec![Carrier::Embedded]);
        assert_eq!(carrier_candidates(true, false), vec![Carrier::Root]);
        assert!(carrier_candidates(false, false).is_empty());
    }
}
