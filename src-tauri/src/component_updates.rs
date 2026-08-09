use pisper_component_updater::{
    resolve_installed, Component, ComponentUpdater, InstalledComponent,
};
use semver::Version;
use serde::Serialize;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager, State};

use crate::{
    cli_manager,
    desktop_bridge::{log_component_update, UPDATER_PUBLIC_KEY},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentUpdateStatus {
    component: Component,
    state: String,
    current_version: String,
    available_version: String,
    message: String,
    release_url: String,
    notes: String,
    size: u64,
    transferred: u64,
    can_install: bool,
    restart_required: bool,
}

impl ComponentUpdateStatus {
    fn current(component: Component, version: String) -> Self {
        Self {
            component,
            state: "idle".into(),
            current_version: version,
            available_version: String::new(),
            message: String::new(),
            release_url: "https://github.com/ling-kong-ran/pisper/releases".into(),
            notes: String::new(),
            size: 0,
            transferred: 0,
            can_install: false,
            restart_required: false,
        }
    }
}

#[derive(Default)]
pub struct ComponentUpdateState {
    statuses: Mutex<HashMap<Component, ComponentUpdateStatus>>,
}

pub fn components_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join("components"))
        .map_err(|error| error.to_string())
}

fn bundled_version(component: Component) -> &'static str {
    match component {
        Component::Desktop => env!("PISPER_BUNDLED_DESKTOP_VERSION"),
        Component::Tui => env!("PISPER_BUNDLED_TUI_VERSION"),
        Component::Runtime => env!("PISPER_BUNDLED_RUNTIME_VERSION"),
    }
}

fn installed_is_newer(installed: &Version, bundled: &str) -> bool {
    let bundled = Version::parse(bundled).unwrap_or_else(|_| Version::new(0, 0, 0));
    installed > &bundled
}

pub fn installed_component(app: &AppHandle, component: Component) -> Option<InstalledComponent> {
    let installed = resolve_installed(&components_root(app).ok()?, component)
        .ok()
        .flatten()?;
    installed_is_newer(&installed.version, bundled_version(component)).then_some(installed)
}

pub fn effective_version(app: &AppHandle, component: Component) -> String {
    installed_component(app, component)
        .map(|installed| installed.version.to_string())
        .unwrap_or_else(|| bundled_version(component).to_string())
}

fn updater(app: &AppHandle) -> Result<ComponentUpdater, String> {
    ComponentUpdater::new(
        components_root(app)?,
        UPDATER_PUBLIC_KEY,
        &format!("Pisper-Desktop/{}", app.package_info().version),
    )
    .map_err(|error| error.to_string())
}

fn snapshot(app: &AppHandle, state: &ComponentUpdateState) -> Vec<ComponentUpdateStatus> {
    let statuses = state
        .statuses
        .lock()
        .expect("component update status poisoned");
    Component::ALL
        .into_iter()
        .map(|component| {
            statuses.get(&component).cloned().unwrap_or_else(|| {
                ComponentUpdateStatus::current(component, effective_version(app, component))
            })
        })
        .collect()
}

fn store(state: &ComponentUpdateState, status: ComponentUpdateStatus) {
    state
        .statuses
        .lock()
        .expect("component update status poisoned")
        .insert(status.component, status);
}

fn restart_pending(state: &ComponentUpdateState, component: Component) -> bool {
    state
        .statuses
        .lock()
        .expect("component update status poisoned")
        .get(&component)
        .is_some_and(|status| status.restart_required)
}

#[tauri::command]
pub fn desktop_component_update_status(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Vec<ComponentUpdateStatus> {
    snapshot(&app, &state)
}

#[tauri::command]
pub async fn desktop_check_component_updates(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Result<Vec<ComponentUpdateStatus>, String> {
    let updater = updater(&app).map_err(|error| {
        log_component_update(&app, &format!("updater initialization failed: {error}"));
        error
    })?;
    log_component_update(&app, "checking Desktop, TUI, and Runtime releases");
    for component in Component::ALL {
        let current_version = effective_version(&app, component);
        let pending_restart = restart_pending(&state, component);
        store(
            &state,
            ComponentUpdateStatus {
                state: "checking".into(),
                restart_required: pending_restart,
                ..ComponentUpdateStatus::current(component, current_version.clone())
            },
        );
        let status = match updater.latest(component).await {
            Ok(release) => {
                let current =
                    Version::parse(&current_version).unwrap_or_else(|_| Version::new(0, 0, 0));
                let latest = Version::parse(&release.version).map_err(|error| {
                    format!("Invalid {} release version: {error}", component.name())
                })?;
                let available = latest > current;
                ComponentUpdateStatus {
                    component,
                    state: if available { "available" } else { "current" }.into(),
                    current_version,
                    available_version: release.version,
                    message: String::new(),
                    release_url: release.release_url,
                    notes: release.notes,
                    size: release.size,
                    transferred: 0,
                    can_install: available,
                    restart_required: pending_restart,
                }
            }
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} release check failed: {error:#}", component.name()),
                );
                ComponentUpdateStatus {
                    component,
                    state: "error".into(),
                    current_version,
                    message: format!("{error:#}"),
                    restart_required: pending_restart,
                    ..ComponentUpdateStatus::current(component, String::new())
                }
            }
        };
        store(&state, status);
    }
    Ok(snapshot(&app, &state))
}

#[tauri::command]
pub async fn desktop_install_component_updates(
    app: AppHandle,
    state: State<'_, ComponentUpdateState>,
) -> Result<Vec<ComponentUpdateStatus>, String> {
    let updater = updater(&app).map_err(|error| {
        log_component_update(&app, &format!("updater initialization failed: {error}"));
        error
    })?;
    log_component_update(&app, "component update batch started");
    for component in Component::ALL {
        let current_version = effective_version(&app, component);
        let pending_restart = restart_pending(&state, component);
        let release = match updater.latest(component).await {
            Ok(release) => release,
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} release check failed: {error:#}", component.name()),
                );
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        message: format!("{error:#}"),
                        restart_required: pending_restart,
                        ..ComponentUpdateStatus::current(component, String::new())
                    },
                );
                continue;
            }
        };
        let current = Version::parse(&current_version).unwrap_or_else(|_| Version::new(0, 0, 0));
        let latest = match Version::parse(&release.version) {
            Ok(version) => version,
            Err(error) => {
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        available_version: release.version,
                        message: error.to_string(),
                        release_url: release.release_url,
                        notes: release.notes,
                        size: release.size,
                        transferred: 0,
                        can_install: false,
                        restart_required: pending_restart,
                    },
                );
                continue;
            }
        };
        if latest <= current {
            store(
                &state,
                ComponentUpdateStatus {
                    component,
                    state: "current".into(),
                    current_version,
                    available_version: release.version,
                    release_url: release.release_url,
                    notes: release.notes,
                    restart_required: pending_restart,
                    ..ComponentUpdateStatus::current(component, String::new())
                },
            );
            continue;
        }
        log_component_update(
            &app,
            &format!(
                "{} {} download started ({} bytes)",
                component.name(),
                release.version,
                release.size
            ),
        );
        store(
            &state,
            ComponentUpdateStatus {
                component,
                state: "downloading".into(),
                current_version: current_version.clone(),
                available_version: release.version.clone(),
                release_url: release.release_url.clone(),
                notes: release.notes.clone(),
                size: release.size,
                ..ComponentUpdateStatus::current(component, current_version.clone())
            },
        );
        let progress_state = state.inner();
        let installed = match updater
            .install_with_progress(&release, |transferred, _total| {
                let mut statuses = progress_state
                    .statuses
                    .lock()
                    .expect("component update status poisoned");
                if let Some(status) = statuses.get_mut(&component) {
                    status.transferred = transferred;
                }
            })
            .await
        {
            Ok(installed) => installed,
            Err(error) => {
                log_component_update(
                    &app,
                    &format!("{} install failed: {error:#}", component.name()),
                );
                store(
                    &state,
                    ComponentUpdateStatus {
                        component,
                        state: "error".into(),
                        current_version,
                        available_version: release.version,
                        message: format!("{error:#}"),
                        release_url: release.release_url,
                        notes: release.notes,
                        size: release.size,
                        transferred: 0,
                        can_install: true,
                        restart_required: pending_restart,
                    },
                );
                continue;
            }
        };

        if component == Component::Tui {
            let _ = cli_manager::refresh_managed_cli(&app);
        }
        log_component_update(
            &app,
            &format!(
                "{} {} downloaded, verified, and installed ({} bytes)",
                component.name(),
                installed.version,
                release.size
            ),
        );
        store(
            &state,
            ComponentUpdateStatus {
                component,
                state: "installed".into(),
                current_version: installed.version.to_string(),
                available_version: installed.version.to_string(),
                message: String::new(),
                release_url: release.release_url,
                notes: release.notes,
                size: release.size,
                transferred: release.size,
                can_install: false,
                restart_required: matches!(component, Component::Desktop | Component::Runtime),
            },
        );
    }
    Ok(snapshot(&app, &state))
}

#[tauri::command]
pub fn desktop_restart_for_component_update(app: AppHandle) -> bool {
    log_component_update(&app, "restarting to activate Desktop or Runtime components");
    crate::stop_sidecar(&app);
    app.cleanup_before_exit();
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::{installed_is_newer, ComponentUpdateStatus};
    use pisper_component_updater::Component;
    use semver::Version;

    #[test]
    fn older_or_equal_installed_components_do_not_override_bundled_components() {
        assert!(!installed_is_newer(
            &Version::parse("1.1.9").unwrap(),
            "1.2.0"
        ));
        assert!(!installed_is_newer(
            &Version::parse("1.2.0").unwrap(),
            "1.2.0"
        ));
        assert!(installed_is_newer(
            &Version::parse("1.2.1").unwrap(),
            "1.2.0"
        ));
    }

    #[test]
    fn component_status_starts_idle_and_component_scoped() {
        let status = ComponentUpdateStatus::current(Component::Runtime, "1.2.3".into());
        assert_eq!(status.state, "idle");
        assert_eq!(status.current_version, "1.2.3");
        assert_eq!(status.transferred, 0);
        assert_eq!(status.component, Component::Runtime);
    }
}
