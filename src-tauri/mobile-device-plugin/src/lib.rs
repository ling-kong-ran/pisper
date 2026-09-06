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

    pub fn import_workspace_directory(&self, destination_root: String) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ImportWorkspaceRequest {
            destination_root: String,
        }
        self.0
            .run_mobile_plugin(
                "importWorkspaceDirectory",
                ImportWorkspaceRequest { destination_root },
            )
            .map_err(Into::into)
    }

    pub fn transcribe_pcm(&self, pcm_base64: impl Into<String>, hotwords: String) -> Result<Value> {
        self.transcribe_pcm_with_options(pcm_base64, hotwords, None, None)
    }

    pub fn transcribe_pcm_with_options(
        &self,
        pcm_base64: impl Into<String>,
        hotwords: String,
        model_id: Option<String>,
        request_id: Option<String>,
    ) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct TranscribeRequest {
            pcm_base64: String,
            hotwords: String,
            model_id: Option<String>,
            request_id: Option<String>,
        }
        self.0
            .run_mobile_plugin(
                "transcribePcm",
                TranscribeRequest {
                    pcm_base64: pcm_base64.into(),
                    hotwords,
                    model_id,
                    request_id,
                },
            )
            .map_err(Into::into)
    }

    pub fn speech_models(&self) -> Result<Value> {
        self.0
            .run_mobile_plugin("speechModels", ())
            .map_err(Into::into)
    }

    fn speech_model_operation(&self, command: &str, model_id: String) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ModelRequest {
            model_id: String,
        }
        self.0
            .run_mobile_plugin(command, ModelRequest { model_id })
            .map_err(Into::into)
    }

    pub fn download_speech_model(&self, model_id: String) -> Result<Value> {
        self.speech_model_operation("downloadSpeechModel", model_id)
    }

    pub fn cancel_speech_model_download(&self, model_id: String) -> Result<Value> {
        self.speech_model_operation("cancelSpeechModelDownload", model_id)
    }

    pub fn synthesize_speech(
        &self,
        text: String,
        voice_id: String,
        request_id: String,
    ) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SynthesisRequest {
            text: String,
            voice_id: String,
            request_id: String,
        }
        self.0
            .run_mobile_plugin(
                "synthesizeSpeech",
                SynthesisRequest {
                    text,
                    voice_id,
                    request_id,
                },
            )
            .map_err(Into::into)
    }

    pub fn play_speech(&self, audio_id: String, request_id: String) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct PlaybackRequest {
            audio_id: String,
            request_id: String,
        }
        self.0
            .run_mobile_plugin(
                "playSpeech",
                PlaybackRequest {
                    audio_id,
                    request_id,
                },
            )
            .map_err(Into::into)
    }

    pub fn cancel_speech(&self, request_id: String) -> Result<Value> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CancelRequest {
            request_id: String,
        }
        self.0
            .run_mobile_plugin("cancelSpeech", CancelRequest { request_id })
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
