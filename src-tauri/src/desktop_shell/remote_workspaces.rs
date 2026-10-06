//! 桌面主动连接 Linux Runtime。档案凭据只由原生层保存；每个远程窗口固定一个服务器。
//! UI 来自安装包的本机 Runtime，远程窗口没有本机终端、文件选择或桌面桥接权限。
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::webview::NewWindowResponse;
use tauri::{Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

use crate::mobile::{
    pairing::{self, QrPayload},
    proxy::{self, ProxyHandle},
    store::{ProfileStore, ServerEndpoint},
};

struct Connection {
    server_id: String,
    proxy: Arc<ProxyHandle>,
}

pub(crate) struct DesktopRemoteState {
    store: Mutex<ProfileStore>,
    connections: Mutex<HashMap<String, Connection>>,
    mutation: tokio::sync::Mutex<()>,
    bootstrap_url: String,
}

impl DesktopRemoteState {
    pub(crate) fn new(app: &tauri::AppHandle, bootstrap_url: String) -> Result<Self, String> {
        let directory = if super::desktop_data_dir_override()?.is_some() {
            super::desktop_data_dir(app)?
        } else {
            app.path()
                .app_data_dir()
                .map_err(|error| error.to_string())?
        };
        let path = directory.join("desktop-remote-servers.json");
        Ok(Self {
            store: Mutex::new(ProfileStore::load(&path)),
            connections: Mutex::new(HashMap::new()),
            mutation: tokio::sync::Mutex::new(()),
            bootstrap_url,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceSummary {
    id: String,
    name: String,
    address: String,
    fingerprint: String,
    connected: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairInput {
    address: String,
    fingerprint: String,
    code: String,
    name: String,
}

fn trusted_main(window: &WebviewWindow, state: &DesktopRemoteState) -> Result<(), String> {
    let current = window.url().map_err(|error| error.to_string())?;
    let local = Url::parse(&state.bootstrap_url).map_err(|error| error.to_string())?;
    if window.label() != "main" || !super::same_origin(&current, &local) {
        return Err("只能从本机主窗口管理远程工作区。".into());
    }
    Ok(())
}

fn pair_payload(input: PairInput) -> Result<QrPayload, String> {
    let url = Url::parse(input.address.trim()).map_err(|_| "请输入完整的 HTTPS 地址。")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("服务器地址必须是 HTTPS 根地址，不能包含账号、路径或查询参数。".into());
    }
    let raw = input.fingerprint.trim();
    let raw = if raw
        .get(..7)
        .is_some_and(|value| value.eq_ignore_ascii_case("sha256:"))
    {
        &raw[7..]
    } else {
        raw
    };
    let fingerprint: String = raw
        .chars()
        .filter(|value| *value != ':' && !value.is_ascii_whitespace())
        .collect();
    if fingerprint.len() != 64 || !fingerprint.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err("请从 Linux 的 Pisper 远程访问页复制完整的 SHA256 证书指纹。".into());
    }
    let code = input.code.trim().to_string();
    if code.is_empty() || code.len() > 64 || code.chars().any(char::is_control) {
        return Err("请输入有效的配对码。".into());
    }
    let name = input.name.trim();
    if name.len() > 160 || name.chars().any(char::is_control) {
        return Err("工作区名称不能包含控制字符或超过 160 字节。".into());
    }
    Ok(QrPayload {
        v: 1,
        name: if name.is_empty() {
            url.host_str().unwrap_or("Linux").to_string()
        } else {
            name.to_string()
        },
        endpoints: vec![ServerEndpoint::lan(
            url.as_str().trim_end_matches('/').to_string(),
        )],
        fp: fingerprint.to_uppercase(),
        code,
    })
}

#[tauri::command]
pub(crate) fn desktop_remote_list(
    window: WebviewWindow,
    state: State<'_, DesktopRemoteState>,
) -> Result<Vec<WorkspaceSummary>, String> {
    trusted_main(&window, &state)?;
    let store = state.store.lock().map_err(|_| "远程档案锁不可用。")?;
    let connections = state.connections.lock().map_err(|_| "远程连接锁不可用。")?;
    Ok(store
        .servers()
        .iter()
        .map(|profile| WorkspaceSummary {
            id: profile.id.clone(),
            name: profile.name.clone(),
            fingerprint: profile.fingerprint.clone(),
            address: profile
                .endpoints
                .iter()
                .find(|endpoint| endpoint.kind == "lan")
                .map(|endpoint| endpoint.url.clone())
                .unwrap_or_default(),
            connected: connections
                .values()
                .any(|connection| connection.server_id == profile.id),
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn desktop_remote_pair(
    window: WebviewWindow,
    state: State<'_, DesktopRemoteState>,
    input: PairInput,
) -> Result<String, String> {
    trusted_main(&window, &state)?;
    let display_name = input.name.trim().to_string();
    let payload = pair_payload(input)?;
    let _mutation = state.mutation.lock().await;
    let mut profile = pairing::pair(&payload, "Pisper Desktop", None).await?;
    if !display_name.is_empty() {
        profile.name = display_name;
    }
    let id = profile.id.clone();
    state
        .store
        .lock()
        .map_err(|_| "远程档案锁不可用。")?
        .upsert(profile)?;
    Ok(id)
}

#[tauri::command]
pub(crate) async fn desktop_remote_open(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopRemoteState>,
    id: String,
) -> Result<(), String> {
    trusted_main(&window, &state)?;
    let _mutation = state.mutation.lock().await;
    let existing = state
        .connections
        .lock()
        .map_err(|_| "远程连接锁不可用。")?
        .iter()
        .find(|(_, connection)| connection.server_id == id)
        .map(|(label, _)| label.clone());
    if let Some(window) = existing.and_then(|label| app.get_webview_window(&label)) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let profile = state
        .store
        .lock()
        .map_err(|_| "远程档案锁不可用。")?
        .servers()
        .iter()
        .find(|profile| profile.id == id)
        .cloned()
        .ok_or("找不到这个远程工作区。")?;
    let name = profile.name.clone();
    let proxy = proxy::start_desktop_proxy(&state.bootstrap_url, profile).await?;
    let label = format!("remote-{}", proxy.port);
    let url = Url::parse(&proxy.bootstrap_url().ok_or("远程代理未生成引导地址。")?)
        .map_err(|error| error.to_string())?;
    let allowed = url.clone();
    let navigation_app = app.clone();
    let new_window_app = app.clone();
    let result = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title(format!("Pisper — {name} · 远程工作区"))
        .inner_size(1440.0, 920.0)
        .min_inner_size(1080.0, 680.0)
        .center()
        .disable_drag_drop_handler()
        .initialization_script(
            "Object.defineProperty(window, '__PISPER_REMOTE_WORKSPACE__', { value: true });",
        )
        // 不注入 desktop-bridge.js，也不赋予 remote-* 窗口任何 Tauri capability。
        .on_navigation(move |target| {
            if super::same_origin(target, &allowed) || target.scheme() == "about" {
                return true;
            }
            if matches!(target.scheme(), "http" | "https" | "mailto") {
                let _ = navigation_app
                    .opener()
                    .open_url(target.to_string(), None::<&str>);
            }
            false
        })
        .on_new_window(move |target, _| {
            if matches!(target.scheme(), "http" | "https" | "mailto") {
                let _ = new_window_app
                    .opener()
                    .open_url(target.to_string(), None::<&str>);
            }
            NewWindowResponse::Deny
        })
        .build();
    let remote_window = match result {
        Ok(window) => window,
        Err(error) => {
            proxy.shutdown();
            return Err(error.to_string());
        }
    };
    state
        .connections
        .lock()
        .map_err(|_| "远程连接锁不可用。")?
        .insert(
            label.clone(),
            Connection {
                server_id: id,
                proxy: proxy.clone(),
            },
        );
    remote_window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            proxy.shutdown();
            if let Some(state) = app.try_state::<DesktopRemoteState>() {
                if let Ok(mut connections) = state.connections.lock() {
                    connections.remove(&label);
                }
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn desktop_remote_forget(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, DesktopRemoteState>,
    id: String,
) -> Result<(), String> {
    trusted_main(&window, &state)?;
    let _mutation = state.mutation.lock().await;
    state
        .store
        .lock()
        .map_err(|_| "远程档案锁不可用。")?
        .forget(&id)?;
    let labels: Vec<_> = state
        .connections
        .lock()
        .map_err(|_| "远程连接锁不可用。")?
        .iter()
        .filter(|(_, connection)| connection.server_id == id)
        .map(|(label, connection)| {
            connection.proxy.shutdown();
            label.clone()
        })
        .collect();
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.close();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(address: &str) -> PairInput {
        PairInput {
            address: address.into(),
            fingerprint: format!("SHA256:{}", "AB".repeat(32)),
            code: "ABCD-EFGH".into(),
            name: "Linux".into(),
        }
    }
    #[test]
    fn validates_https_origin_and_out_of_band_fingerprint() {
        let payload = pair_payload(input(" https://linux.example:5174/ ")).unwrap();
        assert_eq!(payload.endpoints[0].url, "https://linux.example:5174");
        assert_eq!(payload.fp, "AB".repeat(32));
        for address in [
            "http://linux.example",
            "https://user:secret@linux.example",
            "https://linux.example/api",
            "https://linux.example/?token=secret",
            "https://linux.example/#fragment",
            "file:///etc/passwd",
        ] {
            assert!(pair_payload(input(address)).is_err(), "{address}");
        }
        let mut malformed = input("https://linux.example");
        malformed.fingerprint = format!("garbage {}", "AB".repeat(32));
        assert!(pair_payload(malformed).is_err());
    }
    #[test]
    fn summary_has_no_device_token() {
        let summary = WorkspaceSummary {
            id: "one".into(),
            name: "Linux".into(),
            address: "https://linux.example".into(),
            fingerprint: "AB".repeat(32),
            connected: false,
        };
        let json = serde_json::to_value(summary).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 5);
        assert!(json.get("token").is_none());
        assert!(json.get("deviceId").is_none());
    }
}
