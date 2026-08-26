#![cfg(mobile)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "app.pisper.mobiledevice";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_mobile_device);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub capability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRequest {
    pub operation: String,
    #[serde(default)]
    pub parameters: Map<String, Value>,
}

pub struct MobileDevice<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> MobileDevice<R> {
    pub fn permission_states(&self) -> Result<Value> {
        self.0
            .run_mobile_plugin("permissionStates", ())
            .map_err(Into::into)
    }

    pub fn request_permission(&self, capability: impl Into<String>) -> Result<Value> {
        self.0
            .run_mobile_plugin(
                "requestPermission",
                PermissionRequest {
                    capability: capability.into(),
                },
            )
            .map_err(Into::into)
    }

    pub fn open_app_settings(&self) -> Result<()> {
        self.0
            .run_mobile_plugin::<()>("openAppSettings", ())
            .map(|_| ())
            .map_err(Into::into)
    }

    pub fn execute(&self, request: OperationRequest) -> Result<Value> {
        self.0
            .run_mobile_plugin("execute", request)
            .map_err(Into::into)
    }
}

pub trait MobileDeviceExt<R: Runtime> {
    fn mobile_device(&self) -> &MobileDevice<R>;
}

impl<R: Runtime, T: Manager<R>> MobileDeviceExt<R> for T {
    fn mobile_device(&self) -> &MobileDevice<R> {
        self.state::<MobileDevice<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("mobile-device")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "MobileDevicePlugin")?;
            #[cfg(target_os = "ios")]
            let handle = api.register_ios_plugin(init_plugin_mobile_device)?;
            app.manage(MobileDevice(handle));
            Ok(())
        })
        .build()
}
