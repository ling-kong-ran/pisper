use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

const PET_WIDTH: f64 = 192.0;
const PET_HEIGHT: f64 = 288.0;

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub struct DesktopPetWindowState {
    bootstrap_url: String,
}

impl DesktopPetWindowState {
    pub fn new(bootstrap_url: String) -> Self {
        Self { bootstrap_url }
    }
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

    WebviewWindowBuilder::new(app, "desktop-pet", WebviewUrl::External(url))
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
    Ok(())
}

pub(crate) fn show_pet_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("desktop-pet")
        .ok_or_else(|| "Desktop pet window is unavailable.".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn desktop_pet_apply_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    if enabled {
        let state = app
            .try_state::<DesktopPetWindowState>()
            .ok_or_else(|| "Desktop pet window state is unavailable.".to_string())?;
        create_pet_window(&app, &state.bootstrap_url)?;
        show_pet_window(&app)?;
    } else if let Some(window) = app.get_webview_window("desktop-pet") {
        window.destroy().map_err(|error| error.to_string())?;
    }
    Ok(true)
}

#[tauri::command]
pub fn desktop_pet_set_visible(app: AppHandle, visible: bool) -> bool {
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
    crate::show_desktop_pet_context_menu(&app)
}

#[tauri::command]
pub fn desktop_pet_sync_menu(app: AppHandle, enabled: bool) {
    crate::sync_desktop_pet_menu_enabled(&app, enabled);
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
