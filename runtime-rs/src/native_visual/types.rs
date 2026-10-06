use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{fmt, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type Result<T> = std::result::Result<T, VisualError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualError {
    pub message: String,
    pub status: Option<u16>,
    pub cancelled: bool,
}
impl VisualError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            cancelled: false,
        }
    }
    pub fn with_status(message: impl Into<String>, status: u16) -> Self {
        Self {
            message: message.into(),
            status: Some(status),
            cancelled: false,
        }
    }
    pub fn cancelled() -> Self {
        Self {
            message: "This operation was aborted".into(),
            status: None,
            cancelled: true,
        }
    }
}
impl fmt::Display for VisualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for VisualError {}
impl From<std::io::Error> for VisualError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualKind {
    Image,
    Video,
}
impl VisualKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VisualOperation {
    Generate,
    Edit,
}

/// Private input to the catalog. This is never a response/debug payload:
/// auth_json and runtime headers can contain real credentials.
#[derive(Clone)]
pub struct VisualConfigSnapshot {
    pub models_json: Value,
    pub auth_json: Value,
    pub app_json: Value,
    pub runtime_provider_names: Map<String, Value>,
    pub runtime_models: Vec<Value>,
    pub locale: String,
}
impl Default for VisualConfigSnapshot {
    fn default() -> Self {
        Self {
            models_json: serde_json::json!({}),
            auth_json: serde_json::json!({}),
            app_json: serde_json::json!({}),
            runtime_provider_names: Map::new(),
            runtime_models: Vec::new(),
            locale: "zh-CN".into(),
        }
    }
}
pub type VisualConfigRead =
    Arc<dyn Fn() -> BoxFuture<'static, Result<VisualConfigSnapshot>> + Send + Sync>;
pub type VisualPreferenceWrite =
    Arc<dyn Fn(VisualKind, Option<String>) -> BoxFuture<'static, Result<()>> + Send + Sync>;
#[derive(Clone)]
pub struct VisualConfigPort {
    pub read: VisualConfigRead,
    /// Root serializes an atomic mutation of pisper.json.visualDefaultModels
    /// under the canonical writer, preserving all unrelated fields.
    pub write_preference: VisualPreferenceWrite,
}

#[derive(Clone)]
pub(super) struct VisualModel {
    pub(super) public: Value,
    pub(super) key: Value,
    pub(super) headers: Map<String, Value>,
    pub(super) visual: bool,
    pub(super) configured: bool,
    pub(super) score: i64,
}
impl VisualModel {
    pub(super) fn string(&self, name: &str) -> &str {
        self.public[name].as_str().unwrap_or("")
    }
    pub(super) fn reference(&self) -> String {
        format!("{}/{}", self.string("providerId"), self.string("id"))
    }
}

#[derive(Clone)]
pub(super) struct SourceImage {
    pub(super) path: PathBuf,
    pub(super) mime_type: String,
    pub(super) bytes: Vec<u8>,
}
pub(super) struct DriverResult {
    pub(super) bytes: Vec<u8>,
    pub(super) mime_type: String,
    pub(super) extension: String,
    pub(super) remote_id: Option<String>,
}
pub(super) struct PreparedRequest {
    pub(super) input: Value,
    pub(super) kind: VisualKind,
    pub(super) operation: VisualOperation,
    pub(super) sources: Vec<SourceImage>,
    pub(super) mask: Option<SourceImage>,
}

#[derive(Clone)]
pub struct VisualRequest {
    /// Actual authorized cwd, supplied by the host; an input.cwd key is ignored.
    pub cwd: PathBuf,
    pub input: Value,
}
pub type VisualProgress = Arc<dyn Fn(String) + Send + Sync>;
#[derive(Clone)]
pub struct VisualOptions {
    pub cancellation: CancellationToken,
    pub on_progress: Option<VisualProgress>,
    /// false is mandatory for already-paid workflow/game/image-assets work.
    pub allow_fallback: bool,
}
impl Default for VisualOptions {
    fn default() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            on_progress: None,
            allow_fallback: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualResult {
    pub path: PathBuf,
    pub kind: VisualKind,
    pub mime_type: String,
    pub size: u64,
    pub provider: String,
    pub provider_name: String,
    pub model: String,
    pub model_name: String,
    pub remote_id: Option<String>,
    pub operation: VisualOperation,
    pub fallback_used: bool,
    pub attempted_models: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct VisualToolContext {
    pub cwd: PathBuf,
    pub session_id: String,
}
pub type VisualContextPort =
    Arc<dyn Fn(String, PathBuf) -> BoxFuture<'static, Result<VisualToolContext>> + Send + Sync>;
pub type VisualGeneratedFilePort =
    Arc<dyn Fn(VisualToolContext, VisualResult) -> BoxFuture<'static, Result<()>> + Send + Sync>;
