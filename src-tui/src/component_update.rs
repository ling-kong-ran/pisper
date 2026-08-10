use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
};

#[cfg(windows)]
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use pisper_component_updater::{Component, ComponentUpdater, InstalledComponent, ReleaseInfo};
use semver::Version;

use crate::sidecar::{bundled_runtime_version, components_root};

const UPDATER_PUBLIC_KEY: &str = include_str!("../../src-tauri/updater.pubkey");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateSelection {
    Tui,
    Runtime,
    Web,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateRequest {
    pub selection: UpdateSelection,
    pub check_only: bool,
}

pub fn parse_update_request(arguments: &[OsString]) -> Result<Option<UpdateRequest>> {
    if arguments.first().and_then(|value| value.to_str()) != Some("update") {
        return Ok(None);
    }
    let mut selection = UpdateSelection::All;
    let mut check_only = false;
    for argument in &arguments[1..] {
        match argument.to_str() {
            Some("tui") => selection = UpdateSelection::Tui,
            Some("runtime") => selection = UpdateSelection::Runtime,
            Some("web") | Some("desktop") => selection = UpdateSelection::Web,
            Some("all") => selection = UpdateSelection::All,
            Some("--check") => check_only = true,
            Some(value) => bail!("unknown update argument: {value}"),
            None => bail!("update arguments must be valid UTF-8"),
        }
    }
    Ok(Some(UpdateRequest {
        selection,
        check_only,
    }))
}

pub async fn execute(request: UpdateRequest) -> Result<()> {
    let updater = updater()?;
    let components = match request.selection {
        UpdateSelection::Tui => vec![Component::Tui],
        UpdateSelection::Runtime => vec![Component::Runtime],
        UpdateSelection::Web => vec![Component::Desktop],
        UpdateSelection::All => vec![Component::Runtime, Component::Tui],
    };
    for component in components {
        update_component(&updater, component, request.check_only).await?;
    }
    Ok(())
}

pub async fn offer_startup_updates() {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return;
    }
    if let Err(error) = try_offer_startup_updates().await {
        // Offline or sandboxed machines cannot reach the release feed; that
        // must never be visible or fatal. Enable PISPER_UPDATE_CHECK_LOG to
        // surface the underlying error for diagnostics.
        if std::env::var_os("PISPER_UPDATE_CHECK_LOG").is_some() {
            eprintln!(
                "Pisper update check failed; continuing with the installed version: {error:#}"
            );
        }
    }
}

struct AvailableUpdate {
    current: Option<Version>,
    release: ReleaseInfo,
}

async fn try_offer_startup_updates() -> Result<()> {
    let updater = updater()?;
    let (runtime, tui) = tokio::join!(
        available_update(&updater, Component::Runtime),
        available_update(&updater, Component::Tui),
    );
    // Tolerate per-component check failures (e.g. one channel unreachable) so a
    // single offline failure never aborts the whole offer.
    let updates = [runtime, tui]
        .into_iter()
        .filter_map(Result::ok)
        .flatten()
        .collect::<Vec<_>>();
    if updates.is_empty() {
        return Ok(());
    }

    println!("Pisper updates are available:");
    for update in &updates {
        println!(
            "  {:<7} {} -> {} ({:.1} MB)",
            component_label(update.release.component),
            update
                .current
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "not installed".to_owned()),
            update.release.version,
            update.release.size as f64 / 1024.0 / 1024.0,
        );
    }
    print!("Install now? [Y/n] ");
    io::stdout()
        .flush()
        .context("failed to display the update confirmation")?;
    let mut response = String::new();
    if io::stdin()
        .lock()
        .read_line(&mut response)
        .context("failed to read the update confirmation")?
        == 0
        || !startup_confirmation(&response)
    {
        println!("Continuing without updating.");
        return Ok(());
    }

    for update in &updates {
        install_component_update(&updater, &update.release).await?;
    }
    Ok(())
}

async fn available_update(
    updater: &ComponentUpdater,
    component: Component,
) -> Result<Option<AvailableUpdate>> {
    let current = current_version(updater, component);
    let release = updater
        .latest(component)
        .await
        .with_context(|| format!("failed to check {} updates", component.name()))?;
    let latest = Version::parse(&release.version).context("release version is invalid")?;
    if current.as_ref().is_some_and(|version| version >= &latest) {
        return Ok(None);
    }
    Ok(Some(AvailableUpdate { current, release }))
}

fn component_label(component: Component) -> &'static str {
    match component {
        Component::Desktop => "Web",
        Component::Tui => "TUI",
        Component::Runtime => "Runtime",
    }
}

fn startup_confirmation(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
}

pub async fn ensure_web() -> Result<InstalledComponent> {
    let updater = updater()?;
    // If the Web frontend is already installed, keep using it when the release
    // feed is unreachable (offline machines) instead of failing the launch.
    if let Some(installed) = updater.installed(Component::Desktop)? {
        match updater.latest(Component::Desktop).await {
            Ok(release) => {
                let latest =
                    Version::parse(&release.version).context("Web release version is invalid")?;
                if installed.version >= latest {
                    println!("Pisper Web {} is already installed.", installed.version);
                    return Ok(installed);
                }
            }
            Err(_) => {
                println!(
                    "Pisper Web {} is already installed (update check unavailable).",
                    installed.version
                );
                return Ok(installed);
            }
        }
    }
    let release = updater
        .latest(Component::Desktop)
        .await
        .context("failed to locate a signed Pisper Web release")?;
    println!(
        "Downloading Pisper Web {} ({:.1} MB)...",
        release.version,
        release.size as f64 / 1024.0 / 1024.0
    );
    let installed = updater
        .install(&release)
        .await
        .context("failed to install the signed Pisper Web component")?;
    println!("Installed Pisper Web {}.", installed.version);
    Ok(installed)
}

pub async fn ensure_runtime() -> Result<InstalledComponent> {
    let updater = updater()?;
    let release = updater
        .latest(Component::Runtime)
        .await
        .context("failed to locate a Pisper runtime release")?;
    println!(
        "Pisper runtime is not installed; downloading runtime {} ({:.1} MB)...",
        release.version,
        release.size as f64 / 1024.0 / 1024.0
    );
    let installed = updater
        .install(&release)
        .await
        .context("failed to install the Pisper runtime component")?;
    println!("Installed Pisper runtime {}.", installed.version);
    Ok(installed)
}

fn updater() -> Result<ComponentUpdater> {
    ComponentUpdater::new(
        components_root()?,
        UPDATER_PUBLIC_KEY,
        &format!("Pisper-TUI/{}", env!("CARGO_PKG_VERSION")),
    )
}

async fn update_component(
    updater: &ComponentUpdater,
    component: Component,
    check_only: bool,
) -> Result<()> {
    let current = current_version(updater, component);
    let release = updater
        .latest(component)
        .await
        .with_context(|| format!("failed to check {} updates", component.name()))?;
    let latest = Version::parse(&release.version).context("release version is invalid")?;
    if current.as_ref().is_some_and(|version| version >= &latest) {
        println!(
            "Pisper {} {} is current.",
            component.name(),
            current.unwrap()
        );
        return Ok(());
    }
    println!(
        "Pisper {} update: {} -> {} ({:.1} MB)",
        component.name(),
        current
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "not installed".to_string()),
        latest,
        release.size as f64 / 1024.0 / 1024.0
    );
    if check_only {
        return Ok(());
    }
    install_component_update(updater, &release).await
}

async fn install_component_update(updater: &ComponentUpdater, release: &ReleaseInfo) -> Result<()> {
    let component = release.component;
    let installed = updater
        .install(release)
        .await
        .with_context(|| format!("failed to install {} update", component.name()))?;
    if component == Component::Tui {
        match replace_current_tui(&installed) {
            #[cfg(windows)]
            Ok(TuiReplaceOutcome::Scheduled) => {
                println!("The TUI update will finish after this process exits.")
            }
            #[cfg(not(windows))]
            Ok(TuiReplaceOutcome::Applied) => {
                println!("Updated the current TUI executable to {}.", installed.version)
            }
            Ok(TuiReplaceOutcome::Stored) => println!(
                "Installed TUI {} in the component store; the desktop launcher will use it next time.",
                installed.version
            ),
            Err(error) => println!(
                "Installed TUI {} in the component store, but the current executable could not be replaced: {error:#}",
                installed.version
            ),
        }
    } else if component == Component::Desktop {
        println!(
            "Installed Web UI {}. It will be served by the next Pisper process.",
            installed.version
        );
    } else {
        println!(
            "Installed runtime {}. It will be used by the next Pisper process.",
            installed.version
        );
    }
    Ok(())
}

fn current_version(updater: &ComponentUpdater, component: Component) -> Option<Version> {
    if let Ok(Some(installed)) = updater.installed(component) {
        return Some(installed.version);
    }
    let value = match component {
        Component::Desktop => updater
            .installed(Component::Desktop)
            .ok()
            .flatten()
            .map(|installed| installed.version.to_string()),
        Component::Tui => Some(env!("CARGO_PKG_VERSION").to_string()),
        Component::Runtime => bundled_runtime_version(),
    }?;
    Version::parse(&value).ok()
}

enum TuiReplaceOutcome {
    #[cfg(windows)]
    Scheduled,
    #[cfg(not(windows))]
    Applied,
    Stored,
}

fn replace_current_tui(installed: &InstalledComponent) -> Result<TuiReplaceOutcome> {
    let source = installed.executable();
    let current = std::env::current_exe().context("failed to locate the running TUI")?;
    if same_file(&source, &current) {
        return Ok(TuiReplaceOutcome::Stored);
    }

    #[cfg(windows)]
    {
        if current.file_name().and_then(|value| value.to_str()) != Some("pisper.exe") {
            return Ok(TuiReplaceOutcome::Stored);
        }
        let staged = current.with_extension("update.exe");
        fs::copy(&source, &staged).context("failed to stage the TUI update")?;
        schedule_windows_replace(&staged, &current)?;
        Ok(TuiReplaceOutcome::Scheduled)
    }

    #[cfg(not(windows))]
    {
        let staged = current.with_extension("update");
        fs::copy(&source, &staged).context("failed to stage the TUI update")?;
        fs::set_permissions(&staged, fs::metadata(&source)?.permissions())?;
        fs::rename(&staged, &current).context("failed to activate the TUI update")?;
        Ok(TuiReplaceOutcome::Applied)
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    if cfg!(windows) {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    } else {
        left == right
    }
}

#[cfg(windows)]
fn schedule_windows_replace(staged: &Path, current: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let quote = |path: &Path| path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$process = Get-Process -Id {} -ErrorAction SilentlyContinue; if ($process) {{ $process.WaitForExit() }}; Move-Item -LiteralPath '{}' -Destination '{}' -Force",
        std::process::id(),
        quote(staged),
        quote(current)
    );
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .context("failed to schedule the Windows TUI replacement")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_update_request, startup_confirmation, UpdateRequest, UpdateSelection};
    use std::ffi::OsString;

    #[test]
    fn startup_update_confirmation_defaults_to_yes() {
        assert!(startup_confirmation(""));
        assert!(startup_confirmation("y\n"));
        assert!(startup_confirmation("YES"));
        assert!(!startup_confirmation("n\n"));
        assert!(!startup_confirmation("later"));
    }

    #[test]
    fn update_command_selects_components_and_check_mode() {
        assert_eq!(
            parse_update_request(&[OsString::from("update")]).unwrap(),
            Some(UpdateRequest {
                selection: UpdateSelection::All,
                check_only: false,
            })
        );
        assert_eq!(
            parse_update_request(&[
                OsString::from("update"),
                OsString::from("runtime"),
                OsString::from("--check"),
            ])
            .unwrap(),
            Some(UpdateRequest {
                selection: UpdateSelection::Runtime,
                check_only: true,
            })
        );
        assert_eq!(
            parse_update_request(&[OsString::from("update"), OsString::from("web")]).unwrap(),
            Some(UpdateRequest {
                selection: UpdateSelection::Web,
                check_only: false,
            })
        );
        assert!(parse_update_request(&[OsString::from("chat")])
            .unwrap()
            .is_none());
    }
}
