mod cli_manager;
mod component_updates;
mod desktop_bridge;
mod desktop_pet;
mod desktop_terminal;

#[cfg(all(test, target_os = "windows"))]
#[link(name = "pisper_test_resource", kind = "static")]
extern "C" {}

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
    webview::NewWindowResponse,
    AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

const READY_PREFIX: &str = "PISPER_SIDECAR_READY ";
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(30);
const SIDECAR_DESCRIPTOR_NAME: &str = "desktop-sidecar.json";
const DESKTOP_BRIDGE_SCRIPT: &str = include_str!("desktop-bridge.js");

#[derive(serde::Deserialize)]
pub(crate) struct SidecarReady {
    pub(crate) url: String,
    #[serde(rename = "bootstrapUrl")]
    pub(crate) bootstrap_url: String,
    pub(crate) pid: u32,
    #[serde(default, rename = "desktopPetRunning")]
    pub(crate) desktop_pet_running: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SidecarDescriptor {
    version: u8,
    url: String,
    token: String,
    pid: u32,
}

struct ManagedSidecar {
    child: Child,
    pid: u32,
}

struct SidecarState(Mutex<Option<ManagedSidecar>>);
struct LifecycleState {
    quitting: AtomicBool,
}
struct TrayMenuState {
    show: MenuItem<tauri::Wry>,
    pet: CheckMenuItem<tauri::Wry>,
    petdex: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}
struct PetContextMenuState {
    menu: Menu<tauri::Wry>,
    show: MenuItem<tauri::Wry>,
    petdex: MenuItem<tauri::Wry>,
    hide: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn platform_binary_name() -> &'static str {
    if cfg!(windows) {
        "pisper-sidecar.exe"
    } else {
        "pisper-sidecar"
    }
}

fn pipe_logs<R: Read + Send + 'static>(reader: R, label: &'static str) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            eprintln!("[{label}] {line}");
        }
    });
}

fn development_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must have a project parent")
        .to_path_buf()
}

fn frontend_root(app: &tauri::App, app_root: &Path) -> Result<PathBuf, String> {
    if !cfg!(debug_assertions) {
        if let Some(installed) = component_updates::installed_component(
            app.handle(),
            pisper_component_updater::Component::Desktop,
        ) {
            return installed
                .frontend_root()
                .ok_or_else(|| "Installed Pisper desktop payload is missing.".to_string());
        }
        let packaged = app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("desktop")
            .join("dist");
        if packaged.join("index.html").is_file() {
            return Ok(packaged);
        }
    }
    Ok(app_root.join("dist"))
}

fn sidecar_command(app: &tauri::App, allow_installed: bool) -> Result<(Command, bool), String> {
    let mut command;
    let app_root;
    let using_installed;

    if cfg!(debug_assertions) {
        using_installed = false;
        app_root = development_root();
        command = Command::new("node");
        command.arg(app_root.join("runtime").join("sidecar.mjs"));
        command.current_dir(&app_root);
    } else if allow_installed {
        let installed = component_updates::installed_component(
            app.handle(),
            pisper_component_updater::Component::Runtime,
        )
        .ok_or_else(|| "No installed Pisper runtime component is active.".to_string())?;
        using_installed = true;
        app_root = installed
            .runtime_root()
            .ok_or_else(|| "Installed Pisper runtime payload is missing.".to_string())?;
        command = Command::new(installed.executable());
    } else {
        using_installed = false;
        let executable_dir = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .parent()
            .ok_or_else(|| "Pisper executable has no parent directory.".to_string())?
            .to_path_buf();
        app_root = app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("sidecar-runtime");
        command = Command::new(executable_dir.join(platform_binary_name()));
    }

    let frontend_root = frontend_root(app, &app_root)?;
    command
        .env("PISPER_APP_ROOT", &app_root)
        .env("PISPER_FRONTEND_ROOT", frontend_root)
        .env("PISPER_PARENT_PID", std::process::id().to_string())
        .env("PISPER_EXIT_ON_STDIN_CLOSE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    Ok((command, using_installed))
}

fn start_sidecar_process(mut command: Command) -> Result<(Child, SidecarReady), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start Pisper sidecar: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Pisper sidecar stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Pisper sidecar stderr was not captured.".to_string())?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);

    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(payload) = line.strip_prefix(READY_PREFIX) {
                let parsed = serde_json::from_str::<SidecarReady>(payload)
                    .map_err(|error| format!("Invalid sidecar readiness payload: {error}"));
                let _ = ready_tx.send(parsed);
            } else {
                eprintln!("[sidecar] {line}");
            }
        }
    });
    pipe_logs(stderr, "sidecar:error");

    match ready_rx.recv_timeout(SIDECAR_TIMEOUT) {
        Ok(Ok(ready)) => Ok((child, ready)),
        Ok(Err(error)) => {
            force_stop_sidecar_process(&mut child);
            Err(error)
        }
        Err(_) => {
            force_stop_sidecar_process(&mut child);
            Err("Pisper sidecar did not become ready within 30 seconds.".to_string())
        }
    }
}

fn start_sidecar(app: &tauri::App) -> Result<(Child, SidecarReady), String> {
    let installed_available = component_updates::installed_component(
        app.handle(),
        pisper_component_updater::Component::Runtime,
    )
    .is_some()
        && !cfg!(debug_assertions);
    let (command, using_installed) = sidecar_command(app, installed_available)?;
    match start_sidecar_process(command) {
        Ok(result) => Ok(result),
        Err(component_error) if using_installed => {
            eprintln!("[component-update] Installed runtime failed; using bundled runtime: {component_error}");
            if let Ok(root) = component_updates::components_root(app.handle()) {
                let _ = pisper_component_updater::deactivate_component(
                    &root,
                    pisper_component_updater::Component::Runtime,
                );
            }
            let (fallback, _) = sidecar_command(app, false)?;
            start_sidecar_process(fallback).map_err(|fallback_error| {
                format!(
                    "Installed runtime failed ({component_error}); bundled runtime also failed ({fallback_error})"
                )
            })
        }
        Err(error) => Err(error),
    }
}

fn sidecar_descriptor_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join(SIDECAR_DESCRIPTOR_NAME))
}

fn publish_sidecar_descriptor(app: &AppHandle, ready: &SidecarReady) -> Result<(), String> {
    let bootstrap = Url::parse(&ready.bootstrap_url).map_err(|error| error.to_string())?;
    let token = bootstrap
        .query_pairs()
        .find_map(|(name, value)| (name == "token").then(|| value.into_owned()))
        .ok_or_else(|| "Pisper sidecar bootstrap URL has no desktop token.".to_string())?;
    let path = sidecar_descriptor_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let payload = serde_json::to_vec(&SidecarDescriptor {
        version: 1,
        url: ready.url.clone(),
        token,
        pid: ready.pid,
    })
    .map_err(|error| error.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(&payload)
        .map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())
}

fn remove_sidecar_descriptor(app: &AppHandle, pid: u32) {
    let Ok(path) = sidecar_descriptor_path(app) else {
        return;
    };
    let owned = std::fs::read(&path)
        .ok()
        .and_then(|payload| serde_json::from_slice::<SidecarDescriptor>(&payload).ok())
        .is_some_and(|descriptor| descriptor.pid == pid);
    if owned {
        let _ = std::fs::remove_file(path);
    }
}

fn wait_for_sidecar_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    false
}

#[cfg(windows)]
fn terminate_sidecar_tree(pid: u32) {
    let pid = pid.to_string();
    let _ = Command::new("taskkill.exe")
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn terminate_sidecar_tree(_: u32) {}

fn force_stop_sidecar_process(child: &mut Child) {
    terminate_sidecar_tree(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn stop_sidecar(app: &AppHandle) {
    let Some(state) = app.try_state::<SidecarState>() else {
        return;
    };
    let Some(managed) = state.0.lock().expect("sidecar state poisoned").take() else {
        return;
    };
    remove_sidecar_descriptor(app, managed.pid);
    let mut child = managed.child;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"shutdown\n");
        let _ = stdin.flush();
    }

    if !wait_for_sidecar_exit(&mut child, Duration::from_secs(5)) {
        force_stop_sidecar_process(&mut child);
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn create_main_window(app: &tauri::App, ready: &SidecarReady) -> Result<(), String> {
    let url = Url::parse(&ready.bootstrap_url).map_err(|error| error.to_string())?;
    let allowed = Url::parse(&ready.url).map_err(|error| error.to_string())?;
    let navigation_app = app.handle().clone();
    let new_window_app = app.handle().clone();

    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        // Tauri's native Windows file-drop handler intercepts HTML5 drag events used by React Flow.
        .disable_drag_drop_handler()
        .title("Pisper")
        .inner_size(1440.0, 920.0)
        .min_inner_size(980.0, 680.0)
        .center()
        .initialization_script(DESKTOP_BRIDGE_SCRIPT)
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

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub(crate) fn set_tray_language(app: &AppHandle, language: &str) {
    let english = language == "en-US";
    if let Some(menu) = app.try_state::<TrayMenuState>() {
        let _ = menu.show.set_text(if english {
            "Show Pisper"
        } else {
            "显示 Pisper"
        });
        let _ = menu.pet.set_text(if english {
            "Desktop pet"
        } else {
            "桌面宠物"
        });
        let _ = menu.petdex.set_text(if english {
            "Pet provided by Petdex"
        } else {
            "桌宠来自 Petdex"
        });
        let _ = menu.quit.set_text(if english {
            "Quit Pisper"
        } else {
            "退出 Pisper"
        });
    }
    if let Some(menu) = app.try_state::<PetContextMenuState>() {
        let _ = menu.show.set_text(if english {
            "Show Pisper"
        } else {
            "显示 Pisper"
        });
        let _ = menu.petdex.set_text(if english {
            "Pet provided by Petdex"
        } else {
            "桌宠来自 Petdex"
        });
        let _ = menu.hide.set_text(if english {
            "Hide desktop pet"
        } else {
            "隐藏桌宠"
        });
        let _ = menu.quit.set_text(if english {
            "Quit Pisper"
        } else {
            "退出 Pisper"
        });
    }
}

pub(crate) fn sync_desktop_pet_menu_enabled(app: &AppHandle, enabled: bool) {
    if let Some(menu) = app.try_state::<TrayMenuState>() {
        let _ = menu.pet.set_checked(enabled);
        let _ = menu.petdex.set_enabled(enabled);
    }
}

fn request_desktop_pet_enabled(app: &AppHandle, enabled: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.eval(format!(
        "void window.pisperDesktop?.setPetEnabled?.({enabled});"
    ));
}

fn quit_application(app: &AppHandle) {
    if let Some(state) = app.try_state::<LifecycleState>() {
        state.quitting.store(true, Ordering::SeqCst);
    }
    stop_sidecar(app);
    app.exit(0);
}

fn handle_desktop_menu_event(app: &AppHandle, id: &str) {
    match id {
        "tray_show" | "pet_show" => show_main_window(app),
        "tray_pet" => {
            if let Some(menu) = app.try_state::<TrayMenuState>() {
                request_desktop_pet_enabled(app, menu.pet.is_checked().unwrap_or(false));
            }
        }
        "tray_petdex" | "pet_petdex" => {
            let _ = app.opener().open_url("https://petdex.dev", None::<&str>);
        }
        "pet_hide" => request_desktop_pet_enabled(app, false),
        "tray_quit" | "pet_quit" => quit_application(app),
        _ => {}
    }
}

fn create_pet_context_menu(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "pet_show", "显示 Pisper", true, None::<&str>)?;
    let petdex = MenuItem::with_id(app, "pet_petdex", "桌宠来自 Petdex", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "pet_hide", "隐藏桌宠", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "pet_quit", "退出 Pisper", true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .item(&petdex)
        .separator()
        .item(&hide)
        .item(&quit)
        .build()?;
    app.manage(PetContextMenuState {
        menu,
        show,
        petdex,
        hide,
        quit,
    });
    Ok(())
}

pub(crate) fn show_desktop_pet_context_menu(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("desktop-pet") else {
        return false;
    };
    let Some(state) = app.try_state::<PetContextMenuState>() else {
        return false;
    };
    window.popup_menu(&state.menu).is_ok()
}

fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "tray_show", "显示 Pisper", true, None::<&str>)?;
    let pet = CheckMenuItem::with_id(app, "tray_pet", "桌面宠物", true, false, None::<&str>)?;
    let petdex = MenuItem::with_id(app, "tray_petdex", "桌宠来自 Petdex", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray_quit", "退出 Pisper", true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .item(&pet)
        .item(&petdex)
        .separator()
        .item(&quit)
        .build()?;
    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Pisper")
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    app.manage(TrayMenuState {
        show,
        pet,
        petdex,
        quit,
    });
    Ok(())
}

pub fn run() {
    let process_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let cli_args = process_args
        .first()
        .is_some_and(|value| value == "--pisper-cli")
        .then(|| process_args[1..].to_vec());
    let mut builder = tauri::Builder::default()
        .on_menu_event(|app, event| handle_desktop_menu_event(app, event.id().as_ref()));
    if cli_args.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main_window(app);
        }));
    }
    let cli_args_for_setup = cli_args.clone();
    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .manage(component_updates::ComponentUpdateState::default())
        .manage(desktop_terminal::DesktopTerminalState::default())
        .manage(LifecycleState {
            quitting: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            desktop_bridge::desktop_get_app_info,
            desktop_bridge::desktop_pick_directory,
            desktop_bridge::desktop_pick_files,
            desktop_bridge::desktop_set_language,
            cli_manager::desktop_get_cli_status,
            cli_manager::desktop_install_cli,
            cli_manager::desktop_uninstall_cli,
            component_updates::desktop_component_update_status,
            component_updates::desktop_check_component_updates,
            component_updates::desktop_install_component_updates,
            component_updates::desktop_restart_for_component_update,
            desktop_bridge::desktop_open_url,
            desktop_bridge::desktop_open_releases,
            desktop_bridge::desktop_open_update_log,
            desktop_bridge::desktop_get_notification_status,
            desktop_bridge::desktop_open_notification_settings,
            desktop_bridge::desktop_show_notification,
            desktop_terminal::desktop_terminal_profiles,
            desktop_terminal::desktop_terminal_create,
            desktop_terminal::desktop_terminal_write,
            desktop_terminal::desktop_terminal_resize,
            desktop_terminal::desktop_terminal_close,
            desktop_terminal::desktop_terminal_close_all,
            desktop_pet::desktop_pet_apply_enabled,
            desktop_pet::desktop_pet_set_visible,
            desktop_pet::desktop_pet_start_dragging,
            desktop_pet::desktop_pet_show_context_menu,
            desktop_pet::desktop_pet_sync_menu,
            desktop_pet::desktop_show_main_window,
        ])
        .setup(move |app| {
            if let Some(args) = cli_args_for_setup.as_deref() {
                let exit_code = match cli_manager::run_bundled_cli(app, args) {
                    Ok(code) => code,
                    Err(error) => {
                        eprintln!("{error}");
                        1
                    }
                };
                app.handle().exit(exit_code);
                return Ok(());
            }

            if let Err(error) = cli_manager::refresh_managed_cli(app.handle()) {
                eprintln!("Failed to refresh the managed Pisper CLI: {error}");
            }

            let (child, ready) = start_sidecar(app)?;
            app.manage(SidecarState(Mutex::new(Some(ManagedSidecar {
                child,
                pid: ready.pid,
            }))));
            app.manage(desktop_pet::DesktopPetWindowState::new(
                ready.bootstrap_url.clone(),
            ));
            if let Err(error) = publish_sidecar_descriptor(app.handle(), &ready) {
                stop_sidecar(app.handle());
                return Err(error.into());
            }
            let result = create_tray(app)
                .map_err(|error| error.to_string())
                .and_then(|_| create_pet_context_menu(app).map_err(|error| error.to_string()))
                .and_then(|_| create_main_window(app, &ready))
                .and_then(|_| {
                    sync_desktop_pet_menu_enabled(app.handle(), ready.desktop_pet_running);
                    if ready.desktop_pet_running {
                        desktop_pet::create_pet_window(app.handle(), &ready.bootstrap_url)?;
                        desktop_pet::show_pet_window(app.handle())
                    } else {
                        Ok(())
                    }
                });
            if let Err(error) = result {
                stop_sidecar(app.handle());
                return Err(error.into());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let quitting = window
                    .app_handle()
                    .state::<LifecycleState>()
                    .quitting
                    .load(Ordering::SeqCst);
                if !quitting {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        });

    let application = builder
        .build(tauri::generate_context!())
        .expect("failed to build Pisper WebView application");
    application.run(|app, event| {
        #[cfg(target_os = "macos")]
        if matches!(&event, RunEvent::Reopen { .. }) {
            show_main_window(app);
        }
        if matches!(&event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
            if let Some(state) = app.try_state::<desktop_terminal::DesktopTerminalState>() {
                desktop_terminal::close_all(&state);
            }
            stop_sidecar(app);
        }
    });
}
