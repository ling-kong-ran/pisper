use pisper_component_updater::{
    resolve_installed, Component, ComponentUpdater, InstalledComponent,
};
use semver::Version;
use serde::Serialize;
use std::{collections::HashMap, io::Write, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager, State};

use crate::desktop_shell::{
    cli_manager,
    desktop_bridge::{log_component_update as log_default_component_update, UPDATER_PUBLIC_KEY},
};

fn log_component_update(app: &AppHandle, message: &str) {
    match super::desktop_data_dir_override() {
        Ok(Some(directory)) => {
            let directory = directory.join("logs");
            if std::fs::create_dir_all(&directory).is_err() {
                return;
            }
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join("component-updater.log"))
            {
                let _ = writeln!(
                    file,
                    "{} [component-update] {message}",
                    time::OffsetDateTime::now_utc()
                );
            }
        }
        Ok(None) => log_default_component_update(app, message),
        Err(_) => {}
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentUpdateStatus {
    component: Component,
    state: String,
    current_version: String,
    available_version: String,
    message: String,
    release_url: String,
    notes: String,
    size: u64,
    transferred: u64,
    can_install: bool,
    restart_required: bool,
}

impl ComponentUpdateStatus {
    fn current(component: Component, version: String) -> Self {
        Self {
            component,
            state: "idle".into(),
            current_version: version,
            available_version: String::new(),
            message: String::new(),
            release_url: "https://github.com/ling-kong-ran/pisper/releases".into(),
            notes: String::new(),
            size: 0,
            transferred: 0,
            can_install: false,
            restart_required: false,
        }
    }
}

#[derive(Default)]
pub struct ComponentUpdateState {
    statuses: Mutex<HashMap<Component, ComponentUpdateStatus>>,
}

pub fn components_root(app: &AppHandle) -> Result<PathBuf, String> {
    super::desktop_data_dir(app).map(|directory| directory.join("components"))
}

fn bundled_version(component: Component) -> &'static str {
    match component {
        Component::Desktop => env!("PISPER_BUNDLED_DESKTOP_VERSION"),
        Component::Tui => env!("PISPER_BUNDLED_TUI_VERSION"),
        Component::Runtime => env!("PISPER_BUNDLED_RUNTIME_VERSION"),
    }
}

fn installed_is_newer(installed: &Version, bundled: &str) -> bool {
    let bundled = Version::parse(bundled).unwrap_or_else(|_| Version::new(0, 0, 0));
    installed > &bundled
}

pub fn installed_component(app: &AppHandle, component: Component) -> Option<InstalledComponent> {
    let installed = resolve_installed(&components_root(app).ok()?, component)
        .ok()
        .flatten()?;
    installed_is_newer(&installed.version, bundled_version(component)).then_some(installed)
}

pub fn effective_version(app: &AppHandle, component: Component) -> String {
    installed_component(app, component)
        .map(|installed| installed.version.to_string())
        .unwrap_or_else(|| bundled_version(component).to_string())
}

fn cleanup_health_url(runtime_url: &str) -> Option<tauri::Url> {
    let mut url = tauri::Url::parse(runtime_url).ok()?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_path("/api/health");
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

fn cleanup_health_request(runtime_url: &str, bootstrap_url: &str) -> Option<(tauri::Url, String)> {
    let health = cleanup_health_url(runtime_url)?;
    let bootstrap = tauri::Url::parse(bootstrap_url).ok()?;
    if bootstrap.origin() != health.origin()
        || bootstrap.path() != "/_pisper/desktop/bootstrap"
        || !bootstrap.username().is_empty()
        || bootstrap.password().is_some()
    {
        return None;
    }
    let tokens = bootstrap
        .query_pairs()
        .filter(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    let [token] = tokens.as_slice() else {
        return None;
    };
    if token.is_empty()
        || token.len() > 512
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some((health, format!("__pisper_desktop={token}")))
}

fn healthy_rust_runtime(value: &serde_json::Value) -> bool {
    value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
        && value.get("engine").and_then(serde_json::Value::as_str) == Some("pi-rs")
}

fn cleanup_lifecycle_idle(app: &AppHandle, pid: u32) -> bool {
    let Some(lifecycle) = app.try_state::<super::LifecycleState>() else {
        return false;
    };
    if lifecycle.quitting.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    let Some(updates) = app.try_state::<ComponentUpdateState>() else {
        return false;
    };
    let Ok(statuses) = updates.statuses.try_lock() else {
        return false;
    };
    if statuses.values().any(|status| {
        matches!(
            status.state.as_str(),
            "checking" | "downloading" | "installing"
        )
    }) {
        return false;
    }
    let Some(sidecar) = app.try_state::<super::SidecarState>() else {
        return false;
    };
    let Ok(mut state) = sidecar.0.try_lock() else {
        return false;
    };
    state.as_mut().is_some_and(|sidecar| {
        sidecar.pid == pid
            && sidecar.child.id() == pid
            && matches!(sidecar.child.try_wait(), Ok(None))
    })
}

async fn rust_backend_is_healthy(runtime_url: &str, bootstrap_url: &str) -> bool {
    let Some((url, cookie)) = cleanup_health_request(runtime_url, bootstrap_url) else {
        return false;
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
    else {
        return false;
    };
    // 令牌只用于同源本机健康请求，不落盘、不记录，也不允许重定向或代理转发。
    let Ok(mut response) = client
        .get(url)
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= 64 * 1024 => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return false,
        }
    }
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .is_ok_and(|value| healthy_rust_runtime(&value))
}

pub(super) fn cleanup_after_successful_startup(
    app: AppHandle,
    runtime_url: &str,
    bootstrap_url: &str,
    pid: u32,
) {
    let runtime_url = runtime_url.to_string();
    let bootstrap_url = bootstrap_url.to_string();
    tauri::async_runtime::spawn(async move {
        // 启动成功才验证后端；网络和磁盘清理不占用窗口初始化主线程。
        if !rust_backend_is_healthy(&runtime_url, &bootstrap_url).await {
            return;
        }
        let result = tauri::async_runtime::spawn_blocking(move || {
            if !cleanup_lifecycle_idle(&app, pid) {
                return;
            }
            let Ok(root) = components_root(&app) else {
                return;
            };
            match pisper_component_updater::cleanup_legacy_components(&root, true) {
                Ok(0) => {}
                Ok(count) => log_component_update(
                    &app,
                    &format!(
                        "removed {count} obsolete component caches after successful Rust startup"
                    ),
                ),
                Err(_) => log_component_update(
                    &app,
                    "obsolete component cleanup skipped; cache safety check or removal failed",
                ),
            }
        })
        .await;
        // 清理任务失败不改变已成功启动的应用状态。
        let _ = result;
    });
}

fn updater(app: &AppHandle) -> Result<ComponentUpdater, String> {
    ComponentUpdater::new(
        components_root(app)?,
        UPDATER_PUBLIC_KEY,
        &format!("Pisper-Desktop/{}", app.package_info().version),
    )
    .map_err(|error| error.to_string())
}

fn snapshot(app: &AppHandle, state: &ComponentUpdateState) -> Vec<ComponentUpdateStatus> {
    let statuses = state
        .statuses
        .lock()
        .expect("component update status poisoned");
    Component::ALL
        .into_iter()
        .map(|component| {
            statuses.get(&component).cloned().unwrap_or_else(|| {
                ComponentUpdateStatus::current(component, effective_version(app, component))
            })
        })
        .collect()
}

fn store(state: &ComponentUpdateState, status: ComponentUpdateStatus) {
    state
        .statuses
        .lock()
        .expect("component update status poisoned")
        .insert(status.component, status);
}

fn restart_pending(state: &ComponentUpdateState, component: Component) -> bool {
    state
        .statuses
        .lock()
        .expect("component update status poisoned")
        .get(&component)
        .is_some_and(|status| status.restart_required)
}

#[tauri::command]
pub fn desktop_component_update_status(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Vec<ComponentUpdateStatus> {
    snapshot(&app, &state)
}

#[tauri::command]
pub async fn desktop_check_component_updates(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Result<Vec<ComponentUpdateStatus>, String> {
    let updater = updater(&app).map_err(|error| {
        log_component_update(&app, &format!("updater initialization failed: {error}"));
        error
    })?;
    log_component_update(&app, "checking Desktop, TUI, and Runtime releases");
    for component in Component::ALL {
        let current_version = effective_version(&app, component);
        let pending_restart = restart_pending(&state, component);
        store(
            &state,
            ComponentUpdateStatus {
                state: "checking".into(),
                restart_required: pending_restart,
                ..ComponentUpdateStatus::current(component, current_version.clone())
            },
        );
        let status = match updater.latest(component).await {
            Ok(release) => {
                let current =
                    Version::parse(&current_version).unwrap_or_else(|_| Version::new(0, 0, 0));
                let latest = Version::parse(&release.version).map_err(|error| {
                    format!("Invalid {} release version: {error}", component.name())
                })?;
                let available = latest > current;
                ComponentUpdateStatus {
                    component,
                    state: if available { "available" } else { "current" }.into(),
                    current_version,
                    available_version: release.version,
                    message: String::new(),
                    release_url: release.release_url,
                    notes: release.notes,
                    size: release.size,
                    transferred: 0,
                    can_install: available,
                    restart_required: pending_restart,
                }
            }
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} release check failed: {error:#}", component.name()),
                );
                ComponentUpdateStatus {
                    component,
                    state: "error".into(),
                    current_version,
                    message: format!("{error:#}"),
                    restart_required: pending_restart,
                    ..ComponentUpdateStatus::current(component, String::new())
                }
            }
        };
        store(&state, status);
    }
    Ok(snapshot(&app, &state))
}

#[tauri::command]
pub async fn desktop_install_component_updates(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Result<Vec<ComponentUpdateStatus>, String> {
    let updater = updater(&app).map_err(|error| {
        log_component_update(&app, &format!("updater initialization failed: {error}"));
        error
    })?;
    log_component_update(&app, "component update batch started");
    for component in Component::ALL {
        let current_version = effective_version(&app, component);
        let pending_restart = restart_pending(&state, component);
        let release = match updater.latest(component).await {
            Ok(release) => release,
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} release check failed: {error:#}", component.name()),
                );
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        message: format!("{error:#}"),
                        restart_required: pending_restart,
                        ..ComponentUpdateStatus::current(component, String::new())
                    },
                );
                continue;
            }
        };
        let current = Version::parse(&current_version).unwrap_or_else(|_| Version::new(0, 0, 0));
        let latest = match Version::parse(&release.version) {
            Ok(version) => version,
            Err(error) => {
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        available_version: release.version,
                        message: error.to_string(),
                        release_url: release.release_url,
                        notes: release.notes,
                        size: release.size,
                        transferred: 0,
                        can_install: false,
                        restart_required: pending_restart,
                    },
                );
                continue;
            }
        };
        if latest <= current {
            store(
                &state,
                ComponentUpdateStatus {
                    component,
                    state: "current".into(),
                    current_version,
                    available_version: release.version,
                    release_url: release.release_url,
                    notes: release.notes,
                    restart_required: pending_restart,
                    ..ComponentUpdateStatus::current(component, String::new())
                },
            );
            continue;
        }
        log_component_update(
            &app,
            &format!(
                "{} {} download started ({} bytes)",
                component.name(),
                release.version,
                release.size
            ),
        );
        store(
            &state,
            ComponentUpdateStatus {
                component,
                state: "downloading".into(),
                current_version: current_version.clone(),
                available_version: release.version.clone(),
                release_url: release.release_url.clone(),
                notes: release.notes.clone(),
                size: release.size,
                ..ComponentUpdateStatus::current(component, current_version.clone())
            },
        );
        let progress_state = state.inner();
        let installed = match updater
            .install_with_progress(&release, |transferred, _total| {
                let mut statuses = progress_state
                    .statuses
                    .lock()
                    .expect("component update status poisoned");
                if let Some(status) = statuses.get_mut(&component) {
                    status.transferred = transferred;
                }
            })
            .await
        {
            Ok(installed) => installed,
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} install failed: {error:#}", component.name()),
                );
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        available_version: release.version,
                        message: format!("{error:#}"),
                        release_url: release.release_url,
                        notes: release.notes,
                        size: release.size,
                        transferred: 0,
                        can_install: true,
                        restart_required: pending_restart,
                    },
                );
                continue;
            }
        };

        if component == Component::Tui && super::desktop_data_dir_override()?.is_none() {
            let _ = cli_manager::refresh_managed_cli(&app);
        }
        log_component_update(
            &app,
            &format!(
                "{} {} downloaded, verified, and installed ({} bytes)",
                component.name(),
                installed.version,
                release.size
            ),
        );
        store(
            &state,
            ComponentUpdateStatus {
                component,
                state: "installed".into(),
                current_version: installed.version.to_string(),
                available_version: installed.version.to_string(),
                message: String::new(),
                release_url: release.release_url,
                notes: release.notes,
                size: release.size,
                transferred: release.size,
                can_install: false,
                restart_required: matches!(component, Component::Desktop | Component::Runtime),
            },
        );
    }
    Ok(snapshot(&app, &state))
}

#[tauri::command]
pub fn desktop_restart_for_component_update(app: AppHandle) -> bool {
    log_component_update(&app, "restarting to activate Desktop or Runtime components");
    crate::desktop_shell::stop_sidecar(&app);
    app.cleanup_before_exit();
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_health_request, cleanup_health_url, healthy_rust_runtime, installed_is_newer,
        rust_backend_is_healthy, ComponentUpdateStatus,
    };
    use pisper_component_updater::Component;
    use semver::Version;

    #[test]
    fn older_or_equal_installed_components_do_not_override_bundled_components() {
        assert!(!installed_is_newer(
            &Version::parse("1.1.9").unwrap(),
            "1.2.0"
        ));
        assert!(!installed_is_newer(
            &Version::parse("1.2.0").unwrap(),
            "1.2.0"
        ));
        assert!(installed_is_newer(
            &Version::parse("1.2.1").unwrap(),
            "1.2.0"
        ));
    }

    #[test]
    fn cleanup_requires_an_explicit_healthy_rust_local_backend() {
        for value in [
            serde_json::json!({"ok":false,"engine":"pi-rs"}),
            serde_json::json!({"ok":true,"engine":"pi"}),
            serde_json::json!({"ok":true}),
            serde_json::json!({"ok":"true","engine":"pi-rs"}),
        ] {
            assert!(!healthy_rust_runtime(&value));
        }
        assert!(healthy_rust_runtime(
            &serde_json::json!({"ok":true,"engine":"pi-rs"})
        ));
        assert_eq!(
            cleanup_health_url("http://127.0.0.1:5184/?token=secret")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:5184/api/health"
        );
        assert!(cleanup_health_url("http://[::1]:5184").is_some());
        for url in [
            "http://remote.example:5184",
            "https://127.0.0.1:5184",
            "http://127.0.0.1",
            "http://token@127.0.0.1:5184",
        ] {
            assert!(cleanup_health_url(url).is_none())
        }
    }

    #[test]
    fn cleanup_credentials_are_single_url_safe_token_on_the_exact_loopback_origin() {
        let runtime = "http://127.0.0.1:5184";
        assert_eq!(
            cleanup_health_request(
                runtime,
                "http://127.0.0.1:5184/_pisper/desktop/bootstrap?token=test_token-1"
            )
            .unwrap()
            .1,
            "__pisper_desktop=test_token-1"
        );
        for bootstrap in [
            "http://127.0.0.1:5185/_pisper/desktop/bootstrap?token=test-token",
            "http://remote.example:5184/_pisper/desktop/bootstrap?token=test-token",
            "http://127.0.0.1:5184/other?token=test-token",
            "http://127.0.0.1:5184/_pisper/desktop/bootstrap",
            "http://127.0.0.1:5184/_pisper/desktop/bootstrap?token=",
            "http://127.0.0.1:5184/_pisper/desktop/bootstrap?token=a&token=b",
            "http://127.0.0.1:5184/_pisper/desktop/bootstrap?token=a%3Bb",
            "http://127.0.0.1:5184/_pisper/desktop/bootstrap?token=a%0Ab",
        ] {
            assert!(cleanup_health_request(runtime, bootstrap).is_none())
        }
    }

    #[tokio::test]
    async fn authenticated_health_probe_reclaims_cache_only_after_a_successful_response() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for (token, authorized) in [("fixture-token", true), ("wrong-token", false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut buffer = [0_u8; 1024];
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0 && bytes.len() + count <= 16 * 1024);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let request = std::str::from_utf8(&bytes).unwrap();
                assert!(request.starts_with("GET /api/health HTTP/1.1\r\n"));
                let authenticated = request.lines().any(|line| {
                    line.eq_ignore_ascii_case("cookie: __pisper_desktop=fixture-token")
                });
                assert_eq!(authenticated, authorized);
                // 401 即使携带看似健康的JSON，也不能触发清理。
                let body = r#"{"ok":true,"engine":"pi-rs"}"#;
                let status = if authenticated {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let bootstrap = format!("{url}/_pisper/desktop/bootstrap?token={token}");
            let healthy = rust_backend_is_healthy(&url, &bootstrap).await;
            server.join().unwrap();
            assert_eq!(healthy, authorized);
            let directory = std::env::temp_dir().join(format!(
                "pisper-cleanup-health-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let cache = directory.join("desktop/versions/0.5.81");
            std::fs::create_dir_all(cache.join("dist")).unwrap();
            std::fs::write(cache.join("dist/index.html"), b"legacy").unwrap();
            std::fs::write(cache.join("manifest.json"), serde_json::to_vec(&serde_json::json!({
                "name":"pisper-desktop", "version":"0.5.81", "platform": if cfg!(target_os="macos") {"darwin"} else if cfg!(windows) {"windows"} else {"linux"},
                "arch":std::env::consts::ARCH, "command":"dist/index.html",
            })).unwrap()).unwrap();
            assert_eq!(
                pisper_component_updater::cleanup_legacy_components(&directory, healthy).unwrap(),
                usize::from(authorized)
            );
            assert_eq!(cache.exists(), !authorized);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[tokio::test]
    async fn cleanup_health_probe_never_follows_redirects_with_credentials() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let redirect = format!("http://{}/api/health", destination.local_addr().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0_u8; 4096];
            assert!(stream.read(&mut bytes).unwrap() > 0);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(
            !rust_backend_is_healthy(
                &url,
                &format!("{url}/_pisper/desktop/bootstrap?token=fixture-token")
            )
            .await
        );
        server.join().unwrap();
        assert_eq!(
            destination.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn component_status_starts_idle_and_component_scoped() {
        let status = ComponentUpdateStatus::current(Component::Runtime, "1.2.3".into());
        assert_eq!(status.state, "idle");
        assert_eq!(status.current_version, "1.2.3");
        assert_eq!(status.transferred, 0);
        assert_eq!(status.component, Component::Runtime);
    }
}
