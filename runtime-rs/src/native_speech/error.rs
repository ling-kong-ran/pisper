use axum::http::StatusCode;
use std::fmt;

#[derive(Clone, Debug)]
pub struct SpeechError {
    pub code: &'static str,
    pub message: &'static str,
    pub status: StatusCode,
}
pub type Result<T> = std::result::Result<T, SpeechError>;
pub fn download(code: &'static str) -> SpeechError {
    let message = match code {
        "catalog" => "Invalid speech model catalog.",
        "unknown" => "Unknown speech model.",
        "path" => "Unsafe speech model storage path.",
        "integrity" => "Speech model integrity verification failed.",
        "size" => "Speech model download size does not match the manifest.",
        "range" => "Invalid speech model partial response.",
        "http" => "Speech model CDN request failed.",
        "redirect" => "Speech model CDN redirect is not allowed.",
        "timeout" => "Speech model download timed out.",
        "cancelled" => "Speech model download cancelled.",
        "disposed" => "Speech model download service is disposed.",
        "missing" => "Speech model is not installed or failed verification.",
        _ => "Speech model installation failed.",
    };
    SpeechError {
        code,
        message,
        status: if code == "unknown" {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        },
    }
}
pub fn engine(code: &'static str) -> SpeechError {
    let message = match code {
        "invalid" => "Invalid speech input.",
        "busy" => "Speech engine is busy. Please try again after the current operation.",
        "missing" => "Speech model is not installed or failed verification.",
        "session" => "Speech recognition session does not exist or has ended.",
        "limit" => "Speech input or output exceeds the supported limit.",
        "cancelled" => "Speech synthesis was cancelled.",
        "disposed" => "Speech engine is disposed.",
        "timeout" => "Speech inference timed out.",
        "worker" => "Speech inference process stopped unexpectedly.",
        "config" => "Speech model configuration is invalid.",
        _ => "Speech inference failed.",
    };
    SpeechError {
        code,
        message,
        status: match code {
            "busy" => StatusCode::CONFLICT,
            "session" => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        },
    }
}
impl fmt::Display for SpeechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for SpeechError {}
impl From<SpeechError> for crate::ApiError {
    fn from(error: SpeechError) -> Self {
        Self::new(error.status, error.code, error.message)
    }
}
