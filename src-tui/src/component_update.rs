use anyhow::{Context, Result};
use pisper_component_updater::{Component, ComponentUpdater, InstalledComponent};
use semver::Version;

use crate::sidecar::components_root;

const UPDATER_PUBLIC_KEY: &str = include_str!("../../src-tauri/updater.pubkey");

pub async fn ensure_web() -> Result<InstalledComponent> {
    let updater = updater()?;
    // Standalone TUI installations retain their existing signed Web component
    // when the release feed is temporarily unavailable.
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
