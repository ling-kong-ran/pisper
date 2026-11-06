// Computer Use 的桌面端：截屏和键鼠走 computer-control，这里负责会话、指示窗口和“停下”。
//
// Windows 上 Agent 接管前台：盖一层全屏遮罩（不挡输入，也不进截图），按 Esc 停止。
// macOS 和 Linux 上是一个置顶的小窗口，上面有停止按钮。开始时把主窗口最小化，
// 让出屏幕给要操作的应用；结束时还原。
//
// 只有主窗口的界面能调这些命令。指示窗口没有 IPC，它的停止按钮靠导航到一个约定地址告诉我们。
use computer_control::{
    capture_screen, check_support, foreground_is_own, parse_chord, point_hits_own_window,
    request_permissions, Button, Controller, ScreenMap,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
#[cfg(not(target_os = "windows"))]
use tauri::LogicalPosition;
use tauri::{
    ipc::Channel, AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const INDICATOR_LABEL: &str = "computer-indicator";
const STOP_SCHEME: &str = "pisper-computer";
const SCREENSHOT_MAX_WIDTH: u32 = 1440;
const SCREENSHOT_QUALITY: u8 = 72;
#[cfg(not(target_os = "windows"))]
const PANEL_WIDTH: f64 = 340.0;
#[cfg(not(target_os = "windows"))]
const PANEL_HEIGHT: f64 = 68.0;
#[cfg(not(target_os = "windows"))]
const PANEL_MARGIN: f64 = 16.0;
// 指针是滑过去的，用户才看得出 Agent 要点哪里。整段不到 150ms，主线程感觉不到。
const MOVE_STEPS: i32 = 12;
const MOVE_STEP_MS: u64 = 10;
const TYPE_CHUNK_CHARS: usize = 24;
const MAX_SETTLE_MS: u64 = 15_000;
const MAX_TEXT_CHARS: usize = 5_000;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ComputerEvent {
    // 用户按了 Esc 或点了停止按钮。
    Stopped,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerLabels {
    title: String,
    hint: String,
    // Windows 上是按 Esc 停止，遮罩上没有按钮。
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    stop: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginResult {
    status: &'static str,
    reason: Option<String>,
    missing_permissions: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotResult {
    image_base64: String,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ComputerAction {
    Move {
        x: f64,
        y: f64,
    },
    Click {
        x: f64,
        y: f64,
        button: Option<String>,
        count: Option<u8>,
    },
    Drag {
        x: f64,
        y: f64,
        to_x: f64,
        to_y: f64,
    },
    Scroll {
        x: f64,
        y: f64,
        direction: String,
        amount: Option<i32>,
    },
    Type {
        text: String,
    },
    Key {
        keys: String,
    },
}

struct Active {
    session_id: String,
    events: Channel<ComputerEvent>,
    map: Option<ScreenMap>,
    minimized_main: bool,
    #[cfg(target_os = "windows")]
    escape_hook: Option<escape_hook::EscapeHook>,
}

#[derive(Default)]
pub struct ComputerState {
    active: Mutex<Option<Active>>,
    stopped: Arc<AtomicBool>,
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Computer use is only available to the main window.".into())
    }
}

async fn on_main_thread<T: Send + 'static>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(work());
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "The main thread dropped the request.".to_string())?
}

fn escape_for_script(value: &str) -> String {
    // 放进 <script> 里：JSON 转义不管 </script>。
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('<', "\\u003c")
}

fn page_url(html: String) -> Result<Url, String> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(html);
    Url::parse(&format!("data:text/html;charset=utf-8;base64,{encoded}"))
        .map_err(|error| error.to_string())
}

// Windows：铺满屏幕的遮罩。边缘发光，顶部一条提示。
#[cfg(target_os = "windows")]
fn indicator_html(labels: &ComputerLabels) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{{margin:0;height:100%;background:transparent;overflow:hidden;font:500 14px/1.4 "Segoe UI",system-ui,sans-serif}}
#mask{{position:fixed;inset:0;background:rgba(18,12,48,.14);box-shadow:inset 0 0 0 3px rgba(124,92,255,.9),inset 0 0 90px 18px rgba(124,92,255,.42);animation:breathe 1.6s ease-in-out infinite alternate}}
#tip{{position:fixed;top:18px;left:50%;transform:translateX(-50%);padding:9px 18px;border-radius:999px;color:#fff;background:rgba(40,28,92,.92);box-shadow:0 8px 28px rgba(0,0,0,.35);white-space:nowrap}}
#tip b{{font-weight:600}}#tip span{{opacity:.8;margin-left:10px}}
kbd{{font:inherit;padding:1px 7px;border-radius:5px;background:rgba(255,255,255,.18)}}
@keyframes breathe{{from{{opacity:1}}to{{opacity:.62}}}}
@media (prefers-reduced-motion:reduce){{#mask{{animation:none}}}}
</style></head><body><div id="mask"></div><div id="tip"><b id="title"></b><span id="hint"></span></div><script>
document.getElementById('title').textContent={title};
document.getElementById('hint').textContent={hint};
</script></body></html>"#,
        title = escape_for_script(&labels.title),
        hint = escape_for_script(&labels.hint),
    )
}

// macOS、Linux：角落里的小窗口。
#[cfg(not(target_os = "windows"))]
fn indicator_html(labels: &ComputerLabels) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{{margin:0;height:100%;background:transparent;overflow:hidden;font:500 13px/1.35 -apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif;-webkit-user-select:none;user-select:none}}
#panel{{box-sizing:border-box;height:100%;display:flex;align-items:center;gap:12px;padding:0 12px 0 14px;border-radius:14px;color:#fff;background:rgba(40,28,92,.96);box-shadow:inset 0 0 0 1px rgba(255,255,255,.14)}}
#dot{{flex:none;width:10px;height:10px;border-radius:50%;background:#9d86ff;animation:breathe 1.1s ease-in-out infinite alternate}}
#text{{flex:1;min-width:0}}#title{{font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
#hint{{opacity:.72;font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
button{{flex:none;font:inherit;height:32px;padding:0 14px;border:0;border-radius:8px;color:#fff;background:#d0342c;cursor:pointer}}
button:hover{{background:#e0463e}}
@keyframes breathe{{from{{opacity:1;transform:scale(1)}}to{{opacity:.45;transform:scale(.8)}}}}
@media (prefers-reduced-motion:reduce){{#dot{{animation:none}}}}
</style></head><body><div id="panel"><div id="dot"></div><div id="text"><div id="title"></div><div id="hint"></div></div><button id="stop"></button></div><script>
document.getElementById('title').textContent={title};
document.getElementById('hint').textContent={hint};
const stop=document.getElementById('stop');stop.textContent={stop};
stop.onclick=()=>{{location.href='{STOP_SCHEME}://stop'}};
</script></body></html>"#,
        title = escape_for_script(&labels.title),
        hint = escape_for_script(&labels.hint),
        stop = escape_for_script(&labels.stop),
    )
}

fn trigger_stop(app: &AppHandle) {
    let state = app.state::<ComputerState>();
    state.stopped.store(true, Ordering::SeqCst);
    let events = state
        .active
        .lock()
        .ok()
        .and_then(|active| active.as_ref().map(|active| active.events.clone()));
    if let Some(events) = events {
        let _ = events.send(ComputerEvent::Stopped);
    }
    // 遮罩和主窗口立刻还给用户，不等这一轮的工具调用收尾。
    let target = app.clone();
    let _ = app.run_on_main_thread(move || finish_session(&target));
}

fn create_indicator(app: &AppHandle, labels: &ComputerLabels) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(INDICATOR_LABEL) {
        let _ = existing.close();
    }
    let stop_app = app.clone();
    let builder = WebviewWindowBuilder::new(
        app,
        INDICATOR_LABEL,
        WebviewUrl::External(page_url(indicator_html(labels))?),
    )
    .title("Pisper")
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .resizable(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .on_navigation(move |target| {
        if target.scheme() == STOP_SCHEME {
            trigger_stop(&stop_app);
            return false;
        }
        target.scheme() == "data" || target.scheme() == "about"
    });
    let monitor = app
        .primary_monitor()
        .map_err(|error| error.to_string())?
        .ok_or("No display is available.")?;

    // 遮罩不能抢焦点：焦点要留给 Agent 正在操作的应用。
    #[cfg(target_os = "windows")]
    let window = {
        let window = builder
            .focusable(false)
            .build()
            .map_err(|error| error.to_string())?;
        window
            .set_position(*monitor.position())
            .map_err(|error| error.to_string())?;
        window
            .set_size(*monitor.size())
            .map_err(|error| error.to_string())?;
        // 鼠标穿透：Agent 和用户的点击都落到下面的应用上。
        window
            .set_ignore_cursor_events(true)
            .map_err(|error| error.to_string())?;
        exclude_from_capture(&window);
        window
    };
    #[cfg(not(target_os = "windows"))]
    let window = {
        let window = builder
            .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
            .build()
            .map_err(|error| error.to_string())?;
        let scale = monitor.scale_factor();
        let screen = monitor.size().to_logical::<f64>(scale);
        let origin = monitor.position().to_logical::<f64>(scale);
        window
            .set_position(LogicalPosition::new(
                origin.x + screen.width - PANEL_WIDTH - PANEL_MARGIN,
                origin.y + PANEL_MARGIN + 28.0,
            ))
            .map_err(|error| error.to_string())?;
        exclude_from_capture(&window);
        window
    };
    window.show().map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn exclude_from_capture(window: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    if let Ok(hwnd) = window.hwnd() {
        // SAFETY: 句柄来自刚建好的窗口。系统太老不支持时返回失败，只是遮罩会进截图。
        unsafe { SetWindowDisplayAffinity(hwnd.0 as _, WDA_EXCLUDEFROMCAPTURE) };
    }
}

#[cfg(target_os = "macos")]
fn exclude_from_capture(window: &WebviewWindow) {
    use objc2::{msg_send, runtime::AnyObject};
    // NSWindowSharingNone
    const SHARING_NONE: usize = 0;
    if let Ok(ns_window) = window.ns_window() {
        let ns_window = ns_window.cast::<AnyObject>();
        // SAFETY: 主线程上，指针来自刚建好的窗口。
        unsafe {
            let _: () = msg_send![ns_window, setSharingType: SHARING_NONE];
        }
    }
}

// X11 没有这样的机制，截屏前临时藏起来（capture_hiding_panel）。
#[cfg(target_os = "linux")]
fn exclude_from_capture(_window: &WebviewWindow) {}

// 同步收尾。重复调用无害：用户按 Esc 和这一轮结束都会走到这里。
fn finish_session(app: &AppHandle) {
    let active = app
        .state::<ComputerState>()
        .active
        .lock()
        .ok()
        .and_then(|mut active| active.take());
    if let Some(window) = app.get_webview_window(INDICATOR_LABEL) {
        let _ = window.close();
    }
    let Some(active) = active else {
        return;
    };
    #[cfg(target_os = "windows")]
    drop(active.escape_hook);
    if active.minimized_main {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    }
}

// 主页面重新加载后前端不再记得这次会话，不收尾遮罩就留在屏幕上了。
pub fn close_all(app: &AppHandle) {
    let target = app.clone();
    let _ = app.run_on_main_thread(move || finish_session(&target));
}

#[tauri::command]
pub async fn desktop_computer_begin(
    window: WebviewWindow,
    session_id: String,
    labels: ComputerLabels,
    on_event: Channel<ComputerEvent>,
) -> Result<BeginResult, String> {
    ensure_main(&window)?;
    let support = check_support();
    if !support.missing_permissions.is_empty() {
        let missing_permissions = support.missing_permissions.clone();
        // 系统的授权弹窗要在主线程上弹。
        on_main_thread(window.app_handle(), || {
            request_permissions();
            Ok(())
        })
        .await?;
        return Ok(BeginResult {
            status: "permission_required",
            reason: support.reason,
            missing_permissions,
        });
    }
    if !support.supported {
        return Ok(BeginResult {
            status: "unsupported",
            reason: support.reason,
            missing_permissions: Vec::new(),
        });
    }
    let app = window.app_handle().clone();
    {
        let state = app.state::<ComputerState>();
        let active = state.active.lock().map_err(|_| "The state is poisoned.")?;
        if let Some(active) = active.as_ref() {
            // 屏幕只有一块，同一时间只能有一个会话在操作。
            if active.session_id != session_id {
                return Ok(BeginResult {
                    status: "busy",
                    reason: Some("Another session is using the computer.".into()),
                    missing_permissions: Vec::new(),
                });
            }
            return Ok(BeginResult {
                status: "started",
                reason: None,
                missing_permissions: Vec::new(),
            });
        }
    }
    let target = app.clone();
    on_main_thread(&app, move || {
        let state = target.state::<ComputerState>();
        state.stopped.store(false, Ordering::SeqCst);
        create_indicator(&target, &labels)?;
        let minimized_main = target
            .get_webview_window("main")
            .is_some_and(|main| !main.is_minimized().unwrap_or(false) && main.minimize().is_ok());
        #[cfg(target_os = "windows")]
        let escape_hook = {
            let hook_app = target.clone();
            escape_hook::EscapeHook::install(move || trigger_stop(&hook_app))
        };
        let mut active = state.active.lock().map_err(|_| "The state is poisoned.")?;
        *active = Some(Active {
            session_id,
            events: on_event,
            map: None,
            minimized_main,
            #[cfg(target_os = "windows")]
            escape_hook,
        });
        Ok(())
    })
    .await?;
    Ok(BeginResult {
        status: "started",
        reason: None,
        missing_permissions: Vec::new(),
    })
}

#[tauri::command]
pub async fn desktop_computer_end(window: WebviewWindow) -> Result<(), String> {
    ensure_main(&window)?;
    let app = window.app_handle().clone();
    let target = app.clone();
    on_main_thread(&app, move || {
        finish_session(&target);
        Ok(())
    })
    .await
}

fn ensure_running(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ComputerState>();
    if state.stopped.load(Ordering::SeqCst) {
        return Err("The user stopped computer use.".into());
    }
    let running = state
        .active
        .lock()
        .map(|active| active.is_some())
        .unwrap_or(false);
    if running {
        Ok(())
    } else {
        Err("Computer use is not running.".into())
    }
}

#[tauri::command]
pub async fn desktop_computer_screenshot(
    window: WebviewWindow,
    settle_ms: u64,
) -> Result<ScreenshotResult, String> {
    ensure_main(&window)?;
    let app = window.app_handle().clone();
    // 等界面反应过来再截。放在这里等：主窗口最小化时前端的定时器会被限速。
    tokio::time::sleep(Duration::from_millis(settle_ms.min(MAX_SETTLE_MS))).await;
    ensure_running(&app)?;

    #[cfg(target_os = "linux")]
    let panel = app.get_webview_window(INDICATOR_LABEL);
    #[cfg(target_os = "linux")]
    if let Some(panel) = &panel {
        let _ = panel.hide();
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    let captured =
        tokio::task::spawn_blocking(|| capture_screen(SCREENSHOT_MAX_WIDTH, SCREENSHOT_QUALITY))
            .await
            .map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    if let Some(panel) = &panel {
        let _ = panel.show();
    }
    let captured = captured?;
    let state = app.state::<ComputerState>();
    if let Ok(mut active) = state.active.lock() {
        if let Some(active) = active.as_mut() {
            active.map = Some(captured.map);
        }
    }
    Ok(ScreenshotResult {
        image_base64: captured.jpeg_base64,
        width: captured.map.image_width,
        height: captured.map.image_height,
    })
}

// 小窗口挡在目标上时先藏起来，否则点到的是停止按钮。
#[cfg(not(target_os = "windows"))]
fn panel_blocking(app: &AppHandle, x: i32, y: i32) -> Option<WebviewWindow> {
    let panel = app.get_webview_window(INDICATOR_LABEL)?;
    let position = panel.outer_position().ok()?;
    let size = panel.outer_size().ok()?;
    // macOS 的输入坐标是点，窗口坐标是物理像素。
    let scale = if cfg!(target_os = "macos") {
        panel.scale_factor().ok()?
    } else {
        1.0
    };
    let (x, y) = (f64::from(x) * scale, f64::from(y) * scale);
    let inside = x >= f64::from(position.x)
        && y >= f64::from(position.y)
        && x < f64::from(position.x) + f64::from(size.width)
        && y < f64::from(position.y) + f64::from(size.height);
    inside.then_some(panel)
}

fn glide(controller: &mut Controller, x: i32, y: i32) -> Result<(), String> {
    let (from_x, from_y) = controller.location().unwrap_or((x, y));
    for step in 1..=MOVE_STEPS {
        controller.move_to(
            from_x + (x - from_x) * step / MOVE_STEPS,
            from_y + (y - from_y) * step / MOVE_STEPS,
        )?;
        std::thread::sleep(Duration::from_millis(MOVE_STEP_MS));
    }
    Ok(())
}

fn button_from(name: Option<&str>) -> Result<Button, String> {
    match name.unwrap_or("left") {
        "left" => Ok(Button::Left),
        "right" => Ok(Button::Right),
        "middle" => Ok(Button::Middle),
        other => Err(format!("Unknown mouse button \"{other}\".")),
    }
}

const SELF_POINT: &str = "That point is on a Pisper window. Computer use cannot operate Pisper itself; pick another app.";
const SELF_FOREGROUND: &str = "Pisper is in the foreground. Computer use cannot type into Pisper itself; click the target app first.";

// 在主线程上做一个鼠标动作。判断目标和动手在同一段里，中间窗口不会变。
fn mouse_action(
    app: AppHandle,
    x: i32,
    y: i32,
    work: impl FnOnce(&mut Controller) -> Result<(), String>,
) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    let panel = panel_blocking(&app, x, y);
    #[cfg(not(target_os = "windows"))]
    if let Some(panel) = &panel {
        let _ = panel.hide();
    }
    #[cfg(target_os = "windows")]
    let _ = &app;
    let result = if point_hits_own_window(x, y) {
        Err(SELF_POINT.to_string())
    } else {
        Controller::new().and_then(|mut controller| {
            glide(&mut controller, x, y)?;
            work(&mut controller)
        })
    };
    #[cfg(not(target_os = "windows"))]
    if let Some(panel) = &panel {
        let _ = panel.show();
    }
    result
}

#[tauri::command]
pub async fn desktop_computer_act(
    window: WebviewWindow,
    action: ComputerAction,
) -> Result<(), String> {
    ensure_main(&window)?;
    let app = window.app_handle().clone();
    ensure_running(&app)?;
    let map = {
        let state = app.state::<ComputerState>();
        let active = state.active.lock().map_err(|_| "The state is poisoned.")?;
        active.as_ref().and_then(|active| active.map)
    };
    let point = |x: f64, y: f64| {
        map.ok_or_else(|| "Take a screenshot before pointing at the screen.".to_string())?
            .to_screen(x, y)
    };
    let target = app.clone();
    match action {
        ComputerAction::Move { x, y } => {
            let (x, y) = point(x, y)?;
            on_main_thread(&app, move || mouse_action(target, x, y, |_| Ok(()))).await
        }
        ComputerAction::Click {
            x,
            y,
            button,
            count,
        } => {
            let (x, y) = point(x, y)?;
            let button = button_from(button.as_deref())?;
            let count = count.unwrap_or(1).clamp(1, 3);
            on_main_thread(&app, move || {
                mouse_action(target, x, y, |controller| {
                    for _ in 0..count {
                        controller.click(button)?;
                    }
                    Ok(())
                })
            })
            .await
        }
        ComputerAction::Drag { x, y, to_x, to_y } => {
            let (x, y) = point(x, y)?;
            let (to_x, to_y) = point(to_x, to_y)?;
            on_main_thread(&app, move || {
                if point_hits_own_window(to_x, to_y) {
                    return Err(SELF_POINT.to_string());
                }
                mouse_action(target, x, y, |controller| {
                    controller.press(Button::Left)?;
                    let moved = glide(controller, to_x, to_y);
                    // 中途出错也要松开，否则系统里左键一直按着。
                    let released = controller.release(Button::Left);
                    moved.and(released)
                })
            })
            .await
        }
        ComputerAction::Scroll {
            x,
            y,
            direction,
            amount,
        } => {
            let (x, y) = point(x, y)?;
            let amount = amount.unwrap_or(3).clamp(1, 50);
            let (amount, horizontal) = match direction.as_str() {
                "down" => (amount, false),
                "up" => (-amount, false),
                "right" => (amount, true),
                "left" => (-amount, true),
                other => return Err(format!("Unknown scroll direction \"{other}\".")),
            };
            on_main_thread(&app, move || {
                mouse_action(target, x, y, |controller| {
                    controller.scroll(amount, horizontal)
                })
            })
            .await
        }
        ComputerAction::Type { text } => {
            let characters: Vec<char> = text.chars().take(MAX_TEXT_CHARS).collect();
            // 分段输入：长文本中途也能被 Esc 打断。
            for chunk in characters.chunks(TYPE_CHUNK_CHARS) {
                ensure_running(&app)?;
                let part: String = chunk.iter().collect();
                on_main_thread(&app, move || {
                    if foreground_is_own() {
                        return Err(SELF_FOREGROUND.to_string());
                    }
                    Controller::new()?.text(&part)
                })
                .await?;
            }
            Ok(())
        }
        ComputerAction::Key { keys } => {
            let chord = parse_chord(&keys)?;
            on_main_thread(&app, move || {
                if foreground_is_own() {
                    return Err(SELF_FOREGROUND.to_string());
                }
                Controller::new()?.chord(&chord)
            })
            .await
        }
    }
}

#[cfg(target_os = "windows")]
mod escape_hook {
    // 低级键盘钩子：真人按的 Esc 才算。Agent 自己发的 Esc 带 INJECTED 标记，不会把自己停掉。
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT,
    };

    const VK_ESCAPE: u32 = 0x1B;
    type OnEscape = Box<dyn Fn() + Send>;
    static ON_ESCAPE: Mutex<Option<OnEscape>> = Mutex::new(None);

    unsafe extern "system" fn hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
        if code >= 0 && message == WM_KEYDOWN as WPARAM {
            // SAFETY: WH_KEYBOARD_LL 的 lParam 就是这个结构。
            let key = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
            if key.vkCode == VK_ESCAPE && key.flags & LLKHF_INJECTED == 0 {
                // 钩子回调要尽快返回，否则系统会把钩子摘掉；这次按键也不再传给下面的应用。
                std::thread::spawn(|| {
                    if let Ok(callback) = ON_ESCAPE.lock() {
                        if let Some(callback) = callback.as_ref() {
                            callback();
                        }
                    }
                });
                return 1;
            }
        }
        // SAFETY: 按约定把其余消息传下去。
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) }
    }

    pub struct EscapeHook {
        thread_id: u32,
    }

    impl EscapeHook {
        // 钩子装不上时返回 None：遮罩照常显示，只是不能按 Esc，用户还能回 Pisper 停止这一轮。
        pub fn install(on_escape: impl Fn() + Send + 'static) -> Option<Self> {
            *ON_ESCAPE.lock().ok()? = Some(Box::new(on_escape));
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                // SAFETY: 钩子装在这个线程上，消息循环结束后在同一线程卸载。
                unsafe {
                    let handle =
                        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), std::ptr::null_mut(), 0);
                    let _ = sender.send(if handle.is_null() {
                        None
                    } else {
                        Some(GetCurrentThreadId())
                    });
                    if handle.is_null() {
                        return;
                    }
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {}
                    UnhookWindowsHookEx(handle);
                }
            });
            let thread_id = receiver.recv().ok().flatten()?;
            Some(Self { thread_id })
        }
    }

    impl Drop for EscapeHook {
        fn drop(&mut self) {
            // SAFETY: 线程还在等消息；已退出时调用只是失败。
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Ok(mut callback) = ON_ESCAPE.lock() {
                *callback = None;
            }
        }
    }
}
