//! 组件安装/更新（独立 TUI 发行版需要自备 Web 前端与 Runtime 两个签名组件）。
//!
//! 独立 TUI 安装不包含前端和运行时，首次启动前必须从发布源下载对应组件；
//! 桌面版则自带 sidecar-runtime 与 Web 资源，不需要走这里。

use anyhow::{Context, Result};
use pisper_component_updater::{Component, ComponentUpdater, InstalledComponent};
use semver::Version;

use crate::sidecar::components_root;

// 组件包的公开签名密钥，随二进制内嵌，用于校验下载内容的完整性与来源。
const UPDATER_PUBLIC_KEY: &str = include_str!("../../src-tauri/updater.pubkey");

/// 确保 Web 前端组件可用：已安装且版本不落后时直接复用；
/// 发布源暂时不可达时保留已安装版本（比强制失败或覆盖旧版本更稳妥）。
pub async fn ensure_web() -> Result<InstalledComponent> {
    let updater = updater()?;
    // 独立 TUI 安装时，发布源暂时不可达的情况下保留已有签名 Web 组件，
    // 避免一次网络故障就破坏本可正常工作的安装。
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

/// 确保 Runtime 组件可用。Runtime 是 sidecar 进程本体，缺失时无法启动任何会话，
/// 因此这里不做「离线保留旧版」的降级，直接要求最新的签名发布。
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

/// 构造组件更新器：固定组件根目录与签名公钥，
/// 并用 `Pisper-TUI/<版本>` 标识本 TUI 发出的更新请求（便于发布端日志归因）。
fn updater() -> Result<ComponentUpdater> {
    ComponentUpdater::new(
        components_root()?,
        UPDATER_PUBLIC_KEY,
        &format!("Pisper-TUI/{}", env!("CARGO_PKG_VERSION")),
    )
}
