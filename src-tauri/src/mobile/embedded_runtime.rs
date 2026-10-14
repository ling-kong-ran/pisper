#[cfg(target_os = "ios")]
use std::ffi::{c_void, CStr, CString};
use std::fs::File;
use std::io::Read;
#[cfg(target_os = "ios")]
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "ios")]
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::read::GzDecoder;
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(target_os = "android")]
use super::android_bridge::{
    android_asset_exists, android_copy_asset, android_read_asset_prefix, with_android_env,
};
use super::runtime_status::OnDeviceRuntimeStatus;

// Android 的 aapt 会自动解压并改名 `.gz` 资产；使用 `.tgz` 才能让 Rust 收到原始 gzip。
#[cfg(target_os = "android")]
const EMBEDDED_ASSET: &str = "pisper-embedded-runtime.tgz";
#[cfg(not(target_os = "android"))]
const EMBEDDED_ASSET: &str = "pisper-embedded-runtime.tar.gz";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ARCHIVE_BYTES: u64 = 180 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024;
const ARCHIVE_PROBE_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddedManifest {
    schema_version: u32,
    app_version: String,
    runtime_profile: String,
    entry: String,
    build_sha256: String,
    entry_sha256: String,
    frontend_sha256: String,
    #[serde(default)]
    archive_sha256: Option<String>,
}

impl EmbeddedManifest {
    fn matches_app(&self, app_version: &str) -> bool {
        self.schema_version == 1
            && self.app_version == app_version
            && self.runtime_profile == runtime_profile()
            && self.entry == "runtime/mobile-embedded.mjs"
            && valid_sha256(&self.build_sha256)
            && valid_sha256(&self.entry_sha256)
            && valid_sha256(&self.frontend_sha256)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddedReady {
    #[serde(default)]
    bootstrap_url: String,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    runtime_profile: String,
    #[serde(default)]
    error: String,
}

pub struct EmbeddedRuntime {
    root: PathBuf,
    data_root: PathBuf,
    app_version: String,
    resource_archive: Option<PathBuf>,
    status: Mutex<OnDeviceRuntimeStatus>,
    started: AtomicBool,
    #[cfg(target_os = "ios")]
    node_thread_alive: Arc<AtomicBool>,
    token: Mutex<Option<String>>,
}

impl EmbeddedRuntime {
    pub fn new(
        root: PathBuf,
        data_root: PathBuf,
        app_version: String,
        resource_archive: Option<PathBuf>,
    ) -> Self {
        let supported = platform_supported();
        let packaged = supported && packaged_archive_exists(resource_archive.as_deref());
        let installed = read_installed_manifest(&root, &app_version).is_some();
        let status = OnDeviceRuntimeStatus {
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
            message: if !supported {
                "当前平台或架构不支持嵌入式 Node Runtime。".into()
            } else if !packaged && !installed {
                "安装包未包含嵌入式 Node Runtime。".into()
            } else {
                String::new()
            },
            url: String::new(),
            runtime_kind: "node-embedded".into(),
        };
        Self {
            root,
            data_root,
            app_version,
            resource_archive,
            status: Mutex::new(status),
            started: AtomicBool::new(false),
            #[cfg(target_os = "ios")]
            node_thread_alive: Arc::new(AtomicBool::new(false)),
            token: Mutex::new(None),
        }
    }

    pub fn status(&self) -> OnDeviceRuntimeStatus {
        self.status
            .lock()
            .expect("embedded Runtime status mutex poisoned")
            .clone()
    }

    pub fn ensure_started(&self) -> Result<OnDeviceRuntimeStatus, String> {
        let current = self.status();
        if current.running {
            if self.node_is_alive()? {
                super::startup_trace("embedded-healthy-reuse");
                return Ok(current);
            }
            self.reset_after_exit();
        }
        if self.started.load(Ordering::Acquire) {
            if !self.node_is_alive()? {
                self.reset_after_exit();
            } else {
                let token = self
                    .token
                    .lock()
                    .expect("embedded Runtime token mutex poisoned")
                    .clone()
                    .ok_or_else(|| "嵌入式 Node 已启动，但缺少 READY 认证上下文。".to_string())?;
                return match self.wait_until_ready(&token) {
                    Ok(ready) => Ok(self.mark_ready(ready)),
                    Err(error) => {
                        self.fail(&error);
                        Err(error)
                    }
                };
            }
        }
        if !current.supported {
            return Err(current.message);
        }
        if !current.packaged && !current.installed {
            return Err("安装包未包含嵌入式 Node Runtime。".into());
        }

        self.update_status("starting", "正在准备本机 Runtime…");
        super::startup_trace("embedded-install-check-begin");
        let installed_matches = self.installed_matches_packaged_runtime(current.packaged);
        super::startup_trace("embedded-install-check-end");
        if !installed_matches {
            super::startup_trace("embedded-install-required");
            if let Err(error) = self.install() {
                self.fail(&error);
                return Err(error);
            }
        } else {
            super::startup_trace("embedded-install-reuse");
        }
        let token = if !self.started.swap(true, Ordering::AcqRel) {
            match self.start_node() {
                Ok(token) => token,
                Err(error) => {
                    self.started.store(false, Ordering::Release);
                    self.token
                        .lock()
                        .expect("embedded Runtime token mutex poisoned")
                        .take();
                    self.fail(&error);
                    return Err(error);
                }
            }
        } else {
            return Err("嵌入式 Node 已启动，但本机 Runtime 尚未就绪。".into());
        };

        *self
            .token
            .lock()
            .expect("embedded Runtime token mutex poisoned") = Some(token.clone());
        match self.wait_until_ready(&token) {
            Ok(ready) => Ok(self.mark_ready(ready)),
            Err(error) => {
                self.fail(&error);
                Err(error)
            }
        }
    }

    fn install(&self) -> Result<(), String> {
        let parent = self
            .root
            .parent()
            .ok_or_else(|| "嵌入式 Runtime 安装目录无效。".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建嵌入式 Runtime 目录：{error}"))?;
        let archive = parent.join(format!("{EMBEDDED_ASSET}.part"));
        super::startup_trace("embedded-archive-copy-begin");
        let copied = copy_packaged_archive(self.resource_archive.as_deref(), &archive);
        super::startup_trace(if copied.is_ok() {
            "embedded-archive-copy-end"
        } else {
            "embedded-archive-copy-failed"
        });
        let result = copied.and_then(|_| self.install_archive(&archive));
        let _ = std::fs::remove_file(&archive);
        result
    }

    fn install_archive(&self, archive: &Path) -> Result<(), String> {
        let parent = self
            .root
            .parent()
            .ok_or_else(|| "嵌入式 Runtime 安装目录无效。".to_string())?;
        let size = std::fs::metadata(archive)
            .map_err(|error| format!("无法读取嵌入式 Runtime 资产：{error}"))?
            .len();
        if size == 0 || size > MAX_ARCHIVE_BYTES {
            return Err("嵌入式 Runtime 资产大小无效。".into());
        }

        let staging = parent.join("embedded-runtime.staging");
        let previous = parent.join("embedded-runtime.previous");
        // 上次进程可能在两次 rename 之间退出，必须先恢复旧目录，不能删掉唯一完整安装。
        if !self.root.exists() && previous.exists() {
            std::fs::rename(&previous, &self.root)
                .map_err(|error| format!("无法恢复旧嵌入式 Runtime：{error}"))?;
        }
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("无法创建嵌入式 Runtime staging：{error}"))?;
        super::startup_trace("embedded-archive-hash-begin");
        let fingerprint = archive_sha256(archive);
        super::startup_trace(if fingerprint.is_ok() {
            "embedded-archive-hash-end"
        } else {
            "embedded-archive-hash-failed"
        });
        let unpack_result = fingerprint.and_then(|archive_fingerprint| {
            super::startup_trace("embedded-archive-unpack-begin");
            let result = unpack_archive(archive, &staging)
                .and_then(|_| record_archive_fingerprint(&staging, &archive_fingerprint))
                .and_then(|_| {
                    validate_installation(&staging, &self.app_version, &archive_fingerprint)
                });
            super::startup_trace(if result.is_ok() {
                "embedded-archive-unpack-end"
            } else {
                "embedded-archive-unpack-failed"
            });
            result
        });
        if let Err(error) = unpack_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }

        publish_installation(&self.root, &staging, &previous)?;
        self.status
            .lock()
            .expect("embedded Runtime status mutex poisoned")
            .installed = true;
        Ok(())
    }

    fn start_node(&self) -> Result<String, String> {
        super::startup_trace("embedded-node-start-begin");
        std::fs::create_dir_all(&self.data_root)
            .map_err(|error| format!("无法创建本机 Runtime 数据目录：{error}"))?;
        let agent_dir = self.data_root.join("agent");
        let workspace_dir = self.data_root.join("workspace");
        std::fs::create_dir_all(&agent_dir)
            .and_then(|_| std::fs::create_dir_all(&workspace_dir))
            .map_err(|error| format!("无法创建本机 Runtime 工作目录：{error}"))?;
        let ready_file = self.data_root.join("embedded-ready.json");
        let _ = std::fs::remove_file(&ready_file);
        let _ = std::fs::remove_file(ready_file.with_extension("json.tmp"));

        let mut token = [0u8; 32];
        OsRng.fill_bytes(&mut token);
        let token = URL_SAFE_NO_PAD.encode(token);
        set_runtime_env("PISPER_APP_ROOT", &self.root)?;
        set_runtime_env("PISPER_FRONTEND_ROOT", &self.root.join("dist"))?;
        set_runtime_env("PISPER_AGENT_DIR", &agent_dir)?;
        set_runtime_env("PISPER_WORKSPACE_DIR", &workspace_dir)?;
        set_runtime_env("PISPER_MOBILE_READY_FILE", &ready_file)?;
        std::env::set_var("PISPER_RUNTIME_PROFILE", runtime_profile());
        #[cfg(target_os = "ios")]
        std::env::set_var("PISPER_RUNTIME_PLATFORM", "ios");
        // Node Mobile 不保证 argv[1] 保留入口路径，由宿主显式声明启动语义。
        std::env::set_var("PISPER_MOBILE_AUTOSTART", "1");
        std::env::set_var("PISPER_DESKTOP_TOKEN", &token);
        std::env::set_var("PI_SKIP_VERSION_CHECK", "1");
        std::env::set_var("PI_TELEMETRY", "0");

        let entry = self.root.join("runtime/mobile-embedded.mjs");
        let entry = entry
            .to_str()
            .ok_or_else(|| "嵌入式 Runtime 入口路径不是 UTF-8。".to_string())?;
        #[cfg(target_os = "ios")]
        launch_node(
            vec!["node".into(), "--no-warnings".into(), entry.into()],
            self.node_thread_alive.clone(),
        )?;
        #[cfg(not(target_os = "ios"))]
        launch_node(vec!["node".into(), "--no-warnings".into(), entry.into()])?;
        super::startup_trace("embedded-node-start-end");
        Ok(token)
    }

    fn wait_until_ready(&self, token: &str) -> Result<EmbeddedReady, String> {
        super::startup_trace("embedded-ready-wait-begin");
        let ready_file = self.data_root.join("embedded-ready.json");
        let started_at = Instant::now();
        while started_at.elapsed() < START_TIMEOUT {
            match std::fs::read(&ready_file) {
                Ok(contents) => {
                    let ready: EmbeddedReady = serde_json::from_slice(&contents)
                        .map_err(|error| format!("嵌入式 Runtime READY 数据无效：{error}"))?;
                    if !ready.error.is_empty() {
                        return Err(format!("嵌入式 Runtime 启动失败：{}", ready.error));
                    }
                    if ready.pid == 0
                        || ready.runtime_profile != runtime_profile()
                        || !trusted_bootstrap_url(&ready.bootstrap_url, token)
                    {
                        return Err("嵌入式 Runtime 返回了不受信任的启动地址。".into());
                    }
                    super::startup_trace("embedded-ready-wait-end");
                    return Ok(ready);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(format!("无法读取嵌入式 Runtime READY：{error}")),
            }
        }
        super::startup_trace("embedded-ready-wait-timeout");
        Err("嵌入式 Runtime 启动超时。".into())
    }

    fn mark_ready(&self, ready: EmbeddedReady) -> OnDeviceRuntimeStatus {
        let mut status = self
            .status
            .lock()
            .expect("embedded Runtime status mutex poisoned");
        status.installed = true;
        status.running = true;
        status.state = "running".into();
        status.message.clear();
        status.url = ready.bootstrap_url;
        status.clone()
    }

    fn update_status(&self, state: &str, message: &str) {
        let mut status = self
            .status
            .lock()
            .expect("embedded Runtime status mutex poisoned");
        status.state = state.into();
        status.message = message.into();
    }

    fn fail(&self, error: &str) {
        super::startup_trace("embedded-startup-failed");
        let mut status = self
            .status
            .lock()
            .expect("embedded Runtime status mutex poisoned");
        status.running = false;
        status.state = "error".into();
        status.message = error.into();
        status.url.clear();
    }

    fn node_is_alive(&self) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        {
            return android_node_started();
        }
        #[cfg(target_os = "ios")]
        {
            return Ok(self.node_thread_alive.load(Ordering::Acquire));
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            Ok(true)
        }
    }

    fn reset_after_exit(&self) {
        self.started.store(false, Ordering::Release);
        self.token
            .lock()
            .expect("embedded Runtime token mutex poisoned")
            .take();
        let mut status = self
            .status
            .lock()
            .expect("embedded Runtime status mutex poisoned");
        status.running = false;
        status.url.clear();
        if status.installed {
            status.state = "installed".into();
            status.message.clear();
        }
    }
}

fn publish_installation(root: &Path, staging: &Path, previous: &Path) -> Result<(), String> {
    if root.exists() {
        if previous.exists() {
            std::fs::remove_dir_all(previous)
                .map_err(|error| format!("无法清理旧嵌入式 Runtime 备份：{error}"))?;
        }
        std::fs::rename(root, previous)
            .map_err(|error| format!("无法暂存旧嵌入式 Runtime：{error}"))?;
    }
    if let Err(error) = std::fs::rename(staging, root) {
        if previous.exists() {
            std::fs::rename(previous, root).map_err(|rollback_error| {
                format!(
                    "无法安装嵌入式 Runtime：{error}；旧安装仍在 {}，恢复失败：{rollback_error}",
                    previous.display()
                )
            })?;
        }
        let _ = std::fs::remove_dir_all(staging);
        return Err(format!("无法安装嵌入式 Runtime：{error}"));
    }
    let _ = std::fs::remove_dir_all(previous);
    Ok(())
}

fn unpack_archive(archive: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| format!("无法打开嵌入式 Runtime：{error}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(target)
        .map_err(|error| format!("无法解压嵌入式 Runtime：{error}"))?;
    // tar 结束标记早于 gzip 尾部，继续读到 EOF 才会验证 CRC 和截断错误。
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())
        .map(|_| ())
        .map_err(|error| format!("嵌入式 Runtime 压缩数据损坏：{error}"))
}

fn validate_installation(
    root: &Path,
    app_version: &str,
    archive_fingerprint: &str,
) -> Result<(), String> {
    let manifest = read_installed_manifest(root, app_version)
        .ok_or_else(|| "嵌入式 Runtime 清单、入口或 React 资源与 App 不匹配。".to_string())?;
    if manifest.archive_sha256.as_deref() != Some(archive_fingerprint) {
        return Err("嵌入式 Runtime 归档指纹不匹配。".into());
    }
    Ok(())
}

fn record_archive_fingerprint(root: &Path, fingerprint: &str) -> Result<(), String> {
    let marker = root.join("embedded-runtime.json");
    let file =
        open_manifest(&marker).ok_or_else(|| "嵌入式 Runtime 清单缺失或大小无效。".to_string())?;
    let mut value: serde_json::Value = serde_json::from_reader(file.take(MAX_MANIFEST_BYTES))
        .map_err(|error| format!("嵌入式 Runtime 清单无效：{error}"))?;
    value
        .as_object_mut()
        .ok_or_else(|| "嵌入式 Runtime 清单不是对象。".to_string())?
        .insert(
            "archiveSha256".into(),
            serde_json::Value::String(fingerprint.into()),
        );
    let serialized = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("无法生成嵌入式 Runtime 清单：{error}"))?;
    std::fs::write(&marker, serialized)
        .map_err(|error| format!("无法写入嵌入式 Runtime 清单：{error}"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn open_manifest(path: &Path) -> Option<File> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    File::open(path).ok()
}

fn read_installed_manifest(root: &Path, app_version: &str) -> Option<EmbeddedManifest> {
    let file = open_manifest(&root.join("embedded-runtime.json"))?;
    let manifest: EmbeddedManifest = serde_json::from_reader(file.take(MAX_MANIFEST_BYTES)).ok()?;
    if !manifest.matches_app(app_version)
        || !manifest.archive_sha256.as_deref().is_some_and(valid_sha256)
        || ![
            ("runtime/mobile-embedded.mjs", &manifest.entry_sha256),
            ("dist/index.html", &manifest.frontend_sha256),
        ]
        .iter()
        .all(|(path, expected)| {
            let path = root.join(path);
            std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
                && archive_sha256(&path).is_ok_and(|actual| actual == **expected)
        })
    {
        return None;
    }
    Some(manifest)
}

fn archive_manifest(reader: impl Read) -> Option<EmbeddedManifest> {
    // 仅解析首个条目且限制压缩及解压字节数，旧包回退到完整验证，不能顺序扫描大包。
    let decoder = GzDecoder::new(reader.take(ARCHIVE_PROBE_BYTES as u64));
    let mut archive = tar::Archive::new(decoder.take(MAX_MANIFEST_BYTES + 1024));
    let mut entries = archive.entries().ok()?.raw(true);
    let entry = entries.next()?.ok()?;
    let path = entry.path().ok()?;
    if (path.as_ref() != Path::new("./embedded-runtime.json")
        && path.as_ref() != Path::new("embedded-runtime.json"))
        || !entry.header().entry_type().is_file()
        || entry.size() > MAX_MANIFEST_BYTES
    {
        return None;
    }
    serde_json::from_reader(entry).ok()
}

fn archive_sha256(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("无法读取嵌入式 Runtime 归档：{error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法计算嵌入式 Runtime 指纹：{error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

impl EmbeddedRuntime {
    fn installed_matches_packaged_runtime(&self, packaged: bool) -> bool {
        let Some(installed) = read_installed_manifest(&self.root, &self.app_version) else {
            return false;
        };
        if !packaged {
            return true;
        }
        let Ok(packaged) = self.packaged_manifest() else {
            return false;
        };
        // 包内指纹由 App 签名保护，不是任意外部归档；这里只校验合同和两个入口，不遍历闭包。
        packaged.matches_app(&self.app_version)
            && installed.build_sha256 == packaged.build_sha256
            && installed.entry_sha256 == packaged.entry_sha256
            && installed.frontend_sha256 == packaged.frontend_sha256
    }

    fn packaged_manifest(&self) -> Result<EmbeddedManifest, String> {
        #[cfg(target_os = "android")]
        {
            let bytes = android_read_asset_prefix(EMBEDDED_ASSET, ARCHIVE_PROBE_BYTES)?;
            return archive_manifest(std::io::Cursor::new(bytes))
                .ok_or_else(|| "Android Runtime 归档首部清单无效。".into());
        }
        #[cfg(any(target_os = "ios", all(test, not(target_os = "android"))))]
        {
            let archive = self
                .resource_archive
                .as_deref()
                .ok_or_else(|| "iOS App 缺少嵌入式 Runtime 资产。".to_string())?;
            let file = File::open(archive)
                .map_err(|error| format!("无法读取 iOS Runtime 归档清单：{error}"))?;
            archive_manifest(file).ok_or_else(|| "iOS Runtime 归档首部清单无效。".into())
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", test)))]
        {
            Err("当前平台没有嵌入式 Runtime 资产。".into())
        }
    }
}

fn trusted_bootstrap_url(value: &str, expected_token: &str) -> bool {
    let Ok(url) = tauri::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.path() == "/_pisper/desktop/bootstrap"
        && url
            .query_pairs()
            .any(|(key, value)| key == "token" && value == expected_token)
}

fn runtime_profile() -> &'static str {
    if cfg!(feature = "mobile-store") {
        "mobile-store"
    } else {
        "mobile-embedded"
    }
}

fn set_runtime_env(name: &str, value: &Path) -> Result<(), String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} 路径不是 UTF-8。"))?;
    std::env::set_var(name, value);
    Ok(())
}

#[cfg(target_os = "android")]
fn platform_supported() -> bool {
    std::env::consts::ARCH == "aarch64"
}

#[cfg(target_os = "ios")]
fn platform_supported() -> bool {
    std::env::consts::ARCH == "aarch64"
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn platform_supported() -> bool {
    false
}

#[cfg(target_os = "android")]
fn packaged_archive_exists(_resource_archive: Option<&Path>) -> bool {
    android_asset_exists(EMBEDDED_ASSET)
}

#[cfg(target_os = "ios")]
fn packaged_archive_exists(resource_archive: Option<&Path>) -> bool {
    resource_archive.is_some_and(Path::is_file)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn packaged_archive_exists(_resource_archive: Option<&Path>) -> bool {
    false
}

#[cfg(target_os = "android")]
fn copy_packaged_archive(_resource_archive: Option<&Path>, target: &Path) -> Result<(), String> {
    android_copy_asset(EMBEDDED_ASSET, target)
}

#[cfg(target_os = "ios")]
fn copy_packaged_archive(resource_archive: Option<&Path>, target: &Path) -> Result<(), String> {
    let source = resource_archive.ok_or_else(|| "iOS App 缺少嵌入式 Runtime 资产。".to_string())?;
    std::fs::copy(source, target)
        .map(|_| ())
        .map_err(|error| format!("无法复制嵌入式 Runtime 资产：{error}"))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn copy_packaged_archive(_resource_archive: Option<&Path>, _target: &Path) -> Result<(), String> {
    Err("当前平台不支持嵌入式 Runtime。".into())
}

#[cfg(target_os = "android")]
fn android_node_started() -> Result<bool, String> {
    with_android_env(|env, context| {
        let class_loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(|error| android_jni_error(env, "无法获取 Android App ClassLoader", error))?;
        let class_name = env
            .new_string("com.lingkongran.pisper.EmbeddedNodeHost")
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 embedded Node 宿主类名：{error}"))?;
        let host_class = env
            .call_method(
                &class_loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[jni::objects::JValue::Object(&class_name)],
            )
            .and_then(|value| value.l())
            .map(jni::objects::JClass::from)
            .map_err(|error| android_jni_error(env, "无法加载 embedded Node 宿主类", error))?;
        env.call_static_method(&host_class, "isStarted", "()Z", &[])
            .and_then(|value| value.z())
            .map_err(|error| android_jni_error(env, "无法读取 embedded Node 宿主状态", error))
    })
}

#[cfg(target_os = "android")]
fn launch_node(arguments: Vec<String>) -> Result<(), String> {
    with_android_env(|env, context| {
        let string_class = env
            .find_class("java/lang/String")
            .map_err(|error| format!("无法加载 Java String：{error}"))?;
        let empty = env
            .new_string("")
            .map_err(|error| format!("无法创建 Node 参数：{error}"))?;
        let array = env
            .new_object_array(arguments.len() as i32, string_class, empty)
            .map_err(|error| format!("无法创建 Node 参数数组：{error}"))?;
        for (index, argument) in arguments.iter().enumerate() {
            let value = env
                .new_string(argument)
                .map_err(|error| format!("无法创建 Node 参数：{error}"))?;
            env.set_object_array_element(&array, index as i32, value)
                .map_err(|error| format!("无法写入 Node 参数：{error}"))?;
        }

        // 附加的 Rust 工作线程没有 App ClassLoader；必须从当前 Context 显式加载宿主类。
        let class_loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|value| value.l())
            .map_err(|error| android_jni_error(env, "无法获取 Android App ClassLoader", error))?;
        let class_name = env
            .new_string("com.lingkongran.pisper.EmbeddedNodeHost")
            .map(jni::objects::JObject::from)
            .map_err(|error| format!("无法构造 embedded Node 宿主类名：{error}"))?;
        let host_class = env
            .call_method(
                &class_loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[jni::objects::JValue::Object(&class_name)],
            )
            .and_then(|value| value.l())
            .map(jni::objects::JClass::from)
            .map_err(|error| android_jni_error(env, "无法加载 embedded Node 宿主类", error))?;
        let result = env
            .call_static_method(
                &host_class,
                "start",
                "([Ljava/lang/String;)Ljava/lang/String;",
                &[jni::objects::JValue::Object(array.as_ref())],
            )
            .and_then(|value| value.l())
            .map_err(|error| android_jni_error(env, "无法启动 Android embedded Node", error))?;
        if result.is_null() {
            return Ok(());
        }
        let message: String = env
            .get_string((&result).into())
            .map_err(|error| format!("无法读取 embedded Node 错误：{error}"))?
            .into();
        Err(format!("无法加载 Android embedded Node：{message}"))
    })
}

#[cfg(target_os = "android")]
fn android_jni_error(
    env: &mut jni::JNIEnv<'_>,
    context: &str,
    error: jni::errors::Error,
) -> String {
    let detail = if env.exception_check().unwrap_or(false) {
        let throwable = env.exception_occurred().ok();
        let _ = env.exception_clear();
        throwable.and_then(|throwable| {
            let value = env
                .call_method(&throwable, "toString", "()Ljava/lang/String;", &[])
                .and_then(|value| value.l())
                .ok()?;
            if value.is_null() {
                return None;
            }
            let value: String = env.get_string((&value).into()).ok()?.into();
            Some(value)
        })
    } else {
        None
    };
    match detail {
        Some(detail) => format!("{context}：{detail}"),
        None => format!("{context}：{error}"),
    }
}

#[cfg(target_os = "ios")]
type NodeStart = unsafe extern "C" fn(i32, *mut *mut c_char) -> i32;

#[cfg(target_os = "ios")]
extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}

#[cfg(any(target_os = "ios", test))]
fn ios_node_mobile_binary(executable: &Path) -> Option<PathBuf> {
    Some(
        executable
            .parent()?
            .join("Frameworks/NodeMobile.framework/NodeMobile"),
    )
}

#[cfg(target_os = "ios")]
fn ios_dynamic_link_error() -> String {
    let message = unsafe { dlerror() };
    if message.is_null() {
        return "未知动态链接错误".into();
    }
    unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(target_os = "ios")]
fn load_ios_node_start() -> Result<NodeStart, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("无法定位 iOS App 可执行文件：{error}"))?;
    let binary = ios_node_mobile_binary(&executable)
        .ok_or_else(|| "无法定位 iOS NodeMobile framework。".to_string())?;
    if !binary.is_file() {
        return Err(format!(
            "iOS App 缺少 NodeMobile framework：{}",
            binary.display()
        ));
    }
    let path = binary
        .to_str()
        .ok_or_else(|| "iOS NodeMobile framework 路径不是 UTF-8。".to_string())?;
    let path =
        CString::new(path).map_err(|_| "iOS NodeMobile framework 路径包含空字符。".to_string())?;
    let symbol = CString::new("node_start").expect("static NodeMobile symbol is valid");

    unsafe {
        dlerror();
        let handle = dlopen(path.as_ptr(), 0x2 | 0x4);
        if handle.is_null() {
            return Err(format!(
                "无法加载 iOS NodeMobile framework：{}",
                ios_dynamic_link_error()
            ));
        }
        dlerror();
        let address = dlsym(handle, symbol.as_ptr());
        if address.is_null() {
            let error = ios_dynamic_link_error();
            dlclose(handle);
            return Err(format!("iOS NodeMobile framework 缺少 node_start：{error}"));
        }
        // Node 在 App 生命周期内常驻，保留 framework handle，不能提前 dlclose。
        Ok(std::mem::transmute::<*mut c_void, NodeStart>(address))
    }
}

#[cfg(target_os = "ios")]
fn launch_node(arguments: Vec<String>, node_thread_alive: Arc<AtomicBool>) -> Result<(), String> {
    let node_start = load_ios_node_start()?;
    node_thread_alive.store(true, Ordering::Release);
    let alive = node_thread_alive.clone();
    std::thread::Builder::new()
        .name("pisper-embedded-node".into())
        .spawn(move || {
            let owned = arguments
                .into_iter()
                .map(CString::new)
                .collect::<Result<Vec<_>, _>>();
            let Ok(mut owned) = owned else {
                alive.store(false, Ordering::Release);
                return;
            };
            let mut argv = owned
                .iter_mut()
                .map(|value| value.as_ptr().cast_mut())
                .collect::<Vec<_>>();
            // NodeMobile 明确要求在非 WebView 线程进入 node_start。
            unsafe {
                node_start(argv.len() as i32, argv.as_mut_ptr());
            }
            alive.store(false, Ordering::Release);
        })
        .map(|_| ())
        .map_err(|error| {
            node_thread_alive.store(false, Ordering::Release);
            format!("无法创建 iOS embedded Node 线程：{error}")
        })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn launch_node(_arguments: Vec<String>) -> Result<(), String> {
    Err("当前平台不支持嵌入式 Node。".into())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};
    use std::path::{Path, PathBuf};

    use flate2::{write::GzEncoder, Compression};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    use super::{
        archive_manifest, archive_sha256, ios_node_mobile_binary, publish_installation,
        read_installed_manifest, runtime_profile, trusted_bootstrap_url, EmbeddedRuntime,
        ARCHIVE_PROBE_BYTES, MAX_MANIFEST_BYTES,
    };

    const ENTRY: &[u8] = b"export const runtime = true;";
    const FRONTEND: &[u8] = b"<!doctype html><div id='root'></div>";

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "pisper-embedded-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn runtime(&self, archive: PathBuf) -> EmbeddedRuntime {
            EmbeddedRuntime::new(
                self.0.join("embedded-runtime"),
                self.0.join("data"),
                "1.2.3".into(),
                Some(archive),
            )
        }

        fn archive(&self, manifest: &Value) -> PathBuf {
            let path = self.0.join("runtime.tgz");
            std::fs::write(&path, archive_bytes(manifest, true)).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest() -> Value {
        json!({
            "schemaVersion": 1,
            "appVersion": "1.2.3",
            "runtimeProfile": runtime_profile(),
            "entry": "runtime/mobile-embedded.mjs",
            "buildSha256": "a".repeat(64),
            "entrySha256": format!("{:x}", Sha256::digest(ENTRY)),
            "frontendSha256": format!("{:x}", Sha256::digest(FRONTEND)),
        })
    }

    fn tar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn archive_bytes(manifest: &Value, with_entry: bool) -> Vec<u8> {
        let serialized = serde_json::to_vec(manifest).unwrap();
        let mut entries = vec![
            ("./embedded-runtime.json", serialized.as_slice()),
            ("dist/index.html", FRONTEND),
        ];
        if with_entry {
            entries.push(("runtime/mobile-embedded.mjs", ENTRY));
        }
        tar_bytes(&entries)
    }

    #[test]
    fn archive_probe_reads_only_the_first_manifest_with_bounded_io() {
        struct Counted<'a> {
            source: Cursor<Vec<u8>>,
            count: &'a mut usize,
        }
        impl Read for Counted<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let size = self.source.read(buffer)?;
                *self.count += size;
                Ok(size)
            }
        }
        let serialized = serde_json::to_vec(&manifest()).unwrap();
        let noise: Vec<u8> = (0..256 * 1024).map(|_| rand::random()).collect();
        let bytes = tar_bytes(&[
            ("./embedded-runtime.json", &serialized),
            ("large.bin", &noise),
        ]);
        assert!(bytes.len() > ARCHIVE_PROBE_BYTES);
        let mut count = 0;
        let parsed = archive_manifest(Counted {
            source: Cursor::new(bytes),
            count: &mut count,
        })
        .unwrap();
        assert!(parsed.matches_app("1.2.3"));
        assert!(count <= ARCHIVE_PROBE_BYTES);
        assert!(archive_manifest(Cursor::new(tar_bytes(&[
            ("large.bin", &noise),
            ("./embedded-runtime.json", &serialized),
        ])))
        .is_none());
    }

    #[test]
    fn archive_probe_rejects_oversize_truncated_invalid_and_nonfile_manifests() {
        let oversized = vec![b' '; MAX_MANIFEST_BYTES as usize + 1];
        assert!(archive_manifest(Cursor::new(tar_bytes(&[(
            "embedded-runtime.json",
            &oversized
        ),])))
        .is_none());
        assert!(archive_manifest(Cursor::new(b"not gzip")).is_none());
        assert!(
            archive_manifest(Cursor::new(tar_bytes(&[("embedded-runtime.json", b"{"),]))).is_none()
        );
        let bytes = archive_bytes(&manifest(), true);
        assert!(archive_manifest(Cursor::new(&bytes[..16])).is_none());
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "embedded-runtime.json", std::io::empty())
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        assert!(archive_manifest(Cursor::new(bytes)).is_none());
    }

    #[test]
    fn installed_build_matches_only_a_valid_packaged_contract() {
        let fixture = Fixture::new();
        let original = manifest();
        let archive = fixture.archive(&original);
        let runtime = fixture.runtime(archive.clone());
        runtime.install_archive(&archive).unwrap();
        assert!(runtime.installed_matches_packaged_runtime(true));
        assert!(runtime.installed_matches_packaged_runtime(false));
        assert!(!fixture.0.join("embedded-runtime.staging").exists());
        assert!(!fixture.0.join("embedded-runtime.previous").exists());
        for (field, value) in [
            ("buildSha256", json!("b".repeat(64))),
            ("schemaVersion", json!(2)),
            ("appVersion", json!("2.0.0")),
            ("runtimeProfile", json!("desktop")),
            ("entry", json!("runtime/other.mjs")),
            ("buildSha256", json!("G".repeat(64))),
            ("entrySha256", json!("b".repeat(64))),
            ("frontendSha256", json!("b".repeat(64))),
            ("entrySha256", json!("invalid")),
            ("frontendSha256", json!("invalid")),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            fixture.archive(&changed);
            assert!(!runtime.installed_matches_packaged_runtime(true), "{field}");
        }
        std::fs::write(&archive, b"broken").unwrap();
        assert!(!runtime.installed_matches_packaged_runtime(true));
    }

    #[test]
    fn legacy_installation_without_build_migrates_once_through_full_install() {
        let fixture = Fixture::new();
        let archive = fixture.archive(&manifest());
        let runtime = fixture.runtime(archive.clone());
        runtime.install_archive(&archive).unwrap();
        let marker = runtime.root.join("embedded-runtime.json");
        let mut legacy: Value = serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("buildSha256");
        std::fs::write(&marker, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(!runtime.installed_matches_packaged_runtime(true));
        runtime.install_archive(&archive).unwrap();
        let installed = read_installed_manifest(&runtime.root, "1.2.3").unwrap();
        assert_eq!(
            installed.archive_sha256.unwrap(),
            archive_sha256(&archive).unwrap()
        );
        assert!(runtime.installed_matches_packaged_runtime(true));
    }

    #[test]
    fn fast_path_rejects_missing_empty_and_nonempty_corrupted_entries() {
        for relative in ["runtime/mobile-embedded.mjs", "dist/index.html"] {
            let fixture = Fixture::new();
            let archive = fixture.archive(&manifest());
            let runtime = fixture.runtime(archive.clone());
            for damage in [Some(b"corrupt".as_slice()), Some(b"".as_slice()), None] {
                runtime.install_archive(&archive).unwrap();
                let path = runtime.root.join(relative);
                match damage {
                    Some(contents) => std::fs::write(path, contents).unwrap(),
                    None => std::fs::remove_file(path).unwrap(),
                }
                assert!(
                    !runtime.installed_matches_packaged_runtime(true),
                    "{relative}"
                );
            }
        }
    }

    #[test]
    fn failed_archive_validation_preserves_the_old_installation() {
        let fixture = Fixture::new();
        let original = manifest();
        let archive = fixture.archive(&original);
        let runtime = fixture.runtime(archive.clone());
        runtime.install_archive(&archive).unwrap();
        let old_marker = std::fs::read(runtime.root.join("embedded-runtime.json")).unwrap();
        let mut invalid_archives = vec![archive_bytes(&original, false), b"broken".to_vec()];
        for (field, value) in [
            ("schemaVersion", json!(0)),
            ("appVersion", json!("0.0.0")),
            ("runtimeProfile", json!("desktop")),
            ("entry", json!("wrong.mjs")),
            ("buildSha256", json!("bad")),
            ("entrySha256", json!("b".repeat(64))),
            ("frontendSha256", json!("b".repeat(64))),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            invalid_archives.push(archive_bytes(&changed, true));
        }
        let mut damaged_crc = archive_bytes(&original, true);
        let last = damaged_crc.len() - 8;
        damaged_crc[last] ^= 0xff;
        invalid_archives.push(damaged_crc);
        let mut truncated = archive_bytes(&original, true);
        truncated.truncate(truncated.len() - 8);
        invalid_archives.push(truncated);
        for bytes in invalid_archives {
            std::fs::write(&archive, bytes).unwrap();
            assert!(runtime.install_archive(&archive).is_err());
            assert_eq!(
                std::fs::read(runtime.root.join("embedded-runtime.json")).unwrap(),
                old_marker
            );
            assert_eq!(
                std::fs::read(runtime.root.join("runtime/mobile-embedded.mjs")).unwrap(),
                ENTRY
            );
            assert!(!fixture.0.join("embedded-runtime.staging").exists());
        }
    }

    #[test]
    fn failed_publication_rolls_back_and_interrupted_installation_is_recovered() {
        let fixture = Fixture::new();
        let archive = fixture.archive(&manifest());
        let runtime = fixture.runtime(archive.clone());
        runtime.install_archive(&archive).unwrap();
        let previous = fixture.0.join("embedded-runtime.previous");
        assert!(
            publish_installation(&runtime.root, &fixture.0.join("missing-staging"), &previous)
                .is_err()
        );
        assert!(read_installed_manifest(&runtime.root, "1.2.3").is_some());
        assert!(!previous.exists());
        std::fs::rename(&runtime.root, &previous).unwrap();
        std::fs::write(&archive, b"invalid archive").unwrap();
        assert!(runtime.install_archive(&archive).is_err());
        assert!(read_installed_manifest(&runtime.root, "1.2.3").is_some());
    }

    #[test]
    fn oversized_installed_marker_cannot_hide_trailing_bytes() {
        let fixture = Fixture::new();
        let archive = fixture.archive(&manifest());
        let runtime = fixture.runtime(archive.clone());
        runtime.install_archive(&archive).unwrap();
        let mut marker = std::fs::OpenOptions::new()
            .append(true)
            .open(runtime.root.join("embedded-runtime.json"))
            .unwrap();
        marker
            .write_all(&vec![b' '; MAX_MANIFEST_BYTES as usize])
            .unwrap();
        assert!(!runtime.installed_matches_packaged_runtime(true));
    }

    #[test]
    fn ios_node_mobile_binary_resolves_inside_the_app_frameworks_directory() {
        assert_eq!(
            ios_node_mobile_binary(Path::new("/tmp/Pisper.app/Pisper")),
            Some(Path::new("/tmp/Pisper.app/Frameworks/NodeMobile.framework/NodeMobile").into()),
        );
    }

    #[test]
    fn runtime_profile_is_bound_to_the_store_feature() {
        assert_eq!(
            runtime_profile(),
            if cfg!(feature = "mobile-store") {
                "mobile-store"
            } else {
                "mobile-embedded"
            },
        );
    }

    #[test]
    fn embedded_ready_accepts_only_the_expected_authenticated_loopback_url() {
        assert!(trusted_bootstrap_url(
            "http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=secret",
            "secret",
        ));
        assert!(!trusted_bootstrap_url(
            "http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=other",
            "secret",
        ));
        assert!(!trusted_bootstrap_url(
            "http://localhost:41873/_pisper/desktop/bootstrap?token=secret",
            "secret",
        ));
        assert!(!trusted_bootstrap_url("http://127.0.0.1:41873/", "secret",));
        assert!(!trusted_bootstrap_url(
            "https://example.com/_pisper/desktop/bootstrap?token=secret",
            "secret",
        ));
    }
}
