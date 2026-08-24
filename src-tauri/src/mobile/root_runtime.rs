use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;

use super::android_bridge::{android_asset_exists, android_copy_asset};
use super::runtime_status::RootRuntimeStatus;

const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ARCHIVE_BYTES: u64 = 320 * 1024 * 1024;

#[cfg(target_os = "android")]
// aapt 会识别并自动解压 `.gz`；APK 内使用 `.tgz` 保留原始压缩流。
const ARM64_ASSET: &str = "pisper-root-runtime-android-arm64.tgz";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyLine {
    bootstrap_url: String,
    pid: u32,
}

pub struct RootRuntime {
    root: PathBuf,
    data_root: PathBuf,
    app_version: String,
    asset_name: Option<&'static str>,
    status: Mutex<RootRuntimeStatus>,
    child: Mutex<Option<Child>>,
    mounted: Mutex<bool>,
}

impl RootRuntime {
    pub fn new(root: PathBuf, data_root: PathBuf, app_version: String) -> Self {
        let asset_name = packaged_asset_name();
        let packaged = asset_name.is_some_and(android_asset_exists);
        let installed = installed_version(&root).as_deref() == Some(app_version.as_str());
        let status = if asset_name.is_none() {
            RootRuntimeStatus::unsupported("当前平台或架构不支持完整本机 Runtime。")
        } else {
            RootRuntimeStatus {
                supported: true,
                packaged,
                installed,
                running: false,
                state: if installed {
                    "installed".into()
                } else if packaged {
                    "available".into()
                } else {
                    "unavailable".into()
                },
                message: if packaged || installed {
                    String::new()
                } else {
                    "安装包未包含完整本机 Runtime 资产。".into()
                },
                url: String::new(),
                runtime_kind: "node-full".into(),
            }
        };
        Self {
            root,
            data_root,
            app_version,
            asset_name,
            status: Mutex::new(status),
            child: Mutex::new(None),
            mounted: Mutex::new(false),
        }
    }

    pub fn status(&self) -> RootRuntimeStatus {
        let mut child = self
            .child
            .lock()
            .expect("root Runtime child mutex poisoned");
        if child
            .as_mut()
            .is_some_and(|process| process.try_wait().ok().flatten().is_some())
        {
            *child = None;
            let mut status = self
                .status
                .lock()
                .expect("root Runtime status mutex poisoned");
            status.running = false;
            status.url.clear();
            if status.state == "running" {
                status.state = "error".into();
                status.message = "完整本机 Runtime 已意外退出。".into();
            }
        }
        drop(child);
        self.status
            .lock()
            .expect("root Runtime status mutex poisoned")
            .clone()
    }

    pub fn ensure_started(&self) -> Result<RootRuntimeStatus, String> {
        let current = self.status();
        if current.running {
            return Ok(current);
        }
        if !current.supported {
            return Err(current.message);
        }
        if !current.packaged && !current.installed {
            return Err("安装包未包含完整本机 Runtime 资产。".into());
        }

        self.update_status("starting", "正在获取 root 权限并准备完整 Runtime…");
        if let Err(error) = verify_root_access() {
            self.fail(&error);
            return Err(error);
        }
        if installed_version(&self.root).as_deref() != Some(self.app_version.as_str()) {
            if let Err(error) = self.install() {
                self.fail(&error);
                return Err(error);
            }
        }
        match self.start() {
            Ok(ready) => {
                let mut status = self
                    .status
                    .lock()
                    .expect("root Runtime status mutex poisoned");
                status.installed = true;
                status.running = true;
                status.state = "running".into();
                status.message.clear();
                status.url = ready.bootstrap_url;
                Ok(status.clone())
            }
            Err(error) => {
                self.stop();
                self.fail(&error);
                Err(error)
            }
        }
    }

    pub fn stop(&self) {
        if let Some(mut child) = self
            .child
            .lock()
            .expect("root Runtime child mutex poisoned")
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let mounted = std::mem::take(
            &mut *self
                .mounted
                .lock()
                .expect("root Runtime mount mutex poisoned"),
        );
        if mounted {
            let rootfs = self.root.join("rootfs");
            let script = format!(
                concat!(
                    "umount {workspace} 2>/dev/null || true; ",
                    "umount {data} 2>/dev/null || true; ",
                    "umount {sys} 2>/dev/null || true; ",
                    "umount {dev} 2>/dev/null || true; ",
                    "umount {proc} 2>/dev/null || true"
                ),
                workspace = shell_quote(&rootfs.join("workspace")),
                data = shell_quote(&rootfs.join("data")),
                sys = shell_quote(&rootfs.join("sys")),
                dev = shell_quote(&rootfs.join("dev")),
                proc = shell_quote(&rootfs.join("proc")),
            );
            let _ = root_command(&script);
        }
        let mut status = self
            .status
            .lock()
            .expect("root Runtime status mutex poisoned");
        status.running = false;
        status.url.clear();
        if status.installed {
            status.state = "installed".into();
            status.message.clear();
        }
    }

    fn install(&self) -> Result<(), String> {
        let asset_name = self
            .asset_name
            .ok_or_else(|| "当前平台不支持完整本机 Runtime。".to_string())?;
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("无法创建完整 Runtime 目录：{error}"))?;
        let archive = self.root.join(format!("{asset_name}.part"));
        android_copy_asset(asset_name, &archive)?;
        let size = std::fs::metadata(&archive)
            .map_err(|error| format!("无法读取完整 Runtime 资产：{error}"))?
            .len();
        if size == 0 || size > MAX_ARCHIVE_BYTES {
            let _ = std::fs::remove_file(&archive);
            return Err("完整 Runtime 资产大小无效。".into());
        }

        let staging = self.root.join("rootfs.staging");
        let rootfs = self.root.join("rootfs");
        let previous = self.root.join("rootfs.previous");
        let (uid, gid, _) = current_identity()?;
        let script = format!(
            concat!(
                "set -eu; ",
                "rm -rf {staging} {previous}; mkdir -p {staging}; ",
                "/system/bin/tar -xzf {archive} -C {staging}; ",
                "test -x {sidecar}; test -x {setpriv}; ",
                "chown -R 0:0 {staging}; chmod 0755 {staging}; ",
                "mkdir -p {data} {workspace} {proc} {dev} {sys}; ",
                "chown -R {uid}:{gid} {data} {workspace}; ",
                "if [ -d {rootfs} ]; then mv {rootfs} {previous}; fi; ",
                "mv {staging} {rootfs}; rm -rf {previous}; rm -f {archive}"
            ),
            staging = shell_quote(&staging),
            previous = shell_quote(&previous),
            archive = shell_quote(&archive),
            sidecar = shell_quote(&staging.join("opt/pisper/pisper-sidecar")),
            setpriv = shell_quote(&staging.join("usr/bin/setpriv")),
            data = shell_quote(&staging.join("data/agent")),
            workspace = shell_quote(&staging.join("workspace")),
            proc = shell_quote(&staging.join("proc")),
            dev = shell_quote(&staging.join("dev")),
            sys = shell_quote(&staging.join("sys")),
            uid = uid,
            gid = gid,
            rootfs = shell_quote(&rootfs),
        );
        if let Err(error) = root_command(&script) {
            let _ = std::fs::remove_file(&archive);
            return Err(format!("无法安装完整本机 Runtime：{error}"));
        }
        if installed_version(&self.root).as_deref() != Some(self.app_version.as_str()) {
            return Err("完整本机 Runtime 安装后的版本标记不匹配。".into());
        }
        let mut status = self
            .status
            .lock()
            .expect("root Runtime status mutex poisoned");
        status.installed = true;
        Ok(())
    }

    fn start(&self) -> Result<ReadyLine, String> {
        let rootfs = self.root.join("rootfs");
        let shared_workspace = self.data_root.join("workspace");
        std::fs::create_dir_all(self.data_root.join("agent"))
            .and_then(|_| std::fs::create_dir_all(&shared_workspace))
            .map_err(|error| format!("无法创建共享本机 Runtime 数据目录：{error}"))?;
        let (uid, gid, mut groups) = current_identity()?;
        if !groups.contains(&gid) {
            groups.push(gid);
        }
        groups.sort_unstable();
        groups.dedup();
        let groups = groups
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mount_script = format!(
            concat!(
                "set -eu; ",
                "test -e {proc}/self || mount --bind /proc {proc}; ",
                "test -e {dev}/null || mount --bind /dev {dev}; ",
                "test -e {sys}/kernel || mount --bind /sys {sys}; ",
                "umount {workspace} 2>/dev/null || true; ",
                "umount {data} 2>/dev/null || true; ",
                "mount --bind {shared_data} {data}; ",
                "mount --bind {shared_workspace} {workspace}"
            ),
            proc = shell_quote(&rootfs.join("proc")),
            dev = shell_quote(&rootfs.join("dev")),
            sys = shell_quote(&rootfs.join("sys")),
            data = shell_quote(&rootfs.join("data")),
            workspace = shell_quote(&rootfs.join("workspace")),
            shared_data = shell_quote(&self.data_root),
            shared_workspace = shell_quote(&shared_workspace),
        );
        *self
            .mounted
            .lock()
            .expect("root Runtime mount mutex poisoned") = true;
        root_command(&mount_script).map_err(|error| format!("无法挂载完整 Runtime：{error}"))?;

        let mut token = [0u8; 32];
        OsRng.fill_bytes(&mut token);
        let token = URL_SAFE_NO_PAD.encode(token);
        let command = format!(
            concat!(
                "exec /system/bin/chroot {rootfs} /usr/bin/setpriv ",
                "--reuid={uid} --regid={gid} --groups={groups} --no-new-privs ",
                "/usr/bin/env -i HOME=/data USER=pisper LANG=C.UTF-8 ",
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin ",
                "PISPER_APP_ROOT=/opt/pisper/sidecar-runtime ",
                "PISPER_FRONTEND_ROOT=/opt/pisper/dist ",
                "PISPER_AGENT_DIR=/data/agent PISPER_WORKSPACE_DIR=/workspace ",
                "PISPER_RUNTIME_PROFILE=mobile-root ",
                "PISPER_DESKTOP_TOKEN={token} PISPER_EXIT_ON_STDIN_CLOSE=0 ",
                "PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0 ",
                "/opt/pisper/pisper-sidecar"
            ),
            rootfs = shell_quote(&rootfs),
            uid = uid,
            gid = gid,
            groups = groups,
            token = token,
        );
        let mut child = Command::new("su")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动 root 命令：{error}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法读取完整 Runtime 启动输出。".to_string())?;
        let stderr = child.stderr.take();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let mut ready_sender = Some(sender);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(value) = line.strip_prefix(READY_PREFIX) {
                    if let Some(sender) = ready_sender.take() {
                        let _ = sender.send(
                            serde_json::from_str::<ReadyLine>(value)
                                .map_err(|error| format!("完整 Runtime READY 数据无效：{error}")),
                        );
                    }
                } else {
                    // READY 后仍持续排空管道，避免 Runtime 后续日志写入触发 EPIPE。
                    eprintln!("[mobile-root-runtime] {line}");
                }
            }
            if let Some(sender) = ready_sender {
                let _ = sender.send(Err("完整 Runtime 未返回 READY。".into()));
            }
        });
        if let Some(stderr) = stderr {
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader};
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("[mobile-root-runtime] {line}");
                }
            });
        }

        let ready = match receiver.recv_timeout(START_TIMEOUT) {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("完整 Runtime 启动超时。".into());
            }
        };
        if ready.pid == 0 || !ready.bootstrap_url.starts_with("http://127.0.0.1:") {
            let _ = child.kill();
            let _ = child.wait();
            return Err("完整 Runtime 返回了不受信任的启动地址。".into());
        }
        *self
            .child
            .lock()
            .expect("root Runtime child mutex poisoned") = Some(child);
        Ok(ready)
    }

    fn update_status(&self, state: &str, message: &str) {
        let mut status = self
            .status
            .lock()
            .expect("root Runtime status mutex poisoned");
        status.state = state.into();
        status.message = message.into();
    }

    fn fail(&self, error: &str) {
        let mut status = self
            .status
            .lock()
            .expect("root Runtime status mutex poisoned");
        status.running = false;
        status.state = "error".into();
        status.message = error.into();
        status.url.clear();
    }
}

impl Drop for RootRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn installed_marker(root: &Path) -> PathBuf {
    root.join("rootfs/opt/pisper/root-runtime.json")
}

fn installed_version(root: &Path) -> Option<String> {
    let value = std::fs::read(installed_marker(root)).ok()?;
    serde_json::from_slice::<serde_json::Value>(&value)
        .ok()?
        .get("appVersion")?
        .as_str()
        .map(ToOwned::to_owned)
}

fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn verify_root_access() -> Result<(), String> {
    let output = Command::new("su")
        .args(["-c", "id -u"])
        .output()
        .map_err(|error| format!("无法请求 root 权限：{error}"))?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "0" {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if message.is_empty() {
            "未获得 root 权限，当前构建无法启动本机 Node Runtime。".into()
        } else {
            format!("未获得 root 权限：{message}")
        })
    }
}

fn root_command(script: &str) -> Result<(), String> {
    let output = Command::new("su")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|error| format!("无法执行 root 命令：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("命令退出状态 {}", output.status)
        } else {
            stderr
        })
    }
}

fn current_identity() -> Result<(u32, u32, Vec<u32>), String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("无法读取 App 身份：{error}"))?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .ok_or_else(|| format!("App 身份缺少 {name}"))
    };
    let uid = field("Uid:")?
        .split_whitespace()
        .next()
        .ok_or_else(|| "App UID 为空。".to_string())?
        .parse::<u32>()
        .map_err(|error| format!("App UID 无效：{error}"))?;
    let gid = field("Gid:")?
        .split_whitespace()
        .next()
        .ok_or_else(|| "App GID 为空。".to_string())?
        .parse::<u32>()
        .map_err(|error| format!("App GID 无效：{error}"))?;
    let groups = field("Groups:")?
        .split_whitespace()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|error| format!("App 附加组无效：{error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((uid, gid, groups))
}

#[cfg(target_os = "android")]
fn packaged_asset_name() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Some(ARM64_ASSET),
        _ => None,
    }
}

#[cfg(not(target_os = "android"))]
fn packaged_asset_name() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::shell_quote;
    use std::path::Path;

    #[test]
    fn shell_path_is_single_quoted() {
        assert_eq!(shell_quote(Path::new("/tmp/a b")), "'/tmp/a b'");
        assert_eq!(shell_quote(Path::new("/tmp/a'b")), "'/tmp/a'\\''b'");
    }
}
