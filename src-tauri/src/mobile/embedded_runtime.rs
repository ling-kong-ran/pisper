#[cfg(target_os = "ios")]
use std::ffi::{c_void, CStr, CString};
use std::fs::File;
use std::io::Read;
#[cfg(target_os = "ios")]
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::read::GzDecoder;
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(target_os = "android")]
use super::android_bridge::{android_asset_exists, android_copy_asset, with_android_env};
use super::runtime_status::RootRuntimeStatus;

// Android 的 aapt 会自动解压并改名 `.gz` 资产；使用 `.tgz` 才能让 Rust 收到原始 gzip。
#[cfg(target_os = "android")]
const EMBEDDED_ASSET: &str = "pisper-embedded-runtime.tgz";
#[cfg(not(target_os = "android"))]
const EMBEDDED_ASSET: &str = "pisper-embedded-runtime.tar.gz";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ARCHIVE_BYTES: u64 = 180 * 1024 * 1024;

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
    status: Mutex<RootRuntimeStatus>,
    started: AtomicBool,
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
        let installed = installed_version(&root).as_deref() == Some(app_version.as_str())
            && installed_profile(&root).as_deref() == Some(runtime_profile());
        let status = RootRuntimeStatus {
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
            token: Mutex::new(None),
        }
    }

    pub fn status(&self) -> RootRuntimeStatus {
        self.status
            .lock()
            .expect("embedded Runtime status mutex poisoned")
            .clone()
    }

    pub fn ensure_started(&self) -> Result<RootRuntimeStatus, String> {
        let current = self.status();
        if current.running {
            if self.node_is_alive()? {
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
        if !self.installed_matches_packaged_runtime(current.packaged) {
            if let Err(error) = self.install() {
                self.fail(&error);
                return Err(error);
            }
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
        copy_packaged_archive(self.resource_archive.as_deref(), &archive)?;
        let size = std::fs::metadata(&archive)
            .map_err(|error| format!("无法读取嵌入式 Runtime 资产：{error}"))?
            .len();
        if size == 0 || size > MAX_ARCHIVE_BYTES {
            let _ = std::fs::remove_file(&archive);
            return Err("嵌入式 Runtime 资产大小无效。".into());
        }

        let staging = parent.join("embedded-runtime.staging");
        let previous = parent.join("embedded-runtime.previous");
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&previous);
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("无法创建嵌入式 Runtime staging：{error}"))?;
        let archive_fingerprint = archive_sha256(&archive)?;
        let unpack_result = unpack_archive(&archive, &staging)
            .and_then(|_| record_archive_fingerprint(&staging, &archive_fingerprint))
            .and_then(|_| validate_installation(&staging, &self.app_version, &archive_fingerprint));
        let _ = std::fs::remove_file(&archive);
        if let Err(error) = unpack_result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }

        if self.root.exists() {
            std::fs::rename(&self.root, &previous)
                .map_err(|error| format!("无法暂存旧嵌入式 Runtime：{error}"))?;
        }
        if let Err(error) = std::fs::rename(&staging, &self.root) {
            if previous.exists() {
                let _ = std::fs::rename(&previous, &self.root);
            }
            return Err(format!("无法安装嵌入式 Runtime：{error}"));
        }
        let _ = std::fs::remove_dir_all(previous);
        self.status
            .lock()
            .expect("embedded Runtime status mutex poisoned")
            .installed = true;
        Ok(())
    }

    fn start_node(&self) -> Result<String, String> {
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
        launch_node(vec!["node".into(), "--no-warnings".into(), entry.into()])?;
        Ok(token)
    }

    fn wait_until_ready(&self, token: &str) -> Result<EmbeddedReady, String> {
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
                    return Ok(ready);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(format!("无法读取嵌入式 Runtime READY：{error}")),
            }
        }
        Err("嵌入式 Runtime 启动超时。".into())
    }

    fn mark_ready(&self, ready: EmbeddedReady) -> RootRuntimeStatus {
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
        #[cfg(not(target_os = "android"))]
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

fn unpack_archive(archive: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| format!("无法打开嵌入式 Runtime：{error}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(target)
        .map_err(|error| format!("无法解压嵌入式 Runtime：{error}"))
}

fn validate_installation(
    root: &Path,
    app_version: &str,
    archive_fingerprint: &str,
) -> Result<(), String> {
    if !root.join("runtime/mobile-embedded.mjs").is_file()
        || !root.join("dist/index.html").is_file()
    {
        return Err("嵌入式 Runtime 资产缺少入口或 React 资源。".into());
    }
    if installed_version(root).as_deref() != Some(app_version) {
        return Err("嵌入式 Runtime 版本与 App 不匹配。".into());
    }
    if installed_profile(root).as_deref() != Some(runtime_profile()) {
        return Err("嵌入式 Runtime 档案与 App 构建模式不匹配。".into());
    }
    if installed_archive_fingerprint(root).as_deref() != Some(archive_fingerprint) {
        return Err("嵌入式 Runtime 归档指纹不匹配。".into());
    }
    Ok(())
}

fn record_archive_fingerprint(root: &Path, fingerprint: &str) -> Result<(), String> {
    let marker = root.join("embedded-runtime.json");
    let contents =
        std::fs::read(&marker).map_err(|error| format!("无法读取嵌入式 Runtime 清单：{error}"))?;
    let mut value: serde_json::Value = serde_json::from_slice(&contents)
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

fn installed_version(root: &Path) -> Option<String> {
    installed_manifest_value(root, "appVersion")
}

fn installed_profile(root: &Path) -> Option<String> {
    installed_manifest_value(root, "runtimeProfile")
}

fn installed_archive_fingerprint(root: &Path) -> Option<String> {
    installed_manifest_value(root, "archiveSha256")
}

fn installed_manifest_value(root: &Path, key: &str) -> Option<String> {
    let value = std::fs::read(root.join("embedded-runtime.json")).ok()?;
    serde_json::from_slice::<serde_json::Value>(&value)
        .ok()?
        .get(key)?
        .as_str()
        .map(ToOwned::to_owned)
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
        let version_matches =
            installed_version(&self.root).as_deref() == Some(self.app_version.as_str());
        let profile_matches = installed_profile(&self.root).as_deref() == Some(runtime_profile());
        if !version_matches || !profile_matches {
            return false;
        }
        if !packaged {
            return true;
        }
        let Ok(expected) = self.packaged_archive_fingerprint() else {
            return false;
        };
        installed_archive_fingerprint(&self.root).as_deref() == Some(expected.as_str())
    }

    fn packaged_archive_fingerprint(&self) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            std::fs::create_dir_all(&self.data_root)
                .map_err(|error| format!("无法创建 Runtime 指纹目录：{error}"))?;
            let probe = self.data_root.join("embedded-runtime-fingerprint.tgz");
            let result =
                android_copy_asset(EMBEDDED_ASSET, &probe).and_then(|_| archive_sha256(&probe));
            let _ = std::fs::remove_file(&probe);
            return result;
        }
        #[cfg(target_os = "ios")]
        {
            let archive = self
                .resource_archive
                .as_deref()
                .ok_or_else(|| "iOS App 缺少嵌入式 Runtime 资产。".to_string())?;
            return archive_sha256(archive);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
fn launch_node(arguments: Vec<String>) -> Result<(), String> {
    let node_start = load_ios_node_start()?;
    std::thread::Builder::new()
        .name("pisper-embedded-node".into())
        .spawn(move || {
            let owned = arguments
                .into_iter()
                .map(CString::new)
                .collect::<Result<Vec<_>, _>>();
            let Ok(mut owned) = owned else {
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
        })
        .map(|_| ())
        .map_err(|error| format!("无法创建 iOS embedded Node 线程：{error}"))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn launch_node(_arguments: Vec<String>) -> Result<(), String> {
    Err("当前平台不支持嵌入式 Node。".into())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ios_node_mobile_binary, runtime_profile, trusted_bootstrap_url};

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
