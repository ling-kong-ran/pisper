//! 临时 profile 与浏览器进程由同一任务回收；驱动丢弃也不会遗留正常用户 profile。
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{process::Command, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(super) struct BrowserProcess {
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for BrowserProcess {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl BrowserProcess {
    pub(super) async fn launch(root: &Path) -> Result<(Arc<Self>, String), String> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|e| e.to_string())?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|e| e.to_string())?;
        let mut last = None;
        for executable in candidates() {
            if !executable.is_file() {
                continue;
            }
            match Self::launch_one(&root, &executable).await {
                Ok(value) => return Ok(value),
                Err(error) => last = Some(error),
            }
        }
        Err(format!("No controllable browser was found. Install Chrome, Edge, Chromium, or run Pisper Desktop.{}", last.map(|e| format!(" {e}")).unwrap_or_default()))
    }
    async fn launch_one(root: &Path, executable: &Path) -> Result<(Arc<Self>, String), String> {
        let arguments: Vec<String> = serde_json::from_str(include_str!(
            "../browser_scripts/playwright-launch-args.json"
        ))
        .map_err(|e| e.to_string())?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let profile = root.join(format!(
            "cdp-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // 创建、校验和安装进程回收拥有者之间不让出执行权，避免取消时出现无主的刚创建目录。
        std::fs::create_dir(&profile).map_err(|e| e.to_string())?;
        let profile = std::fs::canonicalize(&profile).map_err(|e| {
            let _ = std::fs::remove_dir(&profile);
            e.to_string()
        })?;
        if profile.parent() != Some(root) {
            return Err("Browser profile escaped its owned directory".into());
        }
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .args([
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--no-startup-window",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir(&profile);
                return Err(error.to_string());
            }
        };
        let pid = child.id();
        let cancel = CancellationToken::new();
        let owner_cancel = cancel.clone();
        let task_root = root.to_owned();
        let task_profile = profile.clone();
        let task = tokio::spawn(async move {
            tokio::select! { _ = child.wait() => {}, _ = owner_cancel.cancelled() => {
                if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_err() {
                    #[cfg(windows)] if let Some(pid) = pid {
                        let mut kill = Command::new("taskkill.exe"); kill.args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x08000000).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                        let _ = tokio::time::timeout(Duration::from_secs(5), kill.status()).await;
                    }
                    #[cfg(unix)] if let Some(pid)=pid{
                        let mut kill=Command::new("kill");kill.args(["-KILL",&format!("-{pid}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                        let _=tokio::time::timeout(Duration::from_secs(5),kill.status()).await;
                    }
                    #[cfg(not(any(windows,unix)))] let _ = pid;
                    let _ = child.start_kill(); let _ = child.wait().await;
                }
            } }
            remove_profile(&task_root, &task_profile).await;
        });
        let owner = Arc::new(Self {
            cancel,
            task: Mutex::new(Some(task)),
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    if let Ok(port) = port.parse::<u16>() {
                        if path.starts_with("/devtools/browser/") {
                            return Ok((owner, format!("ws://127.0.0.1:{port}{path}")));
                        }
                    }
                }
            }
            if owner
                .task
                .lock()
                .ok()
                .and_then(|t| t.as_ref().map(|t| t.is_finished()))
                .unwrap_or(true)
                || tokio::time::Instant::now() >= deadline
            {
                owner.close().await;
                return Err(format!("Browser launch failed: {}", executable.display()));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    pub(super) async fn close(&self) {
        self.cancel.cancel();
        let task = self.task.lock().ok().and_then(|mut task| task.take());
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
async fn remove_profile(root: &Path, profile: &Path) {
    // 只删除启动时已规范化且直接属于本次 profile 根目录的目录。
    if let Ok(current) = tokio::fs::canonicalize(profile).await {
        if current == profile
            && current.parent() == Some(root)
            && current
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("cdp-"))
        {
            for _ in 0..20 {
                if tokio::fs::remove_dir_all(&current).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
fn candidates() -> Vec<PathBuf> {
    let mut result = Vec::new();
    #[cfg(windows)]
    {
        for (env, suffix) in [
            ("PROGRAMFILES", "Microsoft/Edge/Application/msedge.exe"),
            ("PROGRAMFILES(X86)", "Microsoft/Edge/Application/msedge.exe"),
            ("PROGRAMFILES", "Google/Chrome/Application/chrome.exe"),
            ("PROGRAMFILES(X86)", "Google/Chrome/Application/chrome.exe"),
            ("LOCALAPPDATA", "Google/Chrome/Application/chrome.exe"),
        ] {
            result.push(PathBuf::from(std::env::var_os(env).unwrap_or_default()).join(suffix));
        }
    }
    #[cfg(target_os = "macos")]
    result.extend(
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .map(PathBuf::from),
    );
    #[cfg(windows)]
    let names = ["msedge.exe", "chrome.exe", "brave.exe"].as_slice();
    #[cfg(not(windows))]
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ]
    .as_slice();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in names {
                result.push(directory.join(name));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    result.retain(|value| seen.insert(value.clone()));
    result
}
