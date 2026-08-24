//! 移动端壳：启动本地回环代理并恢复上次明确选择的路由；首次启动默认进入本机
//! Runtime，桌面配对与模式切换统一由正常 React 设置页承载。
//!
//! 桌面构建时 run_mobile 不会被调用（仅用于在桌面主机上 check/test 代理逻辑），
//! 因此豁免 dead_code。
#![allow(dead_code)]

pub mod android_bridge;
pub mod embedded_runtime;
pub mod on_device_runtime;
pub mod pairing;
pub mod pinning;
pub mod proxy;
#[cfg(not(feature = "mobile-store"))]
pub mod root_runtime;
pub mod runtime_status;
pub mod store;
#[cfg(feature = "mobile-store")]
#[path = "store_update.rs"]
pub mod update;
#[cfg(not(feature = "mobile-store"))]
pub mod update;

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    startup_error: Arc<Mutex<Option<String>>>,
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
    startup_error: Option<String>,
    /// 当前产品路由；active_id 仅表示远程模式要使用的服务器档案。
    mode: Option<String>,
    /// 壳根据设备环境选择同源 Node Runtime 的可用承载方式。
    on_device: runtime_status::RootRuntimeStatus,
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
    let proxy_url = format!("http://127.0.0.1:{}", shared.proxy.port);
    let on_device = shared.on_device.status();
    #[cfg(feature = "mobile-store")]
    let on_device = {
        let mut status = on_device;
        if status.running {
            // 商店包的页面来源固定为代理，不能暴露 embedded Runtime 的直接页面入口。
            status.url = proxy_url.clone();
        }
        status
    };
    let startup_error = shared
        .startup_error
        .lock()
        .ok()
        .and_then(|error| error.clone());
    MobileStateDto {
        paired: active_id.is_some(),
        proxy_url,
        startup_error,
        mode,
        on_device,
        active_id,
        active_transport,
        servers,
    }
}

const STARTUP_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STARTUP_RESPONSE_BYTES: u64 = 256 * 1024;

fn startup_api_context(bootstrap_url: &str) -> Result<(tauri::Url, tauri::Url, String), String> {
    let bootstrap = tauri::Url::parse(bootstrap_url)
        .map_err(|error| format!("本机 Runtime 启动地址无效：{error}"))?;
    if bootstrap.scheme() != "http"
        || bootstrap.host_str() != Some("127.0.0.1")
        || bootstrap.port().is_none()
        || bootstrap.path() != "/_pisper/desktop/bootstrap"
    {
        return Err("本机 Runtime 启动地址不是受信任的回环地址。".into());
    }
    let token = bootstrap
        .query_pairs()
        .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "本机 Runtime 启动地址缺少认证令牌。".to_string())?;
    let mut origin = bootstrap.clone();
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    Ok((origin, bootstrap, token))
}

fn startup_cookie(set_cookie: &str, expected_token: &str) -> Result<String, String> {
    let mut parts = set_cookie.split(';').map(str::trim);
    let pair = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "本机 Runtime bootstrap 未返回认证 Cookie。".to_string())?;
    let (name, encoded_value) = pair
        .split_once('=')
        .ok_or_else(|| "本机 Runtime bootstrap 返回了无效 Cookie。".to_string())?;
    let value_url = tauri::Url::parse(&format!("http://127.0.0.1/?value={encoded_value}"))
        .map_err(|_| "本机 Runtime bootstrap 返回了无效 Cookie。".to_string())?;
    let supplied_token = value_url
        .query_pairs()
        .find_map(|(key, value)| (key == "value").then(|| value.into_owned()))
        .unwrap_or_default();
    let attributes = parts
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if name != "__pisper_desktop"
        || supplied_token != expected_token
        || !attributes.iter().any(|value| value == "httponly")
        || !attributes.iter().any(|value| value == "samesite=strict")
        || !attributes.iter().any(|value| value == "path=/")
    {
        return Err("本机 Runtime bootstrap 认证 Cookie 不受信任。".into());
    }
    Ok(pair.to_string())
}

async fn fetch_startup_cookie(
    client: &reqwest::Client,
    bootstrap: &tauri::Url,
    token: &str,
) -> Result<String, String> {
    let response = client
        .get(bootstrap.clone())
        .send()
        .await
        .map_err(|error| format!("本机 Runtime bootstrap 请求失败：{error}"))?;
    if response.status() != reqwest::StatusCode::FOUND {
        return Err(format!(
            "本机 Runtime bootstrap 返回 HTTP {}。",
            response.status().as_u16()
        ));
    }
    let set_cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "本机 Runtime bootstrap 未返回认证 Cookie。".to_string())?;
    startup_cookie(set_cookie, token)
}

async fn fetch_startup_json(
    client: &reqwest::Client,
    origin: &tauri::Url,
    cookie: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    let mut url = origin.clone();
    url.set_path(path);
    let response = client
        .get(url)
        .header(reqwest::header::COOKIE, cookie)
        .header("x-pisper-client", "mobile-app")
        .send()
        .await
        .map_err(|error| format!("本机 Runtime {path} 请求失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "本机 Runtime {path} 返回 HTTP {}。",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_STARTUP_RESPONSE_BYTES)
    {
        return Err(format!("本机 Runtime {path} 响应过大。"));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("无法读取本机 Runtime {path} 响应：{error}"))?;
    if body.len() as u64 > MAX_STARTUP_RESPONSE_BYTES {
        return Err(format!("本机 Runtime {path} 响应过大。"));
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("本机 Runtime {path} 未返回有效 JSON：{error}"))
}

fn validate_startup_contract_values(
    client_info: &serde_json::Value,
    capabilities: &serde_json::Value,
    config: &serde_json::Value,
    sessions: &serde_json::Value,
) -> Result<(), String> {
    if client_info.get("client").and_then(|value| value.as_str()) != Some("mobile-app") {
        return Err("本机 Runtime 未识别移动 App 客户端。".into());
    }
    let profile = capabilities
        .get("profile")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if !matches!(profile, "mobile-root" | "mobile-embedded" | "mobile-store")
        || !capabilities
            .get("features")
            .is_some_and(serde_json::Value::is_object)
    {
        return Err("本机 Runtime 能力合同无效。".into());
    }
    if !config
        .get("providers")
        .is_some_and(serde_json::Value::is_array)
    {
        return Err("本机 Runtime Provider 配置合同无效。".into());
    }
    if !sessions
        .get("sessions")
        .is_some_and(serde_json::Value::is_array)
    {
        return Err("本机 Runtime 会话合同无效。".into());
    }
    Ok(())
}

async fn validate_local_runtime_startup(bootstrap_url: &str) -> Result<String, String> {
    let (origin, bootstrap, token) = startup_api_context(bootstrap_url)?;
    let client = reqwest::Client::builder()
        .timeout(STARTUP_PROBE_TIMEOUT)
        // bootstrap 的 302 由 WebView 最终消费；探针只提取并校验认证 Cookie。
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("无法创建本机 Runtime 启动探针：{error}"))?;
    let cookie = fetch_startup_cookie(&client, &bootstrap, &token).await?;
    let (client_info, capabilities, config, sessions) = tokio::try_join!(
        fetch_startup_json(&client, &origin, &cookie, "/api/client-info"),
        fetch_startup_json(&client, &origin, &cookie, "/api/runtime/capabilities"),
        fetch_startup_json(&client, &origin, &cookie, "/api/config"),
        fetch_startup_json(&client, &origin, &cookie, "/api/sessions"),
    )?;
    validate_startup_contract_values(&client_info, &capabilities, &config, &sessions)?;
    Ok(cookie)
}

fn record_startup_error(target: &Mutex<Option<String>>, error: Option<String>) {
    if let Ok(mut current) = target.lock() {
        *current = error;
    }
}

async fn ensure_local_runtime_ready(
    on_device: Arc<on_device_runtime::OnDeviceRuntime>,
    startup_error: Arc<Mutex<Option<String>>>,
) -> Result<(runtime_status::RootRuntimeStatus, String), String> {
    let runtime = on_device.clone();
    let status = match tauri::async_runtime::spawn_blocking(move || runtime.ensure_started()).await
    {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            record_startup_error(&startup_error, Some(error.clone()));
            return Err(error);
        }
        Err(error) => {
            let error = format!("本机 Runtime 任务失败：{error}");
            record_startup_error(&startup_error, Some(error.clone()));
            return Err(error);
        }
    };
    let cookie = match validate_local_runtime_startup(&status.url).await {
        Ok(cookie) => cookie,
        Err(error) => {
            record_startup_error(&startup_error, Some(error.clone()));
            return Err(error);
        }
    };
    record_startup_error(&startup_error, None);
    Ok((status, cookie))
}

fn startup_location_replace_script(url: &tauri::Url) -> String {
    let encoded_url = serde_json::Value::String(url.as_str().to_string()).to_string();
    format!("window.location.replace({encoded_url});")
}

#[cfg(target_os = "android")]
fn replace_with_authenticated_runtime(
    window: &tauri::WebviewWindow,
    bootstrap: &tauri::Url,
    _origin: &tauri::Url,
    _cookie: &str,
) -> Result<(), String> {
    // Android 必须由原生导航消费 Strict bootstrap Cookie；加载完成后再清除启动页历史。
    window
        .navigate(bootstrap.clone())
        .map_err(|error| format!("无法打开本机 Runtime：{error}"))
}

#[cfg(target_os = "android")]
fn clear_android_startup_history(window: tauri::WebviewWindow) {
    if let Err(error) = window.with_webview(|webview| {
        webview.jni_handle().exec(|env, _, webview| {
            if let Err(error) = env.call_method(webview, "clearHistory", "()V", &[]) {
                eprintln!("无法清除 Android WebView 启动历史：{error}");
            }
        });
    }) {
        eprintln!("无法访问 Android WebView：{error}");
    }
}

#[cfg(not(target_os = "android"))]
fn replace_with_authenticated_runtime(
    window: &tauri::WebviewWindow,
    _bootstrap: &tauri::Url,
    origin: &tauri::Url,
    cookie: &str,
) -> Result<(), String> {
    let (name, value) = cookie
        .split_once('=')
        .ok_or_else(|| "本机 Runtime 认证 Cookie 无效。".to_string())?;
    let cookie = tauri::webview::Cookie::build((name.to_string(), value.to_string()))
        .domain("127.0.0.1")
        .path("/")
        .http_only(true)
        .same_site(tauri::webview::cookie::SameSite::Strict)
        .build();
    window
        .set_cookie(cookie)
        .map_err(|error| format!("无法写入本机 Runtime 认证：{error}"))?;
    window
        .eval(startup_location_replace_script(origin))
        .map_err(|error| format!("无法打开本机 Runtime：{error}"))
}

fn start_initial_local_runtime(
    app: tauri::AppHandle,
    on_device: Arc<on_device_runtime::OnDeviceRuntime>,
    startup_error: Arc<Mutex<Option<String>>>,
    proxy: Arc<proxy::ProxyHandle>,
) {
    tauri::async_runtime::spawn(async move {
        let (status, cookie) =
            match ensure_local_runtime_ready(on_device, startup_error.clone()).await {
                Ok(result) => result,
                Err(_) => return,
            };
        #[cfg(not(feature = "mobile-store"))]
        let (origin, bootstrap, _) = match startup_api_context(&status.url) {
            Ok(context) => context,
            Err(error) => {
                record_startup_error(&startup_error, Some(error));
                return;
            }
        };
        let Some(window) = app.get_webview_window("main") else {
            record_startup_error(&startup_error, Some("移动端主窗口不可用。".into()));
            return;
        };
        #[cfg(feature = "mobile-store")]
        let navigation = {
            let _ = cookie;
            proxy.configure_local_runtime(&status.url).and_then(|_| {
                let url = tauri::Url::parse(&format!("http://127.0.0.1:{}", proxy.port))
                    .map_err(|error| error.to_string())?;
                window
                    .eval(startup_location_replace_script(&url))
                    .map_err(|error| format!("无法打开本机 Runtime 代理：{error}"))
            })
        };
        #[cfg(not(feature = "mobile-store"))]
        let navigation = {
            let _ = proxy;
            // 启动页只是过渡界面；各平台在保持 Strict Cookie 合同的前提下移除该历史项。
            replace_with_authenticated_runtime(&window, &bootstrap, &origin, &cookie)
        };
        if let Err(error) = navigation {
            record_startup_error(&startup_error, Some(error));
        }
    });
}

#[tauri::command]
fn mobile_state(state: State<'_, MobileShared>) -> MobileStateDto {
    state_dto(&state)
}

#[tauri::command]
async fn mobile_retry_local_startup(
    window: tauri::WebviewWindow,
    state: State<'_, MobileShared>,
) -> Result<(), String> {
    let (status, cookie) =
        ensure_local_runtime_ready(state.on_device.clone(), state.startup_error.clone()).await?;
    #[cfg(feature = "mobile-store")]
    {
        let _ = cookie;
        state.proxy.configure_local_runtime(&status.url)?;
        let url = tauri::Url::parse(&format!("http://127.0.0.1:{}", state.proxy.port))
            .map_err(|error| error.to_string())?;
        return window
            .eval(startup_location_replace_script(&url))
            .map_err(|error| format!("无法打开本机 Runtime 代理：{error}"));
    }
    #[cfg(not(feature = "mobile-store"))]
    {
        let (origin, bootstrap, _) = startup_api_context(&status.url)?;
        replace_with_authenticated_runtime(&window, &bootstrap, &origin, &cookie)
    }
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
    let (status, _) =
        ensure_local_runtime_ready(state.on_device.clone(), state.startup_error.clone()).await?;
    #[cfg(feature = "mobile-store")]
    state.proxy.configure_local_runtime(&status.url)?;
    #[cfg(not(feature = "mobile-store"))]
    let _ = status;
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

#[tauri::command]
fn mobile_execute_device_operation(
    app: tauri::AppHandle,
    request: MobileDeviceOperationRequest,
) -> Result<serde_json::Value, String> {
    if !request.id.starts_with("mop_") || request.id.len() > 80 {
        return Err("移动设备操作 ID 无效。".into());
    }
    let capability = operation_capability(&request.operation)?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let _ = capability;

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
            let startup_error = Arc::new(Mutex::new(None));
            if !use_remote {
                // 首次启动与失效的远程档案都回到本机模式，避免再引入独立连接页。
                store
                    .lock()
                    .map_err(|_| "移动端档案锁已损坏。".to_string())?
                    .set_last_mode("local")?;
            }
            app.manage(MobileShared {
                store,
                proxy: proxy.clone(),
                tunnels,
                on_device: on_device.clone(),
                startup_error: startup_error.clone(),
            });
            app.manage(update::MobileUpdateState::default());
            update::start_automatic_checks(app.handle().clone());

            #[cfg(feature = "mobile-store")]
            let initial = WebviewUrl::App("mobile-startup.html".into());
            #[cfg(not(feature = "mobile-store"))]
            let initial = if use_remote {
                WebviewUrl::External(
                    tauri::Url::parse(&format!("http://127.0.0.1:{}", proxy.port))
                        .map_err(|error| error.to_string())?,
                )
            } else {
                // Runtime 未通过启动合同前不能挂载任何依赖 /api 的业务页面。
                WebviewUrl::App("mobile-startup.html".into())
            };
            let window_builder = WebviewWindowBuilder::new(app, "main", initial).title("Pisper");
            #[cfg(target_os = "android")]
            let window_builder = {
                let startup_history_pending = Arc::new(AtomicBool::new(
                    cfg!(feature = "mobile-store") || !use_remote,
                ));
                window_builder.on_page_load(move |window, payload| {
                    let url = payload.url();
                    if payload.event() == tauri::webview::PageLoadEvent::Finished
                        && url.scheme() == "http"
                        && url.host_str() == Some("127.0.0.1")
                        && startup_history_pending
                            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    {
                        clear_android_startup_history(window);
                    }
                })
            };
            window_builder.build()?;
            #[cfg(feature = "mobile-store")]
            start_initial_local_runtime(app.handle().clone(), on_device, startup_error, proxy);
            #[cfg(not(feature = "mobile-store"))]
            if !use_remote {
                // 窗口先显示本地启动页，Runtime 的解压、启动和 API 合同探针在后台完成。
                start_initial_local_runtime(app.handle().clone(), on_device, startup_error, proxy);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mobile_state,
            mobile_retry_local_startup,
            mobile_pair,
            mobile_pair_manual,
            mobile_select_server,
            mobile_enter_local,
            mobile_leave_local,
            mobile_forget_server,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        startup_api_context, startup_cookie, startup_location_replace_script,
        validate_startup_contract_values,
    };

    #[test]
    fn startup_context_accepts_only_authenticated_loopback_bootstrap_urls() {
        let (origin, bootstrap, token) = startup_api_context(
            "http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=runtime-token",
        )
        .expect("trusted bootstrap URL");
        assert_eq!(origin.as_str(), "http://127.0.0.1:41873/");
        assert_eq!(
            bootstrap.as_str(),
            "http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=runtime-token"
        );
        assert_eq!(token, "runtime-token");
        assert!(startup_api_context("http://localhost:41873/?token=runtime-token").is_err());
        assert!(startup_api_context("https://example.com/?token=runtime-token").is_err());
        assert!(startup_api_context("http://127.0.0.1:41873/_pisper/desktop/bootstrap").is_err());
    }

    #[test]
    fn startup_navigation_replaces_the_transient_history_entry() {
        let url = tauri::Url::parse(
            "http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=runtime-token",
        )
        .expect("startup URL");
        assert_eq!(
            startup_location_replace_script(&url),
            "window.location.replace(\"http://127.0.0.1:41873/_pisper/desktop/bootstrap?token=runtime-token\");"
        );
    }

    #[test]
    fn startup_cookie_requires_the_bootstrap_security_contract() {
        let cookie = startup_cookie(
            "__pisper_desktop=runtime-token; HttpOnly; SameSite=Strict; Path=/",
            "runtime-token",
        )
        .expect("trusted bootstrap cookie");
        assert_eq!(cookie, "__pisper_desktop=runtime-token");
        assert!(startup_cookie(
            "__pisper_desktop=wrong-token; HttpOnly; SameSite=Strict; Path=/",
            "runtime-token"
        )
        .is_err());
        assert!(startup_cookie(
            "other=runtime-token; HttpOnly; SameSite=Strict; Path=/",
            "runtime-token"
        )
        .is_err());
        assert!(startup_cookie(
            "__pisper_desktop=runtime-token; SameSite=Strict; Path=/",
            "runtime-token"
        )
        .is_err());
    }

    #[test]
    fn startup_contract_rejects_incomplete_business_api_shapes() {
        let client = json!({ "client": "mobile-app" });
        let capabilities = json!({
            "profile": "mobile-embedded",
            "features": { "sessions": true }
        });
        let config = json!({ "providers": [] });
        let sessions = json!({ "sessions": [] });
        validate_startup_contract_values(&client, &capabilities, &config, &sessions)
            .expect("complete startup contract");

        assert!(
            validate_startup_contract_values(&client, &capabilities, &json!({}), &sessions)
                .is_err()
        );
        assert!(validate_startup_contract_values(
            &client,
            &json!({ "profile": "desktop", "features": {} }),
            &config,
            &sessions
        )
        .is_err());
        assert!(
            validate_startup_contract_values(&client, &capabilities, &config, &json!({})).is_err()
        );
    }
}
