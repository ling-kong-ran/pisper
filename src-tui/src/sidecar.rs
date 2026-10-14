//! sidecar（Node 运行时进程）的定位、启动与生命周期管理。
//!
//! 连接来源按优先级分为三种：
//! 1. 环境变量 `PISPER_TUI_URL`（远程/外部托管的运行时）；
//! 2. 桌面版 sidecar 描述文件（`desktop-sidecar.json`，由桌面壳写入）；
//! 3. 本进程 spawn 的 TUI sidecar（独立 TUI 发行）。
//!
//! 独立发行时优先使用已安装的签名 Runtime 组件，失败后回退到随二进制
//! 捆绑的 sidecar；两种都没有才在开发模式下用 `node runtime/sidecar.mjs`。

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use pisper_component_updater::{resolve_installed, Component};
use semver::Version;
use serde::Deserialize;

// sidecar 就绪时在 stdout 打印的前缀，TUI 据此解析监听地址。
const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
// 启动等待就绪的超时。
const START_TIMEOUT: Duration = Duration::from_secs(10);
// 等待期间检查子进程是否提前退出的轮询间隔。
const EXIT_CHECK_INTERVAL: Duration = Duration::from_millis(50);
// 桌面版 sidecar 描述文件名（位于系统数据目录下的应用目录中）。
const SIDECAR_DESCRIPTOR_NAME: &str = "desktop-sidecar.json";
pub const APP_IDENTIFIER: &str = "com.lingkongran.pisper";

/// sidecar 就绪负载：监听 URL。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyPayload {
    url: String,
}

/// 桌面版写入的描述文件：提供 sidecar 的 URL、访问令牌与进程号。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarDescriptor {
    version: u8,
    url: String,
    token: String,
    pid: u32,
}

/// sidecar 命令的类别：已安装组件 vs 其他来源，
/// 用于启动失败时决定是否值得回退到捆绑运行时。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SidecarCommandKind {
    Installed,
    Other,
}

/// sidecar 连接来源分类（用于 doctor 输出与诊断）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionKind {
    /// 来自 `PISPER_TUI_URL` 的外部运行时。
    Remote,
    /// 桌面版托管、经描述文件发现的 sidecar。
    Desktop,
    /// 本进程 spawn 的 sidecar。
    Spawned,
}

impl ConnectionKind {
    /// 连接来源的展示标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Remote => "PISPER_TUI_URL",
            Self::Desktop => "desktop sidecar",
            Self::Spawned => "TUI sidecar",
        }
    }
}

/// 一个已就绪的 sidecar 连接（自 spawn 时还持有子进程句柄，用于优雅关闭）。
pub struct SidecarConnection {
    pub url: String,
    pub token: String,
    pub kind: ConnectionKind,
    child: Option<Child>,
}

impl SidecarConnection {
    /// 按优先级选择连接来源并启动（或复用）sidecar。
    /// 自 spawn 路径在就绪前会等待 `PISPER_SIDECAR_READY` 输出，
    /// 若已安装的签名运行时启动失败，自动停用该组件并回退到捆绑运行时。
    pub fn start(workspace: &Path) -> Result<Self> {
        if let Ok(url) = std::env::var("PISPER_TUI_URL") {
            let token = std::env::var("PISPER_TUI_TOKEN")
                .context("PISPER_TUI_TOKEN is required with PISPER_TUI_URL")?;
            return Ok(Self {
                url,
                token,
                kind: ConnectionKind::Remote,
                child: None,
            });
        }
        if let Some(descriptor) = desktop_sidecar_descriptor() {
            return Ok(Self {
                url: descriptor.url,
                token: descriptor.token,
                kind: ConnectionKind::Desktop,
                child: None,
            });
        }

        let token = secure_token()?;
        let (command, app_root, command_kind) = sidecar_command(true)?;
        let first = spawn_sidecar(command, app_root, workspace, &token);
        let (child, ready) = match first {
            Ok(result) => result,
            Err(component_error) if command_kind == SidecarCommandKind::Installed => {
                eprintln!(
                    "Installed Pisper runtime failed; using bundled runtime: {component_error:#}"
                );
                if let Ok(root) = components_root() {
                    let _ =
                        pisper_component_updater::deactivate_component(&root, Component::Runtime);
                }
                let (fallback, fallback_root, _) = sidecar_command(false)?;
                spawn_sidecar(fallback, fallback_root, workspace, &token).with_context(|| {
                    format!(
                        "installed runtime failed ({component_error:#}); bundled runtime also failed"
                    )
                })?
            }
            Err(error) => return Err(error),
        };

        Ok(Self {
            url: ready.url,
            token,
            kind: ConnectionKind::Spawned,
            child: Some(child),
        })
    }

    /// 优雅关闭自 spawn 的 sidecar：先向 stdin 写 `shutdown` 让运行时自行收尾，
    /// 限时 5 秒；超时未退出才强杀。
    pub fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"shutdown\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// sidecar 启动期的 stdout 事件。
enum StartupEvent {
    Ready(Result<ReadyPayload>),
    StdoutClosed,
}

/// 以子进程方式启动 sidecar 并等待就绪。
/// 环境变量用于把工作区、令牌、退出条件等注入运行时；
/// 同时后台收集 stderr 诊断输出，供启动失败时给出可读的错误信息。
fn spawn_sidecar(
    mut command: Command,
    app_root: PathBuf,
    workspace: &Path,
    token: &str,
) -> Result<(Child, ReadyPayload)> {
    command
        .current_dir(workspace)
        .env("PISPER_APP_ROOT", app_root)
        .env("PISPER_DESKTOP_TOKEN", token)
        .env("PISPER_PARENT_PID", std::process::id().to_string())
        .env("PISPER_EXIT_ON_STDIN_CLOSE", "1")
        .env("PISPER_STARTUP_TIMING", "1")
        .env("PISPER_WORKSPACE_DIR", workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(frontend_root) = configured_frontend_root() {
        command.env("PISPER_FRONTEND_ROOT", frontend_root);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let mut child = command.spawn().context("failed to start Pisper sidecar")?;
    let stdout = child
        .stdout
        .take()
        .context("sidecar stdout was not captured")?;
    let stderr = child
        .stderr
        .take()
        .context("sidecar stderr was not captured")?;
    let diagnostics = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr_diagnostics = diagnostics.clone();
    let mut stderr_thread = Some(thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut lines = stderr_diagnostics
                .lock()
                .expect("sidecar diagnostics poisoned");
            lines.push(line);
            if lines.len() > 40 {
                lines.remove(0);
            }
        }
    }));

    let (ready_tx, ready_rx) = mpsc::sync_channel(2);
    thread::spawn(move || {
        let mut ready_sent = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !ready_sent {
                if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                    let parsed = serde_json::from_str::<ReadyPayload>(payload)
                        .map_err(|error| anyhow!("invalid sidecar readiness payload: {error}"));
                    let valid = parsed.is_ok();
                    let _ = ready_tx.send(StartupEvent::Ready(parsed));
                    ready_sent = valid;
                }
            }
        }
        if !ready_sent {
            let _ = ready_tx.send(StartupEvent::StdoutClosed);
        }
    });

    let started = Instant::now();
    let deadline = started + START_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(stop_with_startup_error(
                &mut child,
                &mut stderr_thread,
                format!(
                    "did not become ready within {:.0} seconds",
                    START_TIMEOUT.as_secs_f32()
                ),
                started.elapsed(),
                &diagnostics,
            ));
        }
        match ready_rx.recv_timeout(remaining.min(EXIT_CHECK_INTERVAL)) {
            Ok(StartupEvent::Ready(Ok(ready))) => return Ok((child, ready)),
            Ok(StartupEvent::Ready(Err(error))) => {
                return Err(stop_with_startup_error(
                    &mut child,
                    &mut stderr_thread,
                    error.to_string(),
                    started.elapsed(),
                    &diagnostics,
                ));
            }
            Ok(StartupEvent::StdoutClosed) => {
                let status = child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "without an exit status".to_owned());
                return Err(stop_with_startup_error(
                    &mut child,
                    &mut stderr_thread,
                    format!("closed its output before readiness ({status})"),
                    started.elapsed(),
                    &diagnostics,
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child
                    .try_wait()
                    .context("failed to inspect Pisper sidecar")?
                {
                    return Err(stop_with_startup_error(
                        &mut child,
                        &mut stderr_thread,
                        format!("exited before readiness ({status})"),
                        started.elapsed(),
                        &diagnostics,
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(stop_with_startup_error(
                    &mut child,
                    &mut stderr_thread,
                    "stopped reporting startup status".to_owned(),
                    started.elapsed(),
                    &diagnostics,
                ));
            }
        }
    }
}

/// 停止子进程：先尝试回收（已退出则无需 kill），未退出则强杀，
/// 最后 wait 回收僵尸进程。
fn stop_child(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// 启动失败的统一收尾：停止子进程、回收 stderr 线程，
/// 然后把失败原因连同收集到的诊断输出一并构造为错误。
fn stop_with_startup_error(
    child: &mut Child,
    stderr_thread: &mut Option<thread::JoinHandle<()>>,
    reason: String,
    elapsed: Duration,
    diagnostics: &Arc<Mutex<Vec<String>>>,
) -> anyhow::Error {
    stop_child(child);
    if let Some(stderr_thread) = stderr_thread.take() {
        let _ = stderr_thread.join();
    }
    startup_error(reason, elapsed, diagnostics)
}

/// 组装启动失败错误：包含原因、耗时与子进程的 stderr 诊断。
fn startup_error(
    reason: String,
    elapsed: Duration,
    diagnostics: &Arc<Mutex<Vec<String>>>,
) -> anyhow::Error {
    let detail = diagnostics
        .lock()
        .expect("sidecar diagnostics poisoned")
        .join("\n");
    anyhow!(
        "Pisper sidecar {reason} after {:.2}s{}",
        elapsed.as_secs_f32(),
        if detail.is_empty() {
            "\nNo Runtime diagnostics were produced.".to_owned()
        } else {
            format!("\n{detail}")
        }
    )
}

/// 连接被丢弃时自动关闭 sidecar，避免遗留孤儿进程。
impl Drop for SidecarConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 发现并验证桌面版 sidecar 描述文件。
/// 校验严格：版本号、令牌长度、回环地址都须匹配，且端口必须真的可连接；
/// 描述文件失效（进程已退出）时直接删除，避免下次启动继续命中坏描述。
fn desktop_sidecar_descriptor() -> Option<SidecarDescriptor> {
    let path = dirs::data_local_dir()?
        .join(APP_IDENTIFIER)
        .join(SIDECAR_DESCRIPTOR_NAME);
    let descriptor = serde_json::from_slice::<SidecarDescriptor>(&fs::read(&path).ok()?).ok()?;
    let url = url::Url::parse(&descriptor.url).ok()?;
    if descriptor.version != 1
        || descriptor.pid == 0
        || descriptor.token.len() < 32
        || url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
    {
        return None;
    }
    let address = SocketAddr::from(([127, 0, 0, 1], url.port()?));
    if TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_err() {
        let _ = fs::remove_file(path);
        return None;
    }
    Some(descriptor)
}

/// 解析 sidecar 启动命令，按优先级：
/// `PISPER_RUNTIME_NODE` → `PISPER_SIDECAR_PATH` → 已安装签名运行时 →
/// 随可执行文件捆绑的 sidecar → 开发模式下的 `node runtime/sidecar.mjs`。
fn sidecar_command(allow_installed: bool) -> Result<(Command, PathBuf, SidecarCommandKind)> {
    if let Some(node) = std::env::var_os("PISPER_RUNTIME_NODE") {
        let root = std::env::var_os("PISPER_APP_ROOT")
            .map(PathBuf::from)
            .context("PISPER_APP_ROOT is required with PISPER_RUNTIME_NODE")?;
        return runtime_node_command(node, root)
            .map(|(command, root)| (command, root, SidecarCommandKind::Other));
    }
    if std::env::var_os("PISPER_SIDECAR_PATH").is_some() {
        return configured_sea_command()
            .map(|(command, root)| (command, root, SidecarCommandKind::Other));
    }

    if allow_installed {
        if let Some(installed) = installed_runtime() {
            return Ok((
                Command::new(installed.executable()),
                installed
                    .runtime_root()
                    .context("installed runtime payload is missing")?,
                SidecarCommandKind::Installed,
            ));
        }
    }

    let current_exe = std::env::current_exe().context("failed to locate pisper executable")?;
    let executable_dir = current_exe
        .parent()
        .context("pisper executable has no parent directory")?;
    let bundled_sidecars = [
        executable_dir.join(platform_sidecar_name()),
        executable_dir
            .parent()
            .unwrap_or(executable_dir)
            .join(platform_sidecar_name()),
    ];
    if !cfg!(debug_assertions) {
        if let Some(sidecar) = bundled_sidecars.into_iter().find(|path| path.is_file()) {
            let root = sibling_runtime(&sidecar);
            return Ok((Command::new(sidecar), root, SidecarCommandKind::Other));
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("src-tui must have a project parent")?
        .to_path_buf();
    let entry = root.join("runtime").join("sidecar.mjs");
    if !entry.is_file() {
        return Err(anyhow!(
            "pisper-sidecar was not found next to the CLI and the development sidecar is unavailable"
        ));
    }
    let mut command = Command::new("node");
    command.arg(entry);
    Ok((command, root, SidecarCommandKind::Other))
}

/// 由 `PISPER_SIDECAR_PATH` 指向的 SEA 可执行文件构造命令。
fn configured_sea_command() -> Result<(Command, PathBuf)> {
    let executable = std::env::var_os("PISPER_SIDECAR_PATH")
        .map(PathBuf::from)
        .context("PISPER_SIDECAR_PATH is not configured")?;
    let root = std::env::var_os("PISPER_APP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| sibling_runtime(&executable));
    Ok((Command::new(executable), root))
}

/// 由 `PISPER_RUNTIME_NODE`（Node 可执行文件）+ 应用根目录构造开发命令。
fn runtime_node_command(
    node: impl Into<std::ffi::OsString>,
    root: PathBuf,
) -> Result<(Command, PathBuf)> {
    let entry = root.join("runtime").join("sidecar.mjs");
    if !entry.is_file() {
        return Err(anyhow!(
            "Pisper Runtime entry point is missing: {}",
            entry.display()
        ));
    }
    let mut command = Command::new(node.into());
    command.arg(entry);
    Ok((command, root))
}

/// 组件安装根目录（系统数据目录下的 `components`）。
pub fn components_root() -> Result<PathBuf> {
    dirs::data_local_dir()
        .map(|directory| directory.join(APP_IDENTIFIER).join("components"))
        .context("failed to locate the Pisper component directory")
}

/// 已安装的 Web 前端组件（必须是完整可执行文件形态，且真正存在）。
pub fn installed_frontend() -> Option<pisper_component_updater::InstalledComponent> {
    resolve_installed(&components_root().ok()?, Component::Desktop)
        .ok()
        .flatten()
        .filter(|installed| installed.executable().is_file())
}

/// 前端静态资源根目录：环境变量显式指定优先，其次用已安装组件；
/// 必须含 `index.html` 才算有效，否则视为未配置。
pub fn configured_frontend_root() -> Option<PathBuf> {
    std::env::var_os("PISPER_FRONTEND_ROOT")
        .map(PathBuf::from)
        .or_else(|| installed_frontend()?.frontend_root())
        .filter(|root| root.join("index.html").is_file())
}

/// 已安装的签名 Runtime 组件；仅当其版本高于捆绑版本时才采用，
/// 避免用过时的已安装组件覆盖更新的捆绑运行时。
pub fn installed_runtime() -> Option<pisper_component_updater::InstalledComponent> {
    let installed = resolve_installed(&components_root().ok()?, Component::Runtime)
        .ok()
        .flatten()?;
    let bundled = bundled_runtime_version()
        .and_then(|value| Version::parse(&value).ok())
        .unwrap_or_else(|| Version::new(0, 0, 0));
    (installed.version > bundled).then_some(installed)
}

/// 运行时是否由外部管理（无需 TUI 负责安装）。
fn runtime_is_externally_managed(
    remote_url: bool,
    sidecar_path: bool,
    runtime_node: bool,
    desktop_sidecar: bool,
) -> bool {
    remote_url || sidecar_path || runtime_node || desktop_sidecar
}

/// 是否需要安装 Runtime 组件：
/// 外部管理（远程/桌面/显式路径）或已有签名组件/捆绑 sidecar/开发入口时不需要。
pub fn needs_runtime_install() -> bool {
    if runtime_is_externally_managed(
        std::env::var_os("PISPER_TUI_URL").is_some(),
        std::env::var_os("PISPER_SIDECAR_PATH").is_some(),
        std::env::var_os("PISPER_RUNTIME_NODE").is_some(),
        desktop_sidecar_descriptor().is_some(),
    ) {
        return false;
    }
    if installed_runtime().is_some() {
        return false;
    }
    let Ok(current_exe) = std::env::current_exe() else {
        return true;
    };
    let Some(executable_dir) = current_exe.parent() else {
        return true;
    };
    let bundled = [
        executable_dir.join(platform_sidecar_name()),
        executable_dir
            .parent()
            .unwrap_or(executable_dir)
            .join(platform_sidecar_name()),
    ];
    if !cfg!(debug_assertions) && bundled.into_iter().any(|path| path.is_file()) {
        return false;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."));
    !root.join("runtime").join("sidecar.mjs").is_file()
}

/// 读取捆绑（或仓库根目录）Runtime 的版本号，用于与已安装组件比较。
pub fn bundled_runtime_version() -> Option<String> {
    let current_exe = std::env::current_exe().ok()?;
    let executable_dir = current_exe.parent()?;
    let candidates = [
        executable_dir.join("sidecar-runtime").join("package.json"),
        executable_dir
            .parent()
            .unwrap_or(executable_dir)
            .join("sidecar-runtime")
            .join("package.json"),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("package.json"),
    ];
    candidates.into_iter().find_map(|path| {
        let value = fs::read(path).ok()?;
        serde_json::from_slice::<serde_json::Value>(&value)
            .ok()?
            .get("version")?
            .as_str()
            .map(str::to_owned)
    })
}

/// 与 sidecar 可执行文件同目录的 `sidecar-runtime` 应用根目录。
fn sibling_runtime(executable: &Path) -> PathBuf {
    executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sidecar-runtime")
}

/// 平台对应的 sidecar 可执行文件名。
fn platform_sidecar_name() -> &'static str {
    if cfg!(windows) {
        "pisper-sidecar.exe"
    } else {
        "pisper-sidecar"
    }
}

/// 生成 32 字节随机令牌：用作 sidecar 的访问凭证（随请求 cookie 回传）。
/// 使用密码学安全随机源，避免可预测令牌被本地其他进程冒用。
fn secure_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| anyhow!("failed to generate sidecar token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::{runtime_is_externally_managed, runtime_node_command, spawn_sidecar};
    use std::{fs, path::PathBuf, process::Command, time::Instant};

    /// 构造临时运行时目录（含一个占位 sidecar.mjs 入口），测试后由调用方清理。
    fn temporary_runtime() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pisper-runtime-command-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace("::", "-")
        ));
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join("sidecar.mjs"), "// test entry\n").unwrap();
        root
    }

    /// 验证 npm 运行时命令用指定 node 与签名入口启动（非独立安装器路径）。
    #[test]
    fn npm_runtime_uses_node_with_the_signed_runtime_entry() {
        let root = temporary_runtime();
        let (command, resolved_root) =
            runtime_node_command("node-for-test", root.clone()).expect("runtime command");
        assert_eq!(resolved_root, root);
        assert_eq!(command.get_program(), "node-for-test");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![root.join("runtime").join("sidecar.mjs").as_os_str()]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// 验证 npm 部署的运行时标记为“外部管理”，跳过独立运行时安装器。
    #[test]
    fn npm_node_runtime_skips_the_standalone_runtime_installer() {
        assert!(runtime_is_externally_managed(false, false, true, false));
        assert!(!runtime_is_externally_managed(false, false, false, false));
    }

    /// 验证 sidecar 未达就绪即退出时立即报错（不挂起等待）。
    #[test]
    fn a_sidecar_that_exits_before_readiness_fails_immediately() {
        let command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo runtime-startup-marker 1>&2 & exit /b 7"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "echo runtime-startup-marker >&2; exit 7"]);
            command
        };
        let started = Instant::now();
        let result = spawn_sidecar(
            command,
            std::env::current_dir().unwrap(),
            &std::env::current_dir().unwrap(),
            "test-token",
        );
        assert!(result.is_err());
        assert!(started.elapsed().as_secs_f32() < 2.0);
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("before readiness") || message.contains("closed its output"));
        assert!(message.contains("runtime-startup-marker"));
    }
}
