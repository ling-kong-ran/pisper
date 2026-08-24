use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRuntimeStatus {
    pub supported: bool,
    pub packaged: bool,
    pub installed: bool,
    pub running: bool,
    pub state: String,
    pub message: String,
    pub url: String,
    pub runtime_kind: String,
}

impl RootRuntimeStatus {
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            supported: false,
            packaged: false,
            installed: false,
            running: false,
            state: "unsupported".into(),
            message: message.into(),
            url: String::new(),
            runtime_kind: "node-full".into(),
        }
    }
}
