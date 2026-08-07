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
use serde::Deserialize;

const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const SIDECAR_DESCRIPTOR_NAME: &str = "desktop-sidecar.json";
pub const APP_IDENTIFIER: &str = "com.lingkongran.pisper";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyPayload {
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarDescriptor {
    version: u8,
    url: String,
    token: String,
    pid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionKind {
    Remote,
    Desktop,
    Spawned,
}

impl ConnectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Remote => "PISPER_TUI_URL",
            Self::Desktop => "desktop sidecar",
            Self::Spawned => "TUI sidecar",
        }
    }
}

pub struct SidecarConnection {
    pub url: String,
    pub token: String,
    pub kind: ConnectionKind,
    child: Option<Child>,
}

impl SidecarConnection {
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
        let (command, app_root, using_installed) = sidecar_command(true)?;
        let first = spawn_sidecar(command, app_root, workspace, &token);
        let (child, ready) = match first {
            Ok(result) => result,
            Err(component_error) if using_installed => {
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
        .env("PISPER_WORKSPACE_DIR", workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut lines = stderr_diagnostics
                .lock()
                .expect("sidecar diagnostics poisoned");
            lines.push(line);
            if lines.len() > 40 {
                lines.remove(0);
            }
        }
    });

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !sent {
                if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                    let parsed = serde_json::from_str::<ReadyPayload>(payload)
                        .map_err(|error| anyhow!("invalid sidecar readiness payload: {error}"));
                    let _ = ready_tx.send(parsed);
                    sent = true;
                }
            }
        }
    });

    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(ready)) => Ok((child, ready)),
        Ok(Err(error)) => {
            let _ = child.kill();
            Err(error)
        }
        Err(_) => {
            let detail = diagnostics
                .lock()
                .expect("sidecar diagnostics poisoned")
                .join("\n");
            let _ = child.kill();
            Err(anyhow!(
                "Pisper sidecar did not become ready within 30 seconds{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("\n{detail}")
                }
            ))
        }
    }
}

impl Drop for SidecarConnection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

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

fn sidecar_command(allow_installed: bool) -> Result<(Command, PathBuf, bool)> {
    if let Ok(path) = std::env::var("PISPER_SIDECAR_PATH") {
        let executable = PathBuf::from(path);
        let root = std::env::var("PISPER_APP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| sibling_runtime(&executable));
        return Ok((Command::new(executable), root, false));
    }

    if allow_installed {
        if let Some(installed) = installed_runtime() {
            return Ok((
                Command::new(installed.executable()),
                installed
                    .runtime_root()
                    .context("installed runtime payload is missing")?,
                true,
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
            return Ok((Command::new(sidecar), root, false));
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
    Ok((command, root, false))
}

pub fn components_root() -> Result<PathBuf> {
    dirs::data_local_dir()
        .map(|directory| directory.join(APP_IDENTIFIER).join("components"))
        .context("failed to locate the Pisper component directory")
}

pub fn installed_runtime() -> Option<pisper_component_updater::InstalledComponent> {
    resolve_installed(&components_root().ok()?, Component::Runtime)
        .ok()
        .flatten()
}

pub fn needs_runtime_install() -> bool {
    if std::env::var_os("PISPER_TUI_URL").is_some()
        || std::env::var_os("PISPER_SIDECAR_PATH").is_some()
        || desktop_sidecar_descriptor().is_some()
    {
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

pub fn bundled_runtime_version() -> Option<String> {
    if let Some(installed) = installed_runtime() {
        return Some(installed.version.to_string());
    }
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

fn sibling_runtime(executable: &Path) -> PathBuf {
    executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sidecar-runtime")
}

fn platform_sidecar_name() -> &'static str {
    if cfg!(windows) {
        "pisper-sidecar.exe"
    } else {
        "pisper-sidecar"
    }
}

fn secure_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| anyhow!("failed to generate sidecar token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
