use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    ipc::Channel,
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_dns_sd);

// initializes the Kotlin or Swift plugin classes
pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<DnsSd<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("com.momics.dnssd", "DnsSdPlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_dns_sd)?;
    Ok(DnsSd(handle))
}

/// Access to the DNS-SD APIs on mobile (delegates to the native OS resolver).
pub struct DnsSd<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> DnsSd<R> {
    // Mobile browse start: delegates to native layer with channel for events.
    pub async fn browse_start(
        &self,
        _app: AppHandle<R>,
        options: BrowseOptions,
        channel: Channel,
    ) -> crate::Result<BrowseHandle> {
        #[derive(Serialize)]
        struct BrowsePayload {
            #[serde(flatten)]
            options: BrowseOptions,
            channel: Channel,
        }

        self.0
            .run_mobile_plugin("browse_start", BrowsePayload { options, channel })
            .map_err(Into::into)
    }

    pub async fn browse_stop(&self, _app: AppHandle<R>, browse_id: u64) -> crate::Result<()> {
        #[derive(serde::Serialize)]
        struct StopPayload {
            #[serde(rename = "browseId")]
            browse_id: u64,
        }
        self.0
            .run_mobile_plugin("browse_stop", StopPayload { browse_id })
            .map_err(Into::into)
    }

    pub async fn advertise_start(
        &self,
        _app: AppHandle<R>,
        options: AdvertiseOptions,
    ) -> crate::Result<AdvertiseHandle> {
        self.0
            .run_mobile_plugin("advertise_start", options)
            .map_err(Into::into)
    }

    pub async fn advertise_stop(&self, _app: AppHandle<R>, advertise_id: u64) -> crate::Result<()> {
        #[derive(serde::Serialize)]
        struct StopPayload {
            #[serde(rename = "advertiseId")]
            advertise_id: u64,
        }
        self.0
            .run_mobile_plugin("advertise_stop", StopPayload { advertise_id })
            .map_err(Into::into)
    }
}
