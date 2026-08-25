//! A Tauri v2 plugin exposing DNS-SD (mDNS / Bonjour / Zeroconf) service
//! discovery and advertisement. On desktop it drives the `mdns-sd` crate; on
//! iOS and Android it delegates to the OS resolver (Bonjour `NWBrowser` /
//! `NWListener` and Android `NsdManager`). The guest-js binding maps these
//! into the shared `@momics/dns-sd-shared` `DnsSdAdapter` seam so the public
//! `browse` / `advertise` API is identical across every runtime.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;

pub use error::{Error, Result};

#[cfg(desktop)]
pub use desktop::DnsSd;
#[cfg(mobile)]
pub use mobile::DnsSd;

/// Extensions to [`tauri::App`] / [`tauri::AppHandle`] to access the DNS-SD APIs.
pub trait DnsSdExt<R: Runtime> {
    fn dns_sd(&self) -> &DnsSd<R>;
}

impl<R: Runtime, T: Manager<R>> DnsSdExt<R> for T {
    fn dns_sd(&self) -> &DnsSd<R> {
        self.state::<DnsSd<R>>().inner()
    }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("dns-sd")
        .invoke_handler(tauri::generate_handler![
            commands::browse_start,
            commands::browse_stop,
            commands::advertise_start,
            commands::advertise_stop,
        ])
        .setup(|app, api| {
            #[cfg(mobile)]
            let dns_sd = mobile::init(app, api)?;
            #[cfg(desktop)]
            let dns_sd = desktop::init(app, api)?;
            app.manage(dns_sd);
            Ok(())
        })
        .build()
}
