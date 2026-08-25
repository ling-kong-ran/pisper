use serde::de::DeserializeOwned;
use std::sync::Arc;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

// Re-export the commands module (contains desktop implementation)
pub mod commands;

// Import the message type for type annotation
use commands::BrowseChannelMessage;

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> crate::Result<DnsSd<R>> {
    Ok(DnsSd {
        _phantom: std::marker::PhantomData,
        state: Arc::new(commands::MdnsState::default()),
    })
}

/// Access to the DNS-SD APIs on desktop.
/// Wraps [`commands::MdnsState`] and provides the same interface as mobile.
pub struct DnsSd<R: Runtime> {
    _phantom: std::marker::PhantomData<fn() -> R>,
    state: Arc<commands::MdnsState>,
}

impl<R: Runtime> DnsSd<R> {
    pub async fn browse_start(
        &self,
        app: AppHandle<R>,
        options: crate::models::BrowseOptions,
        channel: tauri::ipc::Channel<BrowseChannelMessage>,
    ) -> crate::Result<crate::models::BrowseHandle> {
        // Call desktop implementation directly (without #[command] macro)
        commands::browse_start_impl(app, self.state.clone(), options, channel)
            .await
            .map_err(crate::Error::Custom)
    }

    pub async fn browse_stop(&self, app: AppHandle<R>, browse_id: u64) -> crate::Result<()> {
        commands::browse_stop_impl(app, self.state.clone(), browse_id)
            .await
            .map_err(crate::Error::Custom)
    }

    pub async fn advertise_start(
        &self,
        app: AppHandle<R>,
        options: crate::models::AdvertiseOptions,
    ) -> crate::Result<crate::models::AdvertiseHandle> {
        commands::advertise_start_impl(app, self.state.clone(), options)
            .await
            .map_err(crate::Error::Custom)
    }

    pub async fn advertise_stop(&self, app: AppHandle<R>, advertise_id: u64) -> crate::Result<()> {
        commands::advertise_stop_impl(app, self.state.clone(), advertise_id)
            .await
            .map_err(crate::Error::Custom)
    }
}
