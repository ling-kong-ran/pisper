//! 旧版壳通过组件更新装进 `<应用本地数据目录>/components/` 的前端、TUI 和 Runtime。
//!
//! 桌面程序现在只整体更新（见 shell_installer）：新壳不再下载组件，只在目录里还留着比内置版本新的
//! 组件时继续用它，免得从旧壳升上来的用户退回更旧的前端或 Runtime。
use pisper_component_updater::{resolve_installed, Component, InstalledComponent};
use semver::Version;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

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

#[cfg(test)]
mod tests {
    use super::installed_is_newer;
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
}
