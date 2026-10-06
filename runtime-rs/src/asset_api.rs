//! Release 的资产上传、预览和流式范围下载边界。
#[path = "native_assets/attachments.rs"]
pub mod attachments;
#[path = "native_assets/projection.rs"]
pub mod projection;
#[path = "native_assets/store.rs"]
pub mod store;
#[path = "native_assets/tracker.rs"]
pub mod tracker;
use crate::{ApiError, AppState};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/assets", get(list).post(create))
        .route("/api/assets/{id}/content", get(content))
        .route("/api/assets/{id}/download", get(download))
        .route("/api/assets/{id}", axum::routing::delete(delete))
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    #[serde(default)]
    query: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    session_id: String,
    preview: Option<String>,
    inline: Option<String>,
}
fn fail(error: anyhow::Error) -> ApiError {
    ApiError::bad_request(crate::security::redact_secret_text(&error.to_string()))
}
fn missing(message: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", message)
}
async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<Search>,
) -> Result<Json<Value>, ApiError> {
    let assets = state
        .assets
        .lock()
        .map_err(|_| ApiError::internal("Asset store lock failed"))?
        .list(&q.query, &q.kind, &q.session_id)
        .map_err(fail)?;
    Ok(Json(json!({"assets":assets})))
}
async fn create(
    State(state): State<Arc<AppState>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .assets
                .lock()
                .map_err(|_| ApiError::internal("Asset store lock failed"))?
                .create(&input)
                .map_err(fail)?,
        ),
    ))
}
async fn content(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<Search>,
) -> Result<Json<Value>, ApiError> {
    state
        .assets
        .lock()
        .map_err(|_| ApiError::internal("Asset store lock failed"))?
        .content(&id, q.preview.as_deref() == Some("1"))
        .map_err(fail)?
        .map(Json)
        .ok_or_else(|| missing("资产不存在。"))
}
async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state
        .assets
        .lock()
        .map_err(|_| ApiError::internal("Asset store lock failed"))?
        .delete(&id)
        .map_err(fail)?
    {
        Ok(Json(json!({"deleted":true})))
    } else {
        Err(missing("资产不存在。"))
    }
}
fn byte_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let header = header.trim().strip_prefix("bytes=")?;
    if header.contains(',') {
        return None;
    }
    let (start, end) = header.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 || size == 0 {
            return None;
        }
        Some((size.saturating_sub(suffix), size - 1))
    } else {
        let start = start.parse::<u64>().ok()?;
        let end = if end.is_empty() {
            size.checked_sub(1)?
        } else {
            end.parse::<u64>().ok()?.min(size.checked_sub(1)?)
        };
        if start > end {
            None
        } else {
            Some((start, end))
        }
    }
}
fn encoded_filename(name: &str) -> String {
    let mut output = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}
async fn download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<Search>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let download = state
        .assets
        .lock()
        .map_err(|_| ApiError::internal("Asset store lock failed"))?
        .download(&id)
        .map_err(fail)?
        .ok_or_else(|| missing("资产不存在或不可下载。"))?;
    let requested = headers.get(header::RANGE).and_then(|h| h.to_str().ok());
    let range = requested.and_then(|h| byte_range(h, download.size));
    if requested.is_some() && range.is_none() {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{}", download.size))
            .body(Body::empty())
            .map_err(|e| ApiError::internal(e.to_string()));
    }
    let (start, length) = range
        .map(|(s, e)| (s, e - s + 1))
        .unwrap_or((0, download.size));
    let mut file = tokio::fs::File::open(&download.path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let stream = futures::stream::try_unfold((file, length), |(mut file, remaining)| async move {
        if remaining == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        let mut bytes = vec![0u8; remaining.min(65536) as usize];
        let read = file.read(&mut bytes).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Asset changed during download",
            ));
        }
        bytes.truncate(read);
        Ok(Some((bytes, (file, remaining - read as u64))))
    });
    let mime = download.asset["mimeType"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let name = download.asset["name"].as_str().unwrap_or("attachment");
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, length)
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "{}; filename*=UTF-8''{}",
                if q.inline.as_deref() == Some("1") {
                    "inline"
                } else {
                    "attachment"
                },
                encoded_filename(name)
            ),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=60");
    if let Some((start, end)) = range {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{}", download.size),
        );
    }
    response
        .body(Body::from_stream(stream))
        .map_err(|e| ApiError::internal(e.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_match_release_boundaries() {
        assert_eq!(byte_range("bytes=2-8", 6), Some((2, 5)));
        assert_eq!(byte_range("bytes=-2", 6), Some((4, 5)));
        assert_eq!(byte_range("bytes=6-", 6), None);
        assert_eq!(byte_range("bytes=0-1,3-4", 6), None);
        assert_eq!(byte_range("bytes=-0", 6), None);
        assert_eq!(encoded_filename("中文.txt"), "%E4%B8%AD%E6%96%87.txt");
    }
}
