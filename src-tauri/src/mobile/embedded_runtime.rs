#[cfg(target_os = "ios")]
use std::ffi::CString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::read::GzDecoder;
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;

use super::root_runtime::RootRuntimeStatus;

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
        let installed = installed_version(&root).as_deref() == Some(app_version.as_str());
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
            return Ok(current);
        }
        if self.started.load(Ordering::Acquire) {
            return Err(if current.message.is_empty() {
                "嵌入式 Node 已启动，但本机 Runtime 尚未就绪。".into()
            } else {
                current.message
            });
        }
        if !current.supported {
            return Err(current.message);
        }
        if !current.packaged && !current.installed {
            return Err("安装包未包含嵌入式 Node Runtime。".into());
        }

        self.update_status("starting", "正在准备本机 Runtime…");
        if installed_version(&self.root).as_deref() != Some(self.app_version.as_str()) {
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
                    self.fail(&error);
                    return Err(error);
                }
            }
        } else {
            return Err("嵌入式 Node 已启动，但本机 Runtime 尚未就绪。".into());
        };

        match self.wait_until_ready(&token) {
            Ok(ready) => {
                let mut status = self
                    .status
                    .lock()
                    .expect("embedded Runtime status mutex poisoned");
                status.installed = true;
                status.running = true;
                status.state = "running".into();
                status.message.clear();
                status.url = ready.bootstrap_url;
                Ok(status.clone())
            }
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
        let unpack_result = unpack_archive(&archive, &staging)
            .and_then(|_| validate_installation(&staging, &self.app_version));
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
        std::env::set_var("PISPER_RUNTIME_PROFILE", "mobile-embedded");
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
                        || ready.runtime_profile != "mobile-embedded"
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
}

fn unpack_archive(archive: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|error| format!("无法打开嵌入式 Runtime：{error}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(target)
        .map_err(|error| format!("无法解压嵌入式 Runtime：{error}"))
}

fn validate_installation(root: &Path, app_version: &str) -> Result<(), String> {
    if !root.join("runtime/mobile-embedded.mjs").is_file()
        || !root.join("dist/index.html").is_file()
    {
        return Err("嵌入式 Runtime 资产缺少入口或 React 资源。".into());
    }
    if installed_version(root).as_deref() != Some(app_version) {
        return Err("嵌入式 Runtime 版本与 App 不匹配。".into());
    }
    Ok(())
}

fn installed_version(root: &Path) -> Option<String> {
    let value = std::fs::read(root.join("embedded-runtime.json")).ok()?;
    serde_json::from_slice::<serde_json::Value>(&value)
        .ok()?
        .get("appVersion")?
        .as_str()
        .map(ToOwned::to_owned)
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
    super::root_runtime::android_asset_exists(EMBEDDED_ASSET)
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
    super::root_runtime::android_copy_asset(EMBEDDED_ASSET, target)
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
fn launch_node(arguments: Vec<String>) -> Result<(), String> {
    super::root_runtime::with_android_env(|env, context| {
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
fn launch_node(arguments: Vec<String>) -> Result<(), String> {
    extern "C" {
        fn node_start(argc: i32, argv: *mut *mut std::ffi::c_char) -> i32;
    }
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
    use super::trusted_bootstrap_url;

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
