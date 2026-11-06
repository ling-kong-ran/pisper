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
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserEvent {
    Navigated {
        url: String,
        loading: bool,
    },
    Title {
        title: String,
    },
    // browser_id 是已经建好、等前端接管的 WebView。建不出来时是 None，前端只能按地址开新页签。
    NewWindow {
        url: String,
        browser_id: Option<String>,
    },
    // 网页调用了 window.close()，或打开它的页面关掉了它。
    Closed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserOpenResult {
    status: &'static str,
}

// 输入内容最多 5000 字，转义后也远小于这个数。
const MAX_SCRIPT_BYTES: usize = 256 * 1024;

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
        navigation_decision, reportable_url, BrowserBounds, NavigationDecision,
    };
    use super::{BrowserAction, BrowserEvent};
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
        sync::{Arc, Mutex},
    };
    use tauri::{ipc::Channel, AppHandle, Manager, Url, WebviewWindow};
    use tauri_plugin_opener::OpenerExt;
    use tokio::sync::oneshot;
    use wry::{
        dpi::{LogicalPosition, LogicalSize},
        NewWindowFeatures, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView,
        WebViewBuilder,
    };

    // 一个会话最多 12 个辅助页面，换会话时旧的还在关。这个上限只防止前端失控时泄漏 WebView。
    const MAX_BROWSERS: usize = 16;
    // 标题来自网页，不让它把事件撑大。
    const MAX_TITLE_CHARS: usize = 200;
    // 弹窗在前端接管前就开始加载，事件先存着。登录跳转很多，但存不了太久：接管只隔一次渲染。
    const MAX_PENDING_EVENTS: usize = 64;
    const PROFILE_DIRECTORY: &str = "browser-profile";

    // 弹窗建好时前端还没有 Channel，事件先留在这里，接管时一次发过去。
    #[derive(Clone, Default)]
    struct EventSink(Arc<Mutex<SinkState>>);

    #[derive(Default)]
    struct SinkState {
        channel: Option<Channel<BrowserEvent>>,
        pending: Vec<BrowserEvent>,
    }

    impl EventSink {
        fn send(&self, event: BrowserEvent) {
            let Ok(mut state) = self.0.lock() else {
                return;
            };
            match &state.channel {
                Some(channel) => {
                    let _ = channel.send(event);
                }
                None if state.pending.len() < MAX_PENDING_EVENTS => state.pending.push(event),
                None => {}
            }
        }

        fn attach(&self, channel: Channel<BrowserEvent>) {
            let Ok(mut state) = self.0.lock() else {
                return;
            };
            for event in std::mem::take(&mut state.pending) {
                let _ = channel.send(event);
            }
            state.channel = Some(channel);
        }
    }

    struct Browser {
        webview: WebView,
        events: EventSink,
    }

    thread_local! {
        // 用 Rc：操作 WebView 时系统可能回调进来（比如弹窗请求），那时不能还借着这张表。
        static BROWSERS: RefCell<HashMap<String, Rc<Browser>>> = RefCell::new(HashMap::new());
        static NEXT_POPUP_ID: Cell<u64> = const { Cell::new(1) };
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
        let browser = BROWSERS
            .with_borrow(|browsers| browsers.get(browser_id).cloned())
            .ok_or_else(|| "The browser is no longer open.".to_string())?;
        action(&browser.webview).map_err(|error| error.to_string())
    }

    fn ensure_room(browser_id: &str) -> Result<(), String> {
        BROWSERS.with_borrow(|browsers| {
            if browsers.contains_key(browser_id) {
                return Err("The browser is already open.".to_string());
            }
            if browsers.len() >= MAX_BROWSERS {
                return Err("Too many browsers are open.".to_string());
            }
            Ok(())
        })
    }

    #[cfg(target_os = "macos")]
    fn configure_platform<'a>(builder: WebViewBuilder<'a>, app: &AppHandle) -> WebViewBuilder<'a> {
        // WKWebView 默认的 UA 不带 Safari 字样，不少网站会判成不支持的浏览器。
        const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";

        let app = app.clone();
        builder
            .with_user_agent(USER_AGENT)
            // 没有下载处理器时 wry 会直接取消下载，交给系统浏览器至少能下载。
            .with_download_started_handler(move |url, _path| {
                open_externally(&app, &url);
                false
            })
    }

    #[cfg(target_os = "windows")]
    fn configure_platform<'a>(builder: WebViewBuilder<'a>, _app: &AppHandle) -> WebViewBuilder<'a> {
        builder
    }

    #[cfg(target_os = "macos")]
    fn with_profile<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
        use wry::WebViewBuilderExtDarwin;

        // 固定的标识，重启后还是同一份登录状态。
        const PROFILE_STORE: [u8; 16] = [
            0x70, 0x69, 0x73, 0x70, 0x65, 0x72, 0x2d, 0x62, 0x72, 0x6f, 0x77, 0x73, 0x65, 0x72,
            0x2d, 0x31,
        ];
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
    fn with_profile<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        builder
    }

    // 弹窗要和打开它的页面用同一套环境，window.opener 才连得上，数据目录也自然相同。
    #[cfg(target_os = "macos")]
    fn popup_builder<'a>(features: NewWindowFeatures) -> WebViewBuilder<'a> {
        use wry::WebViewBuilderExtMacos;
        WebViewBuilder::new().with_webview_configuration(features.opener.target_configuration)
    }

    #[cfg(target_os = "windows")]
    fn popup_builder<'a>(features: NewWindowFeatures) -> WebViewBuilder<'a> {
        use wry::WebViewBuilderExtWindows;
        WebViewBuilder::new().with_environment(features.opener.environment)
    }

    #[cfg(target_os = "macos")]
    fn popup_response(webview: &WebView) -> NewWindowResponse {
        use objc2::rc::Retained;
        use wry::WebViewExtMacOS;
        NewWindowResponse::Create {
            webview: Retained::into_super(webview.webview()),
        }
    }

    #[cfg(target_os = "windows")]
    fn popup_response(webview: &WebView) -> NewWindowResponse {
        use wry::WebViewExtWindows;
        NewWindowResponse::Create {
            webview: webview.webview(),
        }
    }

    // wry 自己也监听这个事件，用来销毁容器窗口；这里多听一份，告诉前端关掉页签。
    #[cfg(target_os = "windows")]
    fn watch_window_close(webview: &WebView, events: EventSink) {
        use webview2_com::WindowCloseRequestedEventHandler;
        use wry::WebViewExtWindows;

        let handler = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
            events.send(BrowserEvent::Closed);
            Ok(())
        }));
        let mut token = Default::default();
        // SAFETY: handler 是有效的 COM 对象，token 在调用期间有效。
        let _ = unsafe {
            webview
                .webview()
                .add_WindowCloseRequested(&handler, &mut token)
        };
    }

    // wry 的 UI 代理没有实现 webViewDidClose:，这里在运行时补上。
    // WebKit 只在设置代理时检查它实现了哪些方法，所以补完要把同一个代理再设置一次。
    #[cfg(target_os = "macos")]
    fn watch_window_close(webview: &WebView, _events: EventSink) {
        use objc2::{
            ffi::class_addMethod,
            msg_send,
            runtime::{AnyClass, AnyObject, Imp, Sel},
            sel,
        };
        use wry::WebViewExtMacOS;

        unsafe extern "C-unwind" fn did_close(
            _this: *mut AnyObject,
            _cmd: Sel,
            view: *mut AnyObject,
        ) {
            use objc2::rc::Retained;
            use wry::WebViewExtMacOS;
            let closed = BROWSERS.with_borrow(|browsers| {
                browsers
                    .values()
                    .find(|browser| {
                        std::ptr::eq(
                            Retained::as_ptr(&browser.webview.webview()).cast::<AnyObject>(),
                            view.cast_const(),
                        )
                    })
                    .cloned()
            });
            if let Some(browser) = closed {
                browser.events.send(BrowserEvent::Closed);
            }
        }

        let view = webview.webview();
        // SAFETY: 方法签名和 webViewDidClose:（void, self, _cmd, WKWebView *）一致；
        // 方法已存在时 class_addMethod 什么也不做；代理还是原来那个对象。
        unsafe {
            let delegate: *mut AnyObject = msg_send![&*view, UIDelegate];
            if delegate.is_null() {
                return;
            }
            let class: *const AnyClass = (*delegate).class();
            let imp: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) = did_close;
            class_addMethod(
                class.cast_mut(),
                sel!(webViewDidClose:),
                std::mem::transmute::<_, Imp>(imp),
                c"v@:@".as_ptr(),
            );
            let _: () = msg_send![&*view, setUIDelegate: delegate];
        }
    }

    // 建好 WebView 并登记。普通页签和弹窗只有起点不同，导航判定和事件完全一样。
    fn register(
        window: &WebviewWindow,
        browser_id: String,
        builder: WebViewBuilder<'_>,
        app_origin: Url,
        events: EventSink,
    ) -> Result<Rc<Browser>, String> {
        let app = window.app_handle().clone();
        let navigation_app = app.clone();
        let navigation_origin = app_origin.clone();
        let new_window_app = app.clone();
        let new_window_parent = window.clone();
        let new_window_events = events.clone();
        let title_events = events.clone();
        let load_events = events.clone();
        let builder = builder
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
            .with_new_window_req_handler(move |target, features| {
                let decision = Url::parse(&target)
                    .map(|parsed| navigation_decision(&parsed, &app_origin))
                    .unwrap_or(NavigationDecision::Deny);
                match decision {
                    NavigationDecision::Allow => open_popup(
                        &new_window_parent,
                        &target,
                        features,
                        &app_origin,
                        &new_window_events,
                    ),
                    NavigationDecision::OpenExternally => {
                        open_externally(&new_window_app, &target);
                        NewWindowResponse::Deny
                    }
                    NavigationDecision::Deny => NewWindowResponse::Deny,
                }
            })
            .with_document_title_changed_handler(move |title| {
                title_events.send(BrowserEvent::Title {
                    title: title.chars().take(MAX_TITLE_CHARS).collect(),
                });
            })
            .with_on_page_load_handler(move |event, current| {
                if let Some(url) = reportable_url(&current) {
                    load_events.send(BrowserEvent::Navigated {
                        url,
                        loading: matches!(event, PageLoadEvent::Started),
                    });
                }
            });

        let webview = configure_platform(builder, &app)
            .build_as_child(window)
            .map_err(|error| error.to_string())?;
        watch_window_close(&webview, events.clone());
        let browser = Rc::new(Browser { webview, events });
        BROWSERS.with_borrow_mut(|browsers| {
            browsers.insert(browser_id, browser.clone());
        });
        Ok(browser)
    }

    // window.open 和 target=_blank：把系统要的 WebView 建好交回去，这样 window.opener 还在，
    // 弹窗登录能把结果传回原页面。前端收到 NewWindow 后接管它，显示成一个页签。
    fn open_popup(
        window: &WebviewWindow,
        target: &str,
        features: NewWindowFeatures,
        app_origin: &Url,
        opener_events: &EventSink,
    ) -> NewWindowResponse {
        let url = reportable_url(target).unwrap_or_default();
        let browser_id = format!(
            "native-popup-{}",
            NEXT_POPUP_ID.with(|next| next.replace(next.get() + 1))
        );
        // 前端第一次同步时才知道真正的位置，它现在还不可见，这个大小只是让页面有东西可排版。
        let bounds = Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(800.0, 600.0).into(),
        };
        let created = ensure_room(&browser_id).and_then(|()| {
            register(
                window,
                browser_id.clone(),
                popup_builder(features).with_bounds(bounds),
                app_origin.clone(),
                EventSink::default(),
            )
        });
        match created {
            Ok(browser) => {
                opener_events.send(BrowserEvent::NewWindow {
                    url,
                    browser_id: Some(browser_id),
                });
                popup_response(&browser.webview)
            }
            // 建不出来时退一步：丢掉 opener，至少按地址开成新页签。
            Err(_) => {
                if !url.is_empty() {
                    opener_events.send(BrowserEvent::NewWindow {
                        url,
                        browser_id: None,
                    });
                }
                NewWindowResponse::Deny
            }
        }
    }

    pub fn open(
        window: &WebviewWindow,
        browser_id: String,
        url: Url,
        bounds: BrowserBounds,
        on_event: Channel<BrowserEvent>,
    ) -> Result<(), String> {
        ensure_room(&browser_id)?;
        let app_origin = window.url().map_err(|error| error.to_string())?;
        let profile = window
            .app_handle()
            .path()
            .app_local_data_dir()
            .map_err(|error| error.to_string())?
            .join(PROFILE_DIRECTORY);
        std::fs::create_dir_all(&profile).map_err(|error| error.to_string())?;
        // macOS 不看这个目录，用的是 with_profile 里的数据存储标识。
        let mut web_context = WebContext::new(Some(profile));
        let builder = with_profile(WebViewBuilder::new_with_web_context(&mut web_context))
            .with_url(url.as_str())
            .with_bounds(rect(bounds));
        let events = EventSink::default();
        events.attach(on_event);
        register(window, browser_id, builder, app_origin, events).map(|_| ())
    }

    pub fn adopt(browser_id: &str, on_event: Channel<BrowserEvent>) -> Result<(), String> {
        let browser = BROWSERS
            .with_borrow(|browsers| browsers.get(browser_id).cloned())
            .ok_or_else(|| "The browser is no longer open.".to_string())?;
        browser.events.attach(on_event);
        Ok(())
    }

    pub fn evaluate(
        browser_id: &str,
        script: &str,
        done: oneshot::Sender<String>,
    ) -> Result<(), String> {
        // wry 要求回调是 Fn，结果只发一次。
        let done = Mutex::new(Some(done));
        with_browser(browser_id, |webview| {
            webview.evaluate_script_with_callback(script, move |result| {
                if let Some(done) = done.lock().ok().and_then(|mut done| done.take()) {
                    let _ = done.send(result);
                }
            })
        })
    }

    // WebView2 的预览截图接口要 COM 流，DevTools 协议直接给 base64，也不用多依赖 windows crate。
    #[cfg(target_os = "windows")]
    pub fn capture(
        browser_id: &str,
        done: oneshot::Sender<Result<String, String>>,
    ) -> Result<(), String> {
        use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR};
        use wry::WebViewExtWindows;

        let done = Mutex::new(Some(done));
        with_browser(browser_id, |webview| {
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |result, json: String| {
                    let image = result
                        .map_err(|error| error.to_string())
                        .and_then(|()| {
                            serde_json::from_str::<serde_json::Value>(&json)
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|value| {
                            value["data"]
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| "The screenshot came back empty.".to_string())
                        });
                    if let Some(done) = done.lock().ok().and_then(|mut done| done.take()) {
                        let _ = done.send(image);
                    }
                    Ok(())
                },
            ));
            let method = CoTaskMemPWSTR::from("Page.captureScreenshot");
            let params = CoTaskMemPWSTR::from(r#"{"format":"png"}"#);
            // SAFETY: 两个字符串在调用期间有效，handler 是有效的 COM 对象。
            unsafe {
                webview.webview().CallDevToolsProtocolMethod(
                    *method.as_ref().as_pcwstr(),
                    *params.as_ref().as_pcwstr(),
                    &handler,
                )
            }
            .map_err(|error| wry::Error::Io(std::io::Error::other(error.to_string())))
        })
    }

    #[cfg(target_os = "macos")]
    pub fn capture(
        browser_id: &str,
        done: oneshot::Sender<Result<String, String>>,
    ) -> Result<(), String> {
        use block2::RcBlock;
        use objc2::{msg_send, runtime::AnyObject};
        use wry::WebViewExtMacOS;

        let done = Mutex::new(Some(done));
        with_browser(browser_id, |webview| {
            let view = webview.webview();
            let block = RcBlock::new(move |image: *mut AnyObject, _error: *mut AnyObject| {
                // SAFETY: WebKit 把 NSImage 或 nil 传给回调，png_base64 处理 nil。
                let result = unsafe { png_base64(image) };
                if let Some(done) = done.lock().ok().and_then(|mut done| done.take()) {
                    let _ = done.send(result);
                }
            });
            // SAFETY: 配置传 nil 是截取整个可视区域；WebKit 会复制这个 block。
            unsafe {
                let _: () = msg_send![
                    &*view,
                    takeSnapshotWithConfiguration: std::ptr::null_mut::<AnyObject>(),
                    completionHandler: &*block
                ];
            }
            Ok(())
        })
    }

    // SAFETY: image 必须是 NSImage 或 nil。这几个 AppKit 类从 macOS 10.0 起就有，
    // 按名字调用，不多依赖 objc2-app-kit。
    #[cfg(target_os = "macos")]
    unsafe fn png_base64(image: *mut objc2::runtime::AnyObject) -> Result<String, String> {
        use base64::Engine;
        use objc2::{
            msg_send,
            runtime::{AnyClass, AnyObject},
        };
        // NSBitmapImageFileTypePNG
        const PNG_FILE_TYPE: usize = 4;

        // 隐藏的视图可能截不出图。
        let empty = || "The browser has nothing to capture while it is hidden.".to_string();
        if image.is_null() {
            return Err(empty());
        }
        let bitmap_class = AnyClass::get(c"NSBitmapImageRep").ok_or_else(empty)?;
        let dictionary_class = AnyClass::get(c"NSDictionary").ok_or_else(empty)?;
        let tiff: *mut AnyObject = msg_send![image, TIFFRepresentation];
        if tiff.is_null() {
            return Err(empty());
        }
        let bitmap: *mut AnyObject = msg_send![bitmap_class, imageRepWithData: tiff];
        if bitmap.is_null() {
            return Err(empty());
        }
        let properties: *mut AnyObject = msg_send![dictionary_class, dictionary];
        let png: *mut AnyObject =
            msg_send![bitmap, representationUsingType: PNG_FILE_TYPE, properties: properties];
        if png.is_null() {
            return Err(empty());
        }
        let bytes: *const std::ffi::c_void = msg_send![png, bytes];
        let length: usize = msg_send![png, length];
        let data = std::slice::from_raw_parts(bytes.cast::<u8>(), length);
        Ok(base64::engine::general_purpose::STANDARD.encode(data))
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

// 接管弹窗：前端在这里给它一个事件通道。弹窗已经关掉时返回错误，由前端按地址重新打开。
#[tauri::command]
pub async fn desktop_browser_adopt(
    window: WebviewWindow,
    browser_id: String,
    on_event: Channel<BrowserEvent>,
) -> Result<BrowserOpenResult, String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        on_main_thread(&window, move || native::adopt(&browser_id, on_event)).await?;
        Ok(BrowserOpenResult { status: "opened" })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = on_event;
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

// Agent 接管时在网页里执行脚本。网页不可信，但这条命令只有主窗口能调，网页自己调不到。
#[tauri::command]
pub async fn desktop_browser_evaluate(
    window: WebviewWindow,
    browser_id: String,
    script: String,
) -> Result<String, String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    if script.len() > MAX_SCRIPT_BYTES {
        return Err("The browser script is too long.".into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        on_main_thread(&window, move || {
            native::evaluate(&browser_id, &script, sender)
        })
        .await?;
        receiver
            .await
            .map_err(|_| "The browser closed before the script finished.".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = script;
        Err("unsupported".into())
    }
}

#[tauri::command]
pub async fn desktop_browser_capture(
    window: WebviewWindow,
    browser_id: String,
) -> Result<String, String> {
    ensure_main(&window)?;
    validate_browser_id(&browser_id)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        on_main_thread(&window, move || native::capture(&browser_id, sender)).await?;
        receiver
            .await
            .map_err(|_| "The browser closed before the screenshot finished.".to_string())?
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
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
