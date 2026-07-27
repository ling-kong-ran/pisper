use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder};

const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct SidecarReady {
    #[serde(rename = "bootstrapUrl")]
    bootstrap_url: String,
}

struct SidecarState(Mutex<Option<Child>>);

fn platform_binary_name() -> &'static str {
    if cfg!(windows) {
        "pisper-sidecar.exe"
    } else {
        "pisper-sidecar"
    }
}

fn pipe_logs<R: Read + Send + 'static>(reader: R, label: &'static str) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            eprintln!("[{label}] {line}");
        }
    });
}

fn development_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must have a project parent")
        .to_path_buf()
}

fn sidecar_command(app: &tauri::App) -> Result<Command, String> {
    let mut command;
    let app_root;

    if cfg!(debug_assertions) {
        app_root = development_root();
        command = Command::new("node");
        command.arg(app_root.join("server").join("sidecar.mjs"));
        command.current_dir(&app_root);
    } else {
        let executable_dir = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .ok_or_else(|| "Pisper executable has no parent directory.".to_string())?
            .to_path_buf();
        app_root = app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("sidecar-runtime");
        command = Command::new(executable_dir.join(platform_binary_name()));
    }

    command
        .env("PISPER_APP_ROOT", &app_root)
        .env("PISPER_PARENT_PID", std::process::id().to_string())
        .env("PISPER_EXIT_ON_STDIN_CLOSE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    Ok(command)
}

fn start_sidecar(app: &tauri::App) -> Result<(Child, SidecarReady), String> {
    let mut child = sidecar_command(app)?
        .spawn()
        .map_err(|error| format!("Failed to start Pisper sidecar: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Pisper sidecar stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Pisper sidecar stderr was not captured.".to_string())?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);

    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                let parsed = serde_json::from_str::<SidecarReady>(payload)
                    .map_err(|error| format!("Invalid sidecar readiness payload: {error}"));
                let _ = ready_tx.send(parsed);
            } else {
                eprintln!("[sidecar] {line}");
            }
        }
    });
    pipe_logs(stderr, "sidecar:error");

    match ready_rx.recv_timeout(SIDECAR_TIMEOUT) {
        Ok(Ok(ready)) => Ok((child, ready)),
        Ok(Err(error)) => {
            let _ = child.kill();
            Err(error)
        }
        Err(_) => {
            let _ = child.kill();
            Err("Pisper sidecar did not become ready within 30 seconds.".to_string())
        }
    }
}

fn stop_sidecar(app: &tauri::AppHandle) {
    let state = app.state::<SidecarState>();
    let Some(mut child) = state.0.lock().expect("sidecar state poisoned").take() else {
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

fn create_main_window(app: &tauri::App, ready: &SidecarReady) -> Result<(), String> {
    let url = Url::parse(&ready.bootstrap_url).map_err(|error| error.to_string())?;
    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Pisper")
        .inner_size(1440.0, 920.0)
        .min_inner_size(980.0, 680.0)
        .center()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let (child, ready) = start_sidecar(app)?;
            app.manage(SidecarState(Mutex::new(Some(child))));
            if let Err(error) = create_main_window(app, &ready) {
                stop_sidecar(app.handle());
                return Err(error.into());
            }
            Ok(())
        });

    let application = builder
        .build(tauri::generate_context!())
        .expect("failed to build Pisper WebView application");
    application.run(|app, event| {
        if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
            stop_sidecar(app);
        }
    });
}
