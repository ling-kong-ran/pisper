use pisper_component_updater::Component;
use serde::Serialize;
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};
use tauri::{AppHandle, Manager};

const MANAGED_MARKER: &str = "PISPER_CLI_MANAGED_V1";
#[cfg(any(not(windows), test))]
const PROFILE_START: &str = "# >>> Pisper CLI >>>";
#[cfg(any(not(windows), test))]
const PROFILE_END: &str = "# <<< Pisper CLI <<<";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallStatus {
    supported: bool,
    installed: bool,
    path_configured: bool,
    needs_repair: bool,
    command: &'static str,
    install_path: String,
}

fn public_command_name() -> &'static str {
    if cfg!(windows) {
        "pisper.exe"
    } else {
        "pisper"
    }
}

fn payload_name() -> &'static str {
    if cfg!(windows) {
        "pisper-cli.exe"
    } else {
        "pisper-cli"
    }
}

fn sidecar_name() -> &'static str {
    if cfg!(windows) {
        "pisper-sidecar.exe"
    } else {
        "pisper-sidecar"
    }
}

fn executable_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Pisper executable has no parent directory.".to_string())
}

fn bundled_payload() -> Result<PathBuf, String> {
    let packaged = executable_dir()?.join(payload_name());
    if packaged.is_file() {
        return Ok(packaged);
    }

    if cfg!(debug_assertions) {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| "src-tauri must have a project parent.".to_string())?;
        for profile in ["debug", "release"] {
            let candidate = root
                .join("src-tui")
                .join("target")
                .join(profile)
                .join(format!("pisper{suffix}"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err("This Pisper build does not contain the terminal client.".to_string())
}

fn preferred_payload(app: &AppHandle) -> Result<(PathBuf, String), String> {
    if let Some(installed) = crate::component_updates::installed_component(app, Component::Tui) {
        return Ok((installed.executable(), installed.version.to_string()));
    }
    Ok((
        bundled_payload()?,
        env!("PISPER_BUNDLED_TUI_VERSION").to_string(),
    ))
}

fn preferred_runtime(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    if let Some(installed) = crate::component_updates::installed_component(app, Component::Runtime)
    {
        return Ok((
            installed.executable(),
            installed
                .runtime_root()
                .ok_or_else(|| "Installed runtime payload is missing.".to_string())?,
        ));
    }
    let executable_dir = executable_dir()?;
    Ok((
        executable_dir.join(sidecar_name()),
        app.path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("sidecar-runtime"),
    ))
}

fn install_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        app.path()
            .app_local_data_dir()
            .map(|path| path.join("cli"))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        app.path()
            .home_dir()
            .map(|path| path.join(".local").join("bin"))
            .map_err(|error| error.to_string())
    }
}

fn install_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(install_dir(app)?.join(public_command_name()))
}

fn marker_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(install_dir(app)?.join(".pisper-cli-managed"))
}

#[cfg(any(windows, test))]
fn managed_marker(version: &str, payload_size: u64) -> String {
    format!("{MANAGED_MARKER}\nversion={version}\npayload_size={payload_size}\n")
}

#[cfg(windows)]
fn expected_marker(app: &AppHandle) -> Result<String, String> {
    let (payload, version) = preferred_payload(app)?;
    let payload_size = fs::metadata(&payload)
        .map_err(|error| format!("Failed to inspect {}: {error}", payload.display()))?
        .len();
    Ok(managed_marker(&version, payload_size))
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Failed to read {}: {error}", path.display())),
    }
}

fn write_file(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, content).map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

#[cfg(not(windows))]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

#[cfg(not(windows))]
fn expected_launcher(app: &AppHandle) -> Result<String, String> {
    if crate::component_updates::installed_component(app, Component::Tui).is_none() {
        if let Some(app_image) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
            if app_image.is_file() {
                return Ok(format!(
                    "#!/bin/sh\n# {MANAGED_MARKER}\nexec {} --pisper-cli \"$@\"\n",
                    shell_quote(&app_image)
                ));
            }
        }
    }
    let (payload, _) = preferred_payload(app)?;
    let (sidecar, runtime) = preferred_runtime(app)?;
    Ok(format!(
        "#!/bin/sh\n# {MANAGED_MARKER}\nPISPER_SIDECAR_PATH={} PISPER_APP_ROOT={} exec {} \"$@\"\n",
        shell_quote(&sidecar),
        shell_quote(&runtime),
        shell_quote(&payload)
    ))
}

#[cfg(not(windows))]
fn profile_path(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app.path().home_dir().map_err(|error| error.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&shell)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    Ok(match name {
        "fish" => home
            .join(".config")
            .join("fish")
            .join("conf.d")
            .join("pisper.fish"),
        "zsh" if cfg!(target_os = "macos") => home.join(".zprofile"),
        "zsh" => home.join(".zshrc"),
        "bash" if cfg!(target_os = "macos") => home.join(".bash_profile"),
        "bash" => home.join(".bashrc"),
        _ if cfg!(target_os = "macos") => home.join(".zprofile"),
        _ => home.join(".profile"),
    })
}

#[cfg(not(windows))]
fn profile_body(path: &Path) -> &'static str {
    if path.extension().and_then(|value| value.to_str()) == Some("fish") {
        "set -gx PATH \"$HOME/.local/bin\" $PATH"
    } else {
        "export PATH=\"$HOME/.local/bin:$PATH\""
    }
}

#[cfg(any(not(windows), test))]
fn remove_managed_block(source: &str) -> Result<String, String> {
    let Some(start) = source.find(PROFILE_START) else {
        if source.contains(PROFILE_END) {
            return Err("The Pisper PATH block in the shell profile is incomplete.".to_string());
        }
        return Ok(source.to_string());
    };
    let tail = &source[start + PROFILE_START.len()..];
    let Some(relative_end) = tail.find(PROFILE_END) else {
        return Err("The Pisper PATH block in the shell profile is incomplete.".to_string());
    };
    let mut end = start + PROFILE_START.len() + relative_end + PROFILE_END.len();
    if source[end..].starts_with("\r\n") {
        end += 2;
    } else if source[end..].starts_with('\n') {
        end += 1;
    }
    let mut result = format!("{}{}", &source[..start], &source[end..]);
    if result.contains(PROFILE_START) || result.contains(PROFILE_END) {
        return Err("Multiple Pisper PATH blocks were found in the shell profile.".to_string());
    }
    while result.ends_with("\n\n\n") {
        result.pop();
    }
    Ok(result)
}

#[cfg(not(windows))]
fn install_profile_path(path: &Path) -> Result<(), String> {
    let original = read_optional(path)?.unwrap_or_default();
    let mut updated = remove_managed_block(&original)?;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(PROFILE_START);
    updated.push('\n');
    updated.push_str(profile_body(path));
    updated.push('\n');
    updated.push_str(PROFILE_END);
    updated.push('\n');
    write_file(path, updated.as_bytes())
}

#[cfg(not(windows))]
fn candidate_profiles(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let home = app.path().home_dir().map_err(|error| error.to_string())?;
    Ok(vec![
        home.join(".profile"),
        home.join(".zprofile"),
        home.join(".zshrc"),
        home.join(".bash_profile"),
        home.join(".bashrc"),
        home.join(".config")
            .join("fish")
            .join("conf.d")
            .join("pisper.fish"),
    ])
}

#[cfg(not(windows))]
fn remove_profile_paths(app: &AppHandle) -> Result<(), String> {
    for path in candidate_profiles(app)? {
        let Some(original) = read_optional(&path)? else {
            continue;
        };
        if !original.contains(PROFILE_START) && !original.contains(PROFILE_END) {
            continue;
        }
        let updated = remove_managed_block(&original)?;
        if updated.trim().is_empty()
            && path.file_name().and_then(|value| value.to_str()) == Some("pisper.fish")
        {
            fs::remove_file(&path)
                .map_err(|error| format!("Failed to remove {}: {error}", path.display()))?;
        } else {
            write_file(&path, updated.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn profile_is_configured(app: &AppHandle) -> bool {
    candidate_profiles(app).is_ok_and(|paths| {
        paths.into_iter().any(|path| {
            fs::read_to_string(path)
                .is_ok_and(|value| value.contains(PROFILE_START) && value.contains(PROFILE_END))
        })
    })
}

#[cfg(not(windows))]
fn install_unix(app: &AppHandle) -> Result<(), String> {
    let target = install_path(app)?;
    if let Some(existing) = read_optional(&target)? {
        if !existing.contains(MANAGED_MARKER) {
            return Err(format!(
                "{} already exists and is not managed by Pisper.",
                target.display()
            ));
        }
    }
    let launcher = expected_launcher(app)?;
    write_file(&target, launcher.as_bytes())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("Failed to make {} executable: {error}", target.display()))?;
    install_profile_path(&profile_path(app)?)
}

#[cfg(windows)]
fn install_windows(app: &AppHandle) -> Result<(), String> {
    let (source, _) = preferred_payload(app)?;
    let target = install_path(app)?;
    let marker = marker_path(app)?;
    if target.exists() && !marker.exists() {
        return Err(format!(
            "{} already exists and is not managed by Pisper.",
            target.display()
        ));
    }
    fs::create_dir_all(install_dir(app)?).map_err(|error| error.to_string())?;
    let temporary = target.with_extension("exe.tmp");
    fs::copy(&source, &temporary).map_err(|error| {
        format!(
            "Failed to stage the terminal client from {}: {error}",
            source.display()
        )
    })?;
    if target.exists() {
        if let Err(error) = fs::remove_file(&target) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "Failed to update {}. Close running Pisper CLI sessions and retry: {error}",
                target.display()
            ));
        }
    }
    fs::rename(&temporary, &target)
        .map_err(|error| format!("Failed to install {}: {error}", target.display()))?;
    write_file(&marker, expected_marker(app)?.as_bytes())?;
    windows_path::add(&install_dir(app)?)
}

#[cfg(windows)]
mod windows_path {
    use std::{io::ErrorKind, path::Path};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ},
        RegKey, RegValue,
    };

    fn decode(value: &RegValue) -> Result<String, String> {
        if value.bytes.len() % 2 != 0 {
            return Err("The user PATH registry value is not valid UTF-16.".to_string());
        }
        let mut units = value
            .bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        while units.last() == Some(&0) {
            units.pop();
        }
        String::from_utf16(&units).map_err(|error| error.to_string())
    }

    fn encode(value: &str, value_type: winreg::enums::RegType) -> RegValue {
        let mut units = value.encode_utf16().collect::<Vec<_>>();
        units.push(0);
        RegValue {
            bytes: units
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
            vtype: value_type,
        }
    }

    fn environment() -> Result<RegKey, String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
            .map_err(|error| format!("Failed to open the user environment registry key: {error}"))
    }

    fn read_path(key: &RegKey) -> Result<(String, winreg::enums::RegType), String> {
        match key.get_raw_value("Path") {
            Ok(value) => Ok((decode(&value)?, value.vtype)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok((String::new(), REG_EXPAND_SZ)),
            Err(error) => Err(format!("Failed to read the user PATH: {error}")),
        }
    }

    fn normalized(value: &str) -> String {
        value
            .trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }

    fn broadcast() {
        let environment = "Environment\0".encode_utf16().collect::<Vec<_>>();
        let mut result = 0_usize;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2_000,
                &mut result,
            );
        }
    }

    pub fn contains(directory: &Path) -> bool {
        let Ok(key) = environment() else {
            return false;
        };
        let Ok((current, _)) = read_path(&key) else {
            return false;
        };
        let expected = normalized(&directory.to_string_lossy());
        current
            .split(';')
            .any(|entry| normalized(entry) == expected)
    }

    pub fn add(directory: &Path) -> Result<(), String> {
        let key = environment()?;
        let (current, value_type) = read_path(&key)?;
        let expected = normalized(&directory.to_string_lossy());
        if current
            .split(';')
            .any(|entry| normalized(entry) == expected)
        {
            return Ok(());
        }
        let directory = directory.to_string_lossy();
        let updated = if current.is_empty() || current.ends_with(';') {
            format!("{current}{directory}")
        } else {
            format!("{current};{directory}")
        };
        key.set_raw_value("Path", &encode(&updated, value_type))
            .map_err(|error| format!("Failed to update the user PATH: {error}"))?;
        broadcast();
        Ok(())
    }

    pub fn remove(directory: &Path) -> Result<(), String> {
        let key = environment()?;
        let (current, value_type) = read_path(&key)?;
        let expected = normalized(&directory.to_string_lossy());
        if !current
            .split(';')
            .any(|entry| normalized(entry) == expected)
        {
            return Ok(());
        }
        let updated = current
            .split(';')
            .filter(|entry| normalized(entry) != expected)
            .collect::<Vec<_>>()
            .join(";");
        key.set_raw_value("Path", &encode(&updated, value_type))
            .map_err(|error| format!("Failed to update the user PATH: {error}"))?;
        broadcast();
        Ok(())
    }
}

fn has_managed_marker(value: Option<&str>) -> bool {
    value.is_some_and(|contents| contents.contains(MANAGED_MARKER))
}

pub fn refresh_managed_cli(app: &AppHandle) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let marker = read_optional(&marker_path(app)?)?;
        if !has_managed_marker(marker.as_deref()) {
            return Ok(false);
        }
        let needs_refresh = !install_path(app)?.is_file()
            || marker.as_deref() != Some(expected_marker(app)?.as_str())
            || !windows_path::contains(&install_dir(app)?);
        if needs_refresh {
            install_windows(app)?;
        }
        Ok(needs_refresh)
    }

    #[cfg(not(windows))]
    {
        let target = install_path(app)?;
        let contents = read_optional(&target)?;
        if !has_managed_marker(contents.as_deref()) {
            return Ok(false);
        }
        let expected = expected_launcher(app)?;
        let needs_refresh =
            contents.as_deref() != Some(expected.as_str()) || !profile_is_configured(app);
        if needs_refresh {
            install_unix(app)?;
        }
        Ok(needs_refresh)
    }
}

fn current_status(app: &AppHandle) -> Result<CliInstallStatus, String> {
    let target = install_path(app)?;
    let supported = preferred_payload(app).is_ok();

    #[cfg(windows)]
    let (installed, path_configured, launcher_matches) = {
        let marker = read_optional(&marker_path(app)?)?;
        (
            target.is_file() && has_managed_marker(marker.as_deref()),
            windows_path::contains(&install_dir(app)?),
            marker.as_deref() == Some(expected_marker(app)?.as_str()),
        )
    };

    #[cfg(not(windows))]
    let (installed, path_configured, launcher_matches) = {
        let contents = read_optional(&target)?;
        let installed = has_managed_marker(contents.as_deref());
        let expected = expected_launcher(app).ok();
        (
            installed,
            profile_is_configured(app),
            contents.is_some() && contents == expected,
        )
    };

    Ok(CliInstallStatus {
        supported,
        installed,
        path_configured,
        needs_repair: installed && (!path_configured || !launcher_matches),
        command: "pisper",
        install_path: target.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn desktop_get_cli_status(app: AppHandle) -> Result<CliInstallStatus, String> {
    current_status(&app)
}

#[tauri::command]
pub fn desktop_install_cli(app: AppHandle) -> Result<CliInstallStatus, String> {
    #[cfg(windows)]
    install_windows(&app)?;
    #[cfg(not(windows))]
    install_unix(&app)?;
    current_status(&app)
}

#[tauri::command]
pub fn desktop_uninstall_cli(app: AppHandle) -> Result<CliInstallStatus, String> {
    let target = install_path(&app)?;
    let marker = marker_path(&app)?;
    #[cfg(windows)]
    let owned = marker.exists();
    #[cfg(not(windows))]
    let owned = marker.exists()
        || read_optional(&target)?
            .as_deref()
            .is_some_and(|value| value.contains(MANAGED_MARKER));
    if owned {
        match fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to remove {}. Close running Pisper CLI sessions and retry: {error}",
                    target.display()
                ));
            }
        }
        let _ = fs::remove_file(&marker);
    }

    #[cfg(windows)]
    if owned {
        windows_path::remove(&install_dir(&app)?)?;
    }
    #[cfg(not(windows))]
    remove_profile_paths(&app)?;

    #[cfg(windows)]
    let _ = fs::remove_dir(install_dir(&app)?);
    current_status(&app)
}

pub fn run_bundled_cli(app: &tauri::App, args: &[std::ffi::OsString]) -> Result<i32, String> {
    let (payload, _) = preferred_payload(app.handle())?;
    let (sidecar, runtime) = preferred_runtime(app.handle())?;
    let status = Command::new(payload)
        .args(args)
        .env("PISPER_SIDECAR_PATH", sidecar)
        .env("PISPER_APP_ROOT", runtime)
        .status()
        .map_err(|error| format!("Failed to start Pisper CLI: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::{
        has_managed_marker, managed_marker, remove_managed_block, PROFILE_END, PROFILE_START,
    };

    #[test]
    fn managed_markers_identify_the_exact_bundled_payload() {
        assert_eq!(
            managed_marker("0.4.12", 10_124_257),
            "PISPER_CLI_MANAGED_V1\nversion=0.4.12\npayload_size=10124257\n"
        );
        assert!(has_managed_marker(Some(
            "PISPER_CLI_MANAGED_V1\nversion=0.4.12\n"
        )));
        assert!(!has_managed_marker(Some("version=0.4.12\n")));
        assert!(!has_managed_marker(None));
    }

    #[test]
    fn removes_only_the_managed_profile_block() {
        let source = format!(
            "export EDITOR=vim\n{PROFILE_START}\nexport PATH=managed\n{PROFILE_END}\nexport FOO=bar\n"
        );
        assert_eq!(
            remove_managed_block(&source).unwrap(),
            "export EDITOR=vim\nexport FOO=bar\n"
        );
    }

    #[test]
    fn rejects_incomplete_profile_blocks() {
        assert!(remove_managed_block(PROFILE_START).is_err());
        assert!(remove_managed_block(PROFILE_END).is_err());
    }
}
