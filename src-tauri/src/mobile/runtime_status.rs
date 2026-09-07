use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnDeviceRuntimeStatus {
    pub supported: bool,
    pub packaged: bool,
    pub installed: bool,
    pub running: bool,
    pub state: String,
    pub message: String,
    pub url: String,
    pub runtime_kind: String,
}
