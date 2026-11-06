// 右侧浏览器的原生子 WebView（macOS、Windows）。
//
// iframe 打不开拒绝被嵌入的网站，在 macOS 上还会被主窗口的导航拦截挡住，所以桌面端让网页作为顶层页面加载。
// 这里直接用 wry 建子 WebView，不开 Tauri 的 unstable 特性：那个特性会把主窗口和桌宠也改成子 WebView。
//
// 网页不可信：
// - 不注册 IPC，也不走 Tauri 的 WebView 管理，页面拿不到任何 Tauri 能力；
// - 用独立的数据目录，和应用不共享 Cookie，登录状态留在这个目录里；
// - 导航判定见 desktop_browser_policy.rs。
//
// wry 的 WebView 不能跨线程，所有 WebView 都只在主线程创建、操作和销毁，用线程局部变量保存。
use super::desktop_browser_policy::{
    parse_browser_url, validate_bounds, validate_browser_id, BrowserBounds,
};
use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, AppHandle, WebviewWindow};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOpenInput {
    browser_id: String,
    url: String,
    bounds: BrowserBounds,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserAction {
    Back,
    Forward,
    Reload,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BrowserEvent {
    Navigated { url: String, loading: bool },
    Title { title: String },
    NewWindow { url: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOpenResult {
    status: &'static str,
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("The native browser is only available to the main window.".into())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use super::super::desktop_browser_policy::{
        navigation_decision, parse_browser_url, reportable_url, BrowserBounds, NavigationDecision,
    };
    use super::{BrowserAction, BrowserEvent};
    use std::{cell::RefCell, collections::HashMap};
    use tauri::{ipc::Channel, AppHandle, Manager, Url, WebviewWindow};
    use tauri_plugin_opener::OpenerExt;
    use wry::{
        dpi::{LogicalPosition, LogicalSize},
        NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    };

    // 一个会话最多 12 个辅助页面，换会话时旧的还在关。这个上限只防止前端失控时泄漏 WebView。
    const MAX_BROWSERS: usize = 16;
    // 标题来自网页，不让它把事件撑大。
    const MAX_TITLE_CHARS: usize = 200;
    const PROFILE_DIRECTORY: &str = "browser-profile";

    thread_local! {
        static BROWSERS: RefCell<HashMap<String, WebView>> = RefCell::new(HashMap::new());
    }

    fn rect(bounds: BrowserBounds) -> Rect {
        Rect {
            position: LogicalPosition::new(bounds.x, bounds.y).into(),
            size: LogicalSize::new(bounds.width, bounds.height).into(),
        }
    }

    fn open_externally(app: &AppHandle, url: &str) {
        let _ = app.opener().open_url(url, None::<&str>);
    }

    fn with_browser<T>(
        browser_id: &str,
        action: impl FnOnce(&WebView) -> wry::Result<T>,
    ) -> Result<T, String> {
        BROWSERS.with_borrow(|browsers| {
            let webview = browsers
                .get(browser_id)
                .ok_or_else(|| "The browser is no longer open.".to_string())?;
            action(webview).map_err(|error| error.to_string())
        })
    }

    #[cfg(target_os = "macos")]
    fn configure_platform<'a>(builder: WebViewBuilder<'a>, app: &AppHandle) -> WebViewBuilder<'a> {
        use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
        use wry::WebViewBuilderExtDarwin;

        // 固定的标识，重启后还是同一份登录状态。
        const PROFILE_STORE: [u8; 16] = [
            0x70, 0x69, 0x73, 0x70, 0x65, 0x72, 0x2d, 0x62, 0x72, 0x6f, 0x77, 0x73, 0x65, 0x72,
            0x2d, 0x31,
        ];
        // WKWebView 默认的 UA 不带 Safari 字样，不少网站会判成不支持的浏览器。
        const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";

        let app = app.clone();
        let builder = builder
            .with_user_agent(USER_AGENT)
            // 没有下载处理器时 wry 会直接取消下载，交给系统浏览器至少能下载。
            .with_download_started_handler(move |url, _path| {
                open_externally(&app, &url);
                false
            });
        let separate_store_available = NSProcessInfo::processInfo()
            .isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
                majorVersion: 14,
                minorVersion: 0,
                patchVersion: 0,
            });
        // macOS 14 之前没有独立的数据存储，wry 会退回和主窗口共用的默认存储。
        // 隔离优先：这些系统上用无痕存储，代价是关掉后登录状态不保留。
        if separate_store_available {
            builder.with_data_store_identifier(PROFILE_STORE)
        } else {
            builder.with_incognito(true)
        }
    }

    #[cfg(target_os = "windows")]
    fn configure_platform<'a>(builder: WebViewBuilder<'a>, _app: &AppHandle) -> WebViewBuilder<'a> {
        builder
    }

    pub fn open(
        window: &WebviewWindow,
        browser_id: String,
        url: Url,
        bounds: BrowserBounds,
        on_event: Channel<BrowserEvent>,
    ) -> Result<(), String> {
        BROWSERS.with_borrow(|browsers| {
            if browsers.contains_key(&browser_id) {
                return Err("The browser is already open.".to_string());
            }
            if browsers.len() >= MAX_BROWSERS {
                return Err("Too many browsers are open.".to_string());
            }
            Ok(())
        })?;

        let app = window.app_handle().clone();
        let app_origin = window.url().map_err(|error| error.to_string())?;
        let profile = app
            .path()
            .app_local_data_dir()
            .map_err(|error| error.to_string())?
            .join(PROFILE_DIRECTORY);
        std::fs::create_dir_all(&profile).map_err(|error| error.to_string())?;
        // macOS 不看这个目录，用的是上面的数据存储标识。
        let mut web_context = WebContext::new(Some(profile));

        let navigation_app = app.clone();
        let navigation_origin = app_origin.clone();
        let new_window_app = app.clone();
        let new_window_events = on_event.clone();
        let title_events = on_event.clone();
        let builder = WebViewBuilder::new_with_web_context(&mut web_context)
            .with_url(url.as_str())
            .with_bounds(rect(bounds))
            // 第一次同步时才显示：那时前端已经判断过面板有没有被遮住。
            .with_visible(false)
            .with_focused(false)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(true)
            .with_navigation_handler(move |target| {
                let Ok(parsed) = Url::parse(&target) else {
                    return false;
                };
                match navigation_decision(&parsed, &navigation_origin) {
                    NavigationDecision::Allow => true,
                    NavigationDecision::OpenExternally => {
                        open_externally(&navigation_app, &target);
                        false
                    }
                    NavigationDecision::Deny => false,
                }
            })
            // 新窗口开成新页签。about:blank 这类地址在新页签里没有内容可看，不打开。
            .with_new_window_req_handler(move |target, _features| {
                let decision = Url::parse(&target)
                    .map(|parsed| navigation_decision(&parsed, &app_origin))
                    .unwrap_or(NavigationDecision::Deny);
                match decision {
                    NavigationDecision::Allow if parse_browser_url(&target).is_ok() => {
                        let _ = new_window_events.send(BrowserEvent::NewWindow { url: target });
                    }
                    NavigationDecision::Allow => {}
                    NavigationDecision::OpenExternally => {
                        open_externally(&new_window_app, &target);
                    }
                    NavigationDecision::Deny => {}
                }
                NewWindowResponse::Deny
            })
            .with_document_title_changed_handler(move |title| {
                let _ = title_events.send(BrowserEvent::Title {
                    title: title.chars().take(MAX_TITLE_CHARS).collect(),
                });
            })
            .with_on_page_load_handler(move |event, current| {
                if let Some(url) = reportable_url(&current) {
                    let _ = on_event.send(BrowserEvent::Navigated {
                        url,
                        loading: matches!(event, PageLoadEvent::Started),
                    });
                }
            });

        let webview = configure_platform(builder, &app)
            .build_as_child(window)
            .map_err(|error| error.to_string())?;
        BROWSERS.with_borrow_mut(|browsers| {
            browsers.insert(browser_id, webview);
        });
        Ok(())
    }

    pub fn navigate(browser_id: &str, url: &Url) -> Result<(), String> {
        with_browser(browser_id, |webview| webview.load_url(url.as_str()))
    }

    pub fn action(browser_id: &str, action: BrowserAction) -> Result<(), String> {
        with_browser(browser_id, |webview| match action {
            // wry 没有前进后退的接口，走页面自己的历史。
            BrowserAction::Back => webview.evaluate_script("history.back()"),
            BrowserAction::Forward => webview.evaluate_script("history.forward()"),
            BrowserAction::Reload => webview.reload(),
        })
    }

    pub fn sync(browser_id: &str, bounds: BrowserBounds, visible: bool) -> Result<(), String> {
        with_browser(browser_id, |webview| {
            webview.set_bounds(rect(bounds))?;
            webview.set_visible(visible)
        })
    }

    // 销毁放在借用之外：销毁时系统可能回调进来，再次访问 BROWSERS。
    pub fn close(browser_id: &str) {
        let removed = BROWSERS.with_borrow_mut(|browsers| browsers.remove(browser_id));
        drop(removed);
    }

    pub fn close_all() {
        let removed = BROWSERS.with_borrow_mut(std::mem::take);
        drop(removed);
    }
}

// 在主线程执行并等它做完。命令本身是 async 的：WebView2 创建时要转消息循环，
// 放在同步命令里（IPC 回调中）会死锁。
#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn on_main_thread<T: Send + 'static>(
    window: &WebviewWindow,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            let _ = sender.send(work());
        })
        .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "The main thread dropped the browser request.".to_string())?
}

#[tauri::command]
pub async fn desktop_browser_open(
    window: WebviewWindow,
    input: BrowserOpenInput,
    on_event: Channel<BrowserEvent>,
) -> Result<BrowserOpenResult, String> {
    ensure_main(&window)?;
    validate_browser_id(&input.browser_id)?;
    validate_bounds(&input.bounds)?;
    let url = parse_browser_url(&input.url)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let target = window.clone();
        on_main_thread(&window, move || {
            native::open(&target, input.browser_id, url, input.bounds, on_event)
        })
        .await?;
        Ok(BrowserOpenResult { status: "opened" })
    }
    // 其他平台不支持，由前端退回 iframe。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (url, on_event);
        Ok(BrowserOpenResult {
            status: "unsupported",
        })
    }
}

#[tauri::command]
pub async fn desktop_browser_navigate(
    window: WebviewWindow,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    let url = parse_browser_url(&url)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return on_main_thread(&window, move || native::navigate(&browser_id, &url)).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = url;
        Err("unsupported".into())
    }
}

#[tauri::command]
pub async fn desktop_browser_action(
    window: WebviewWindow,
    browser_id: String,
    action: BrowserAction,
) -> Result<(), String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return on_main_thread(&window, move || native::action(&browser_id, action)).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = action;
        Err("unsupported".into())
    }
}

#[tauri::command]
pub async fn desktop_browser_sync(
    window: WebviewWindow,
    browser_id: String,
    bounds: BrowserBounds,
    visible: bool,
) -> Result<(), String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    validate_bounds(&bounds)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return on_main_thread(&window, move || native::sync(&browser_id, bounds, visible)).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = visible;
        Err("unsupported".into())
    }
}

#[tauri::command]
pub async fn desktop_browser_close(
    window: WebviewWindow,
    browser_id: String,
) -> Result<(), String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    on_main_thread(&window, move || {
        native::close(&browser_id);
        Ok(())
    })
    .await?;
    Ok(())
}

// 主页面重新加载后前端不再记得这些 WebView，不关掉就成了盖在界面上的孤儿。
pub fn close_all(app: &AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let _ = app.run_on_main_thread(native::close_all);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = app;
}
