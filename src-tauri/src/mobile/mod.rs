//! 移动端壳：启动本地回环代理，按配对状态决定窗口初始地址——
//! 已配对直接进主界面（经代理访问桌面端 UI），未配对进入内置连接页。
//!
//! 桌面构建时 run_mobile 不会被调用（仅用于在桌面主机上 check/test 代理逻辑），
//! 因此豁免 dead_code。
#![allow(dead_code)]

pub mod local;
pub mod pairing;
pub mod pinning;
pub mod proxy;
pub mod store;
pub mod update;

use std::sync::{Arc, Mutex};

use serde::Serialize;
use store::{ProfileStore, ServerProfile, SharedStore};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

use pairing::QrPayload;

pub struct MobileShared {
    store: Arc<SharedStore>,
    proxy: Arc<proxy::ProxyHandle>,
    tunnels: Arc<crate::iroh_tunnel::TunnelBridgePool>,
    local: Arc<local::LocalRuntime>,
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
    /// 本机 Runtime 的回环入口：连接页/服务器设置用它切入本机模式。
    local_url: String,
    active_id: Option<String>,
    active_transport: Option<String>,
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
        local_url: format!("http://127.0.0.1:{}", shared.local.port),
        active_id,
        active_transport: shared.proxy.active_transport(),
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
    let profile = pairing::pair(&payload, &device_name, Some(state.tunnels.as_ref())).await?;
    let store = state.store.clone();
    store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .upsert(profile)?;
    // 配对成功即明确选择远程模式：覆盖可能存在的 local 记忆。
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .set_last_mode("remote")?;
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
        endpoints: vec![store::ServerEndpoint::lan(url)],
        fp: fingerprint,
        code,
    };
    let device_name = device_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(default_device_name);
    let profile = pairing::pair(&payload, &device_name, Some(state.tunnels.as_ref())).await?;
    {
        let mut store = state
            .store
            .lock()
            .map_err(|_| "state poisoned".to_string())?;
        store.upsert(profile)?;
        store.set_last_mode("remote")?;
    }
    Ok(state_dto(&state))
}

#[tauri::command]
fn mobile_select_server(
    id: String,
    state: State<'_, MobileShared>,
) -> Result<MobileStateDto, String> {
    {
        let mut store = state
            .store
            .lock()
            .map_err(|_| "state poisoned".to_string())?;
        store.select(&id)?;
        // 显式选择远程服务器，同时把模式记忆切回远程。
        store.set_last_mode("remote")?;
    }
    Ok(state_dto(&state))
}

/// 进入本机模式：记录模式记忆并返回最新状态（前端读 localUrl 导航）。
#[tauri::command]
fn mobile_enter_local(state: State<'_, MobileShared>) -> Result<MobileStateDto, String> {
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .set_last_mode("local")?;
    Ok(state_dto(&state))
}

/// 离开本机模式：回到远程优先的路由语义（冷启动不再直进本机页）。
#[tauri::command]
fn mobile_leave_local(state: State<'_, MobileShared>) -> Result<MobileStateDto, String> {
    state
        .store
        .lock()
        .map_err(|_| "state poisoned".to_string())?
        .set_last_mode("remote")?;
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

/// 连接页的地址：Android 上 Tauri 资产源是 http://tauri.localhost，iOS 是 tauri://localhost。
/// 前端（经代理访问的桌面 UI）用它跳回配对/服务器管理页。
#[tauri::command]
fn mobile_connect_url() -> String {
    #[cfg(target_os = "android")]
    {
        "http://tauri.localhost/connect.html".to_string()
    }
    #[cfg(target_os = "ios")]
    {
        "tauri://localhost/connect.html".to_string()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        String::new()
    }
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
    // rustls 的默认 CryptoProvider 必须显式安装：reqwest 同时启用了 ring 与
    // aws-lc-rs，进程级默认 provider 无法自动确定，任何走默认路径的 TLS 调用
    // （包括传递依赖深处的）都会直接 panic。安装 ring 为进程默认。
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 扫码插件整个 crate 是 cfg(mobile) 的，桌面构建时不能注册。
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    let builder = builder
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let store = Arc::new(Mutex::new(ProfileStore::load(
                &data_dir.join("pisper-mobile.json"),
            )));
            let tunnel_secret = crate::iroh_tunnel::load_or_create_secret(
                &data_dir.join("iroh-mobile-secret.key"),
            )?;
            let tunnels = Arc::new(tauri::async_runtime::block_on(
                crate::iroh_tunnel::TunnelBridgePool::start(
                    tunnel_secret,
                    iroh::RelayMode::Default,
                ),
            )?);
            // 代理必须先于窗口创建就绪：窗口初始地址依赖代理端口。
            let proxy = tauri::async_runtime::block_on(proxy::start_proxy(
                store.clone(),
                Some(tunnels.clone()),
            ))?;
            // 本机 Runtime 与代理并列启动：同规则，监听器留在该运行时上。
            let local = tauri::async_runtime::block_on(local::start_runtime(
                &data_dir.join("local-runtime"),
            ))?;
            // manage 会移走 local，启动路由只需要端口，先取出。
            let local_port = local.port;
            let last_mode = store
                .lock()
                .ok()
                .and_then(|store| store.last_mode().map(str::to_string));
            let paired = store
                .lock()
                .map(|store| store.active().is_some())
                .unwrap_or(false);
            app.manage(MobileShared {
                store,
                proxy: proxy.clone(),
                tunnels,
                local,
            });
            app.manage(update::MobileUpdateState::default());
            update::start_automatic_checks(app.handle().clone());

            // 启动路由：模式记忆优先（local 直进本机页），其次已配对远程，最后连接页。
            let initial = if last_mode.as_deref() == Some("local") {
                WebviewUrl::External(
                    tauri::Url::parse(&format!("http://127.0.0.1:{local_port}"))
                        .map_err(|error| error.to_string())?,
                )
            } else if paired {
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
            mobile_enter_local,
            mobile_leave_local,
            mobile_forget_server,
            mobile_connect_url,
            update::mobile_app_info,
            update::mobile_check_app_update,
            update::mobile_open_app_update,
        ]);

    // generate_context! 每个 crate 只能展开一次（macOS 会嵌入 Info.plist 符号），
    // 桌面端由 desktop_shell 持有，这里仅在移动端目标展开，避免符号重复定义。
    #[cfg(any(target_os = "android", target_os = "ios"))]
    builder
        .build(tauri::generate_context!())
        .expect("failed to build Pisper mobile application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Resumed) {
                update::check_after_resume(app.clone());
            }
        });

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = builder;
}
