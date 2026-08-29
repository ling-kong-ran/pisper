use std::time::Duration;
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, PhysicalPosition, Url, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

const PET_WIDTH: f64 = 192.0;
const PET_HEIGHT: f64 = 288.0;
const PET_MARGIN: f64 = 20.0;

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub struct DesktopPetWindowState {
    bootstrap_url: String,
    lifecycle: tokio::sync::Mutex<()>,
}

impl DesktopPetWindowState {
    pub fn new(bootstrap_url: String) -> Self {
        Self {
            bootstrap_url,
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }
}

fn pet_window_is_visible_on_screen(window: &WebviewWindow) -> bool {
    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    let left = i64::from(position.x);
    let top = i64::from(position.y);
    let right = left + i64::from(size.width);
    let bottom = top + i64::from(size.height);

    monitors.iter().any(|monitor| {
        let work_area = monitor.work_area();
        let work_left = i64::from(work_area.position.x);
        let work_top = i64::from(work_area.position.y);
        let work_right = work_left + i64::from(work_area.size.width);
        let work_bottom = work_top + i64::from(work_area.size.height);
        right > work_left && left < work_right && bottom > work_top && top < work_bottom
    })
}

fn place_pet_window(window: &WebviewWindow) -> Result<(), String> {
    let Some(monitor) = window
        .primary_monitor()
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let work_area = monitor.work_area();
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let margin = (PET_MARGIN * monitor.scale_factor()).round() as i64;
    let left = i64::from(work_area.position.x);
    let top = i64::from(work_area.position.y);
    let x = (left + i64::from(work_area.size.width) - i64::from(size.width) - margin).max(left);
    let y = (top + i64::from(work_area.size.height) - i64::from(size.height) - margin).max(top);
    window
        .set_position(PhysicalPosition::new(x as i32, y as i32))
        .map_err(|error| error.to_string())
}

pub fn create_pet_window(app: &AppHandle, bootstrap_url: &str) -> Result<(), String> {
    if app.get_webview_window("desktop-pet").is_some() {
        return Ok(());
    }
    let mut url = Url::parse(bootstrap_url).map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("next", "/tauri-pet.html");
    let allowed = url.clone();
    let navigation_app = app.clone();
    let new_window_app = app.clone();

    let window = WebviewWindowBuilder::new(app, "desktop-pet", WebviewUrl::External(url))
        .title("Pisper Pet")
        .inner_size(PET_WIDTH, PET_HEIGHT)
        .min_inner_size(PET_WIDTH, PET_HEIGHT)
        .max_inner_size(PET_WIDTH, PET_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // 桌宠是悬浮提示层，不应抢走主窗口的键盘焦点。
        .focused(false)
        .focusable(false)
        .visible(false)
        .on_navigation(move |target| {
            if same_origin(target, &allowed) || target.scheme() == "about" {
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
        .build()
        .map_err(|error| error.to_string())?;
    place_pet_window(&window)?;
    Ok(())
}

pub(crate) fn show_pet_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("desktop-pet")
        .ok_or_else(|| "Desktop pet window is unavailable.".to_string())?;
    if !pet_window_is_visible_on_screen(&window) {
        place_pet_window(&window)?;
    }
    if !window.is_visible().map_err(|error| error.to_string())? {
        window.unminimize().map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn destroy_pet_window_and_wait(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("desktop-pet") {
        window.destroy().map_err(|error| error.to_string())?;
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.get_webview_window("desktop-pet").is_some() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "Desktop pet window did not finish closing within 2 seconds.".to_string())
}

#[tauri::command]
pub async fn desktop_pet_apply_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let state = app
        .try_state::<DesktopPetWindowState>()
        .ok_or_else(|| "Desktop pet window state is unavailable.".to_string())?;
    let _lifecycle = state.lifecycle.lock().await;
    if enabled {
        create_pet_window(&app, &state.bootstrap_url)?;
        if let Err(first_error) = show_pet_window(&app) {
            destroy_pet_window_and_wait(&app).await?;
            create_pet_window(&app, &state.bootstrap_url)?;
            show_pet_window(&app)
                .map_err(|error| format!("{first_error}; retry failed: {error}"))?;
        }
    } else if let Some(window) = app.get_webview_window("desktop-pet") {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(true)
}

#[tauri::command]
pub async fn desktop_pet_set_visible(app: AppHandle, visible: bool) -> bool {
    let Some(state) = app.try_state::<DesktopPetWindowState>() else {
        return false;
    };
    let _lifecycle = state.lifecycle.lock().await;
    if visible {
        return show_pet_window(&app).is_ok();
    }
    app.get_webview_window("desktop-pet")
        .is_some_and(|window| window.hide().is_ok())
}

#[tauri::command]
pub fn desktop_pet_start_dragging(app: AppHandle) -> bool {
    app.get_webview_window("desktop-pet")
        .is_some_and(|window| window.start_dragging().is_ok())
}

#[tauri::command]
pub fn desktop_pet_show_context_menu(app: AppHandle) -> bool {
    crate::desktop_shell::show_desktop_pet_context_menu(&app)
}

#[tauri::command]
pub fn desktop_pet_sync_menu(app: AppHandle, enabled: bool) {
    crate::desktop_shell::sync_desktop_pet_menu_enabled(&app, enabled);
}

#[tauri::command]
pub fn desktop_show_main_window(app: AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    let _ = window.show();
    let _ = window.unminimize();
    window.set_focus().is_ok()
}
