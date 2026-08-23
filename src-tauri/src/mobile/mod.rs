//! 移动端壳：启动本地回环代理并恢复上次明确选择的路由；首次启动默认进入本机
//! Runtime，桌面配对与模式切换统一由正常 React 设置页承载。
//!
//! 桌面构建时 run_mobile 不会被调用（仅用于在桌面主机上 check/test 代理逻辑），
//! 因此豁免 dead_code。
#![allow(dead_code)]

pub mod embedded_runtime;
pub mod on_device_runtime;
pub mod pairing;
pub mod pinning;
pub mod proxy;
pub mod root_runtime;
pub mod store;
pub mod update;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use store::{ProfileStore, ServerProfile, SharedStore};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};

#[cfg(any(target_os = "android", target_os = "ios"))]
use pisper_mobile_device_plugin::MobileDeviceExt;

use pairing::QrPayload;

pub struct MobileShared {
    store: Arc<SharedStore>,
    proxy: Arc<proxy::ProxyHandle>,
    tunnels: Arc<crate::iroh_tunnel::TunnelBridgePool>,
    on_device: Arc<on_device_runtime::OnDeviceRuntime>,
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
    /// 当前产品路由；active_id 仅表示远程模式要使用的服务器档案。
    mode: Option<String>,
    /// 壳根据设备环境选择同源 Node Runtime 的可用承载方式。
    on_device: root_runtime::RootRuntimeStatus,
    active_id: Option<String>,
    active_transport: Option<String>,
    servers: Vec<ServerDto>,
}

fn state_dto(shared: &MobileShared) -> MobileStateDto {
    let (servers, active_id, mode) = shared
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
                store.last_mode().map(str::to_string),
            )
        })
        .unwrap_or_default();
    let active_transport = if mode.as_deref() == Some("remote") {
        shared.proxy.active_transport()
    } else {
        None
    };
    MobileStateDto {
        paired: active_id.is_some(),
        proxy_url: format!("http://127.0.0.1:{}", shared.proxy.port),
        mode,
        on_device: shared.on_device.status(),
        active_id,
        active_transport,
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
    // 配对成功即明确选择远程模式；同进程 Node 会驻留，root 载体可以释放。
    state.on_device.deactivate();
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
    state.on_device.deactivate();
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
    state.on_device.deactivate();
    Ok(state_dto(&state))
}

/// 进入同源本机 Node Runtime；承载准备与启动放到阻塞线程。
#[tauri::command]
async fn mobile_enter_local(state: State<'_, MobileShared>) -> Result<MobileStateDto, String> {
    let on_device = state.on_device.clone();
    tauri::async_runtime::spawn_blocking(move || on_device.ensure_started())
        .await
        .map_err(|error| format!("本机 Runtime 任务失败：{error}"))??;
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
    state.on_device.deactivate();
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileDeviceOperationRequest {
    id: String,
    operation: String,
    #[serde(default)]
    parameters: serde_json::Map<String, serde_json::Value>,
}

fn operation_capability(operation: &str) -> Result<&'static str, String> {
    match operation {
        "contacts.search" => Ok("contacts"),
        "camera.capture" => Ok("camera"),
        "location.current" => Ok("location"),
        "apps.open_url"
        | "apps.open_map"
        | "apps.open_system_settings"
        | "apps.open_dialer"
        | "apps.compose_sms"
        | "apps.open_app" => Ok("externalApps"),
        _ => Err("不支持的移动设备操作。".into()),
    }
}

fn device_capability_state(
    app: &tauri::AppHandle,
    store: &SharedStore,
) -> Result<serde_json::Value, String> {
    let enabled = store
        .lock()
        .map_err(|_| "移动端档案锁已损坏。".to_string())?
        .device_capabilities();
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let permissions = app
        .mobile_device()
        .permission_states()
        .map_err(|error| error.to_string())?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let permissions = {
        let _ = app;
        serde_json::json!({
            "contacts": "unsupported",
            "camera": "unsupported",
            "location": "unsupported",
            "externalApps": "not-required"
        })
    };
    Ok(serde_json::json!({ "enabled": enabled, "permissions": permissions }))
}

#[tauri::command]
fn mobile_get_device_capabilities(
    app: tauri::AppHandle,
    state: State<'_, MobileShared>,
) -> Result<serde_json::Value, String> {
    device_capability_state(&app, state.store.as_ref())
}

#[tauri::command]
fn mobile_set_device_capability(
    app: tauri::AppHandle,
    state: State<'_, MobileShared>,
    capability: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    state
        .store
        .lock()
        .map_err(|_| "移动端档案锁已损坏。".to_string())?
        .set_device_capability(&capability, enabled)?;
    device_capability_state(&app, state.store.as_ref())
}

#[tauri::command]
fn mobile_request_device_permission(
    app: tauri::AppHandle,
    state: State<'_, MobileShared>,
    capability: String,
) -> Result<serde_json::Value, String> {
    let enabled = state
        .store
        .lock()
        .map_err(|_| "移动端档案锁已损坏。".to_string())?
        .device_capabilities()
        .enabled(&capability);
    if !enabled {
        return Err("请先在 Pisper 设置中启用该设备能力。".into());
    }
    if capability == "externalApps" {
        return device_capability_state(&app, state.store.as_ref());
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return app
        .mobile_device()
        .request_permission(capability)
        .map_err(|error| error.to_string());
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Err("当前平台不支持移动设备权限。".into())
    }
}

#[tauri::command]
fn mobile_open_device_settings(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return app
        .mobile_device()
        .open_app_settings()
        .map_err(|error| error.to_string());
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Err("当前平台不支持移动设备设置。".into())
    }
}

#[tauri::command]
fn mobile_execute_device_operation(
    app: tauri::AppHandle,
    state: State<'_, MobileShared>,
    request: MobileDeviceOperationRequest,
) -> Result<serde_json::Value, String> {
    if !request.id.starts_with("mop_") || request.id.len() > 80 {
        return Err("移动设备操作 ID 无效。".into());
    }
    let capability = operation_capability(&request.operation)?;
    let enabled = state
        .store
        .lock()
        .map_err(|_| "移动端档案锁已损坏。".to_string())?
        .device_capabilities()
        .enabled(capability);
    if !enabled {
        return Err("该设备能力已在 Pisper 设置中关闭。".into());
    }

    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let device = app.mobile_device();
        if capability != "externalApps" {
            let states = device
                .permission_states()
                .map_err(|error| error.to_string())?;
            if states.get(capability).and_then(|value| value.as_str()) != Some("granted") {
                let result = device
                    .request_permission(capability)
                    .map_err(|error| error.to_string())?;
                if result.get("state").and_then(|value| value.as_str()) != Some("granted") {
                    return Err("系统未授予该设备能力权限。".into());
                }
            }
        }
        let result = device
            .execute(pisper_mobile_device_plugin::OperationRequest {
                operation: request.operation,
                parameters: request.parameters,
            })
            .map_err(|error| error.to_string())?;
        if serde_json::to_vec(&result)
            .map_err(|error| error.to_string())?
            .len()
            > 12 * 1024 * 1024
        {
            return Err("移动设备操作结果超过 12 MB 限制。".into());
        }
        return Ok(result);
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = app;
        Err("当前平台不支持移动设备操作。".into())
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
    let builder = builder
        .plugin(pisper_mobile_device_plugin::init())
        .plugin(tauri_plugin_barcode_scanner::init());
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
            // root chroot 与 embedded Node 仅是内部承载，产品状态始终只有一个本机 Runtime。
            let embedded_resource = app
                .path()
                .resource_dir()
                .ok()
                .map(|path| path.join("pisper-embedded-runtime.tar.gz"));
            let on_device = Arc::new(on_device_runtime::OnDeviceRuntime::new(
                data_dir.join("on-device-runtime"),
                data_dir.join("local-runtime-data"),
                app.package_info().version.to_string(),
                embedded_resource,
            ));
            let last_mode = store
                .lock()
                .ok()
                .and_then(|store| store.last_mode().map(str::to_string));
            let paired = store
                .lock()
                .map(|store| store.active().is_some())
                .unwrap_or(false);
            let use_remote = last_mode.as_deref() == Some("remote") && paired;
            let on_device_url = if use_remote {
                None
            } else {
                // 首次启动与失效的远程档案都回到本机模式，避免再引入独立连接页。
                store
                    .lock()
                    .map_err(|_| "移动端档案锁已损坏。".to_string())?
                    .set_last_mode("local")?;
                on_device.ensure_started().ok().map(|status| status.url)
            };
            app.manage(MobileShared {
                store,
                proxy: proxy.clone(),
                tunnels,
                on_device,
            });
            app.manage(update::MobileUpdateState::default());
            update::start_automatic_checks(app.handle().clone());

            let initial = if use_remote {
                WebviewUrl::External(
                    tauri::Url::parse(&format!("http://127.0.0.1:{}", proxy.port))
                        .map_err(|error| error.to_string())?,
                )
            } else if let Some(url) = on_device_url {
                WebviewUrl::External(tauri::Url::parse(&url).map_err(|error| error.to_string())?)
            } else {
                // Runtime 故障仍保留完整 App 壳，用户可在设置中修复或配对桌面端。
                WebviewUrl::App("index.html".into())
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
            mobile_get_device_capabilities,
            mobile_set_device_capability,
            mobile_request_device_permission,
            mobile_open_device_settings,
            mobile_execute_device_operation,
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
            if matches!(
                event,
                tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
            ) {
                app.state::<MobileShared>().on_device.shutdown();
            }
        });

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = builder;
}
