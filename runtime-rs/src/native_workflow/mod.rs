pub(crate) mod bundle;
pub(crate) mod engine_cache;
pub(crate) mod image_nodes;
pub(crate) mod image_processing;
pub(crate) mod image_protocol;
pub(crate) mod inputs;
pub(crate) mod media;
pub(crate) mod model;
#[cfg(test)]
pub(crate) mod test_support;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, Clone)]
pub(crate) struct WorkflowError {
    pub(crate) status: StatusCode,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) partial_output: Option<serde_json::Value>,
}

impl WorkflowError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::coded("workflow_invalid", message)
    }
    pub(crate) fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            message: message.into(),
            partial_output: None,
        }
    }
    pub(crate) fn busy(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "workflow_busy".into(),
            message: message.into(),
            partial_output: None,
        }
    }
    pub(crate) fn io(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "workflow_storage_error".into(),
            message: error.to_string(),
            partial_output: None,
        }
    }
    pub(crate) fn cancelled() -> Self {
        Self::coded("WORKFLOW_CANCELLED", "工作流已停止。")
    }
    pub(crate) fn with_partial_output(mut self, output: serde_json::Value) -> Self {
        self.partial_output = Some(output);
        self
    }
}
impl std::fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for WorkflowError {}
impl IntoResponse for WorkflowError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error":self.message,"code":self.code})),
        )
            .into_response()
    }
}
pub(crate) type Result<T> = std::result::Result<T, WorkflowError>;

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub(crate) fn id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(WorkflowError::io)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

pub(crate) fn save_json(path: &std::path::Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(WorkflowError::io)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", id()?));
    let bytes = serde_json::to_vec_pretty(value).map_err(WorkflowError::io)?;
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(WorkflowError::io)?;
        file.write_all(&bytes).map_err(WorkflowError::io)?;
        file.sync_all().map_err(WorkflowError::io)?;
        drop(file);
        // 同目录 rename 在 Windows 也替换原文件，不能先移走原文件形成掉电窗口。
        std::fs::rename(&temporary, path).map_err(WorkflowError::io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

pub(crate) fn read_json(
    path: &std::path::Path,
    fallback: serde_json::Value,
) -> Result<serde_json::Value> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(WorkflowError::io),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback),
        Err(error) => Err(WorkflowError::io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_state_never_removes_existing_file() {
        let directory = test_support::TempDirectory::new();
        let path = directory.path.join("state.json");
        save_json(&path, &json!({"revision":0})).unwrap();
        let reader_path = path.clone();
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let reader_running = running.clone();
        let reader = std::thread::spawn(move || {
            while reader_running.load(std::sync::atomic::Ordering::Acquire) {
                let bytes = std::fs::read(&reader_path).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert!(value["revision"].is_u64());
            }
        });
        for revision in 1..100 {
            save_json(&path, &json!({"revision":revision})).unwrap();
        }
        running.store(false, std::sync::atomic::Ordering::Release);
        reader.join().unwrap();
        assert_eq!(read_json(&path, json!({})).unwrap()["revision"], 99);
    }
}
