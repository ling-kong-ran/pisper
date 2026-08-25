use crate::models::*;
use crate::DnsSd;
use tauri::ipc::Channel;
use tauri::{command, AppHandle, Runtime, State};

#[cfg(desktop)]
use crate::desktop::commands::BrowseChannelMessage;

#[cfg(desktop)]
#[command]
pub(crate) async fn browse_start<R: Runtime>(
    app: AppHandle<R>,
    mdns: State<'_, DnsSd<R>>,
    options: BrowseOptions,
    channel: Channel<BrowseChannelMessage>,
) -> crate::Result<BrowseHandle> {
    mdns.inner().browse_start(app, options, channel).await
}

#[cfg(mobile)]
#[command]
pub(crate) async fn browse_start<R: Runtime>(
    app: AppHandle<R>,
    mdns: State<'_, DnsSd<R>>,
    options: BrowseOptions,
    channel: Channel,
) -> crate::Result<BrowseHandle> {
    mdns.inner().browse_start(app, options, channel).await
}

#[command]
pub(crate) async fn browse_stop<R: Runtime>(
    app: AppHandle<R>,
    mdns: State<'_, DnsSd<R>>,
    browse_id: u64,
) -> crate::Result<()> {
    mdns.inner().browse_stop(app, browse_id).await
}

#[command]
pub(crate) async fn advertise_start<R: Runtime>(
    app: AppHandle<R>,
    mdns: State<'_, DnsSd<R>>,
    options: AdvertiseOptions,
) -> crate::Result<AdvertiseHandle> {
    mdns.inner().advertise_start(app, options).await
}

#[command]
pub(crate) async fn advertise_stop<R: Runtime>(
    app: AppHandle<R>,
    mdns: State<'_, DnsSd<R>>,
    advertise_id: u64,
) -> crate::Result<()> {
    mdns.inner().advertise_stop(app, advertise_id).await
}
