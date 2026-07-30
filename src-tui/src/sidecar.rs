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
use serde::Deserialize;

const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const SIDECAR_DESCRIPTOR_NAME: &str = "desktop-sidecar.json";
const APP_IDENTIFIER: &str = "com.lingkongran.pisper";

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

pub struct SidecarConnection {
    pub url: String,
    pub token: String,
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
                child: None,
            });
        }
        if let Some(descriptor) = desktop_sidecar_descriptor() {
            return Ok(Self {
                url: descriptor.url,
                token: descriptor.token,
                child: None,
            });
        }

        let token = secure_token()?;
        let (mut command, app_root) = sidecar_command()?;
        command
            .current_dir(workspace)
            .env("PISPER_APP_ROOT", app_root)
            .env("PISPER_DESKTOP_TOKEN", &token)
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

        let ready = match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(result) => result?,
            Err(_) => {
                let detail = diagnostics
                    .lock()
                    .expect("sidecar diagnostics poisoned")
                    .join("\n");
                let _ = child.kill();
                return Err(anyhow!(
                    "Pisper sidecar did not become ready within 30 seconds{}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!("\n{detail}")
                    }
                ));
            }
        };

        Ok(Self {
            url: ready.url,
            token,
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

fn sidecar_command() -> Result<(Command, PathBuf)> {
    if let Ok(path) = std::env::var("PISPER_SIDECAR_PATH") {
        let executable = PathBuf::from(path);
        let root = std::env::var("PISPER_APP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| sibling_runtime(&executable));
        return Ok((Command::new(executable), root));
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
            return Ok((Command::new(sidecar), root));
        }
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("src-tui must have a project parent")?
        .to_path_buf();
    let entry = root.join("server").join("sidecar.mjs");
    if !entry.is_file() {
        return Err(anyhow!(
            "pisper-sidecar was not found next to the CLI and the development sidecar is unavailable"
        ));
    }
    let mut command = Command::new("node");
    command.arg(entry);
    Ok((command, root))
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
