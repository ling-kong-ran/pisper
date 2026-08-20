//! 移动端壳：启动本地回环代理，按配对状态决定窗口初始地址——
//! 已配对直接进主界面（经代理访问桌面端 UI），未配对进入内置连接页。
//!
//! 桌面构建时 run_mobile 不会被调用（仅用于在桌面主机上 check/test 代理逻辑），
//! 因此豁免 dead_code。
#![allow(dead_code)]

pub mod pairing;
pub mod pinning;
pub mod proxy;
pub mod store;

use std::sync::{Arc, Mutex};

use serde::Serialize;
use store::{ProfileStore, ServerProfile, SharedStore};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

use pairing::QrPayload;

pub struct MobileShared {
    store: Arc<SharedStore>,
    proxy: Arc<proxy::ProxyHandle>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerDto {
    id: String,
    name: String,
    endpoints: Vec<store::ServerEndpoint>,
    paired_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileStateDto {
    paired: bool,
    proxy_url: String,
    active_id: Option<String>,
    servers: Vec<ServerDto>,
}

fn state_dto(shared: &MobileShared) -> MobileStateDto {
    let (servers, active_id) = shared
        .store
        .lock()
        .map(|store| {
            (
                store
                    .servers()
                    .iter()
                    .map(|server: &ServerProfile| ServerDto {
                        id: server.id.clone(),
                        name: server.name.clone(),
                        endpoints: server.endpoints.clone(),
                        paired_at: server.paired_at.clone(),
                    })
                    .collect::<Vec<_>>(),
                store.active().map(|server| server.id.clone()),
            )
        })
        .unwrap_or_default();
    MobileStateDto {
        paired: active_id.is_some(),
        proxy_url: format!("http://127.0.0.1:{}", shared.proxy.port),
        active_id,
        servers,
    }
}

#[tauri::command]
fn mobile_state(state: State<'_, MobileShared>) -> MobileStateDto {
    state_dto(&state)
}

#[tauri::command]
async fn mobile_pair(
    payload_json: &str,
    device_name: Option<String>,
    state: State<'_, MobileShared>,
) -> Result<MobileStateDto, String> {
    let payload: QrPayload = serde_json::from_str(payload_json)
        .map_err(|_| "二维码内容不是有效的配对信息。".to_string())?;
    let device_name = device_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(default_device_name);
    let profile = pairing::pair(&payload, &device_name).await?;
    let store = state.store.clone();
    store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .upsert(profile)?;
    Ok(state_dto(&state))
}

#[tauri::command]
async fn mobile_pair_manual(
    url: String,
    code: String,
    fingerprint: String,
    device_name: Option<String>,
    state: State<'_, MobileShared>,
) -> Result<MobileStateDto, String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.starts_with("https://") {
        return Err("地址必须是 https:// 开头（例如 https://192.168.1.5:5174）。".into());
    }
    let payload = QrPayload {
        v: 1,
        name: String::new(),
        endpoints: vec![store::ServerEndpoint {
            kind: "lan".into(),
            url,
        }],
        fp: fingerprint,
        code,
    };
    let device_name = device_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(default_device_name);
    let profile = pairing::pair(&payload, &device_name).await?;
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .upsert(profile)?;
    Ok(state_dto(&state))
}

#[tauri::command]
fn mobile_select_server(
    id: String,
    state: State<'_, MobileShared>,
) -> Result<MobileStateDto, String> {
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .select(&id)?;
    Ok(state_dto(&state))
}

#[tauri::command]
fn mobile_forget_server(
    id: String,
    state: State<'_, MobileShared>,
) -> Result<MobileStateDto, String> {
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .forget(&id)?;
    Ok(state_dto(&state))
}

fn default_device_name() -> String {
    #[cfg(target_os = "ios")]
    {
        "iPhone".to_string()
    }
    #[cfg(not(target_os = "ios"))]
    {
        "Android 设备".to_string()
    }
}

/// 移动端入口：与桌面壳完全分离，不启动 sidecar。
pub fn run_mobile() {
    // 扫码插件整个 crate 是 cfg(mobile) 的，桌面构建时不能注册。
    let builder = tauri::Builder::default().plugin(tauri_plugin_notification::init());
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .setup(|app| {
            let store_path = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?
                .join("pisper-mobile.json");
            let store = Arc::new(Mutex::new(ProfileStore::load(&store_path)));
            // 代理必须先于窗口创建就绪：窗口初始地址依赖代理端口。
            let proxy = tauri::async_runtime::block_on(proxy::start_proxy(store.clone()))?;
            let paired = store
                .lock()
                .map(|store| store.active().is_some())
                .unwrap_or(false);
            app.manage(MobileShared {
                store,
                proxy: proxy.clone(),
            });

            let initial = if paired {
                WebviewUrl::External(
                    tauri::Url::parse(&format!("http://127.0.0.1:{}", proxy.port))
                        .map_err(|error| error.to_string())?,
                )
            } else {
                WebviewUrl::App("connect.html".into())
            };
            WebviewWindowBuilder::new(app, "main", initial)
                .title("Pisper")
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mobile_state,
            mobile_pair,
            mobile_pair_manual,
            mobile_select_server,
            mobile_forget_server,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Pisper mobile application")
        .run(|_, _| {});
}
