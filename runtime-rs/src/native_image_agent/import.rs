use super::{
    active, error,
    fs_boundary::{self, Failure},
    service::ImageAgentService,
    Result, MAX_IMAGE_BYTES,
};
use crate::{native_workflow::media, workflow_engine::RunCancellation};
use axum::http::StatusCode;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn invalid() -> super::WorkflowError {
    error("image_tools_source_invalid", StatusCode::BAD_REQUEST)
}
fn map_failure(failure: Failure) -> super::WorkflowError {
    match failure {
        Failure::Invalid => invalid(),
        Failure::Changed => error("image_tools_source_changed", StatusCode::BAD_REQUEST),
        Failure::Io => error("image_tools_source_unavailable", StatusCode::NOT_FOUND),
        Failure::Cancelled => error("workflow_image_cancelled", StatusCode::CONFLICT),
    }
}
fn input(cwd: &Path, source: &str) -> Result<()> {
    let cwd = cwd.to_string_lossy();
    let source = source.trim();
    if cwd.trim().is_empty()
        || cwd.encode_utf16().count() > 4096
        || source.is_empty()
        || source.encode_utf16().count() > 4096
        || cwd
            .chars()
            .chain(source.chars())
            .any(|character| character < '\u{20}')
    {
        return Err(invalid());
    }
    let lower = source.to_ascii_lowercase();
    if ["file:", "data:", "http:", "https:", "ftp:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        || source.split_once("://").is_some_and(|(scheme, _)| {
            let mut letters = scheme.bytes();
            letters
                .next()
                .is_some_and(|letter| letter.is_ascii_alphabetic())
                && letters.all(|letter| {
                    letter.is_ascii_alphanumeric() || [b'+', b'.', b'-'].contains(&letter)
                })
        })
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) async fn import(
    service: &ImageAgentService,
    cwd: PathBuf,
    source: String,
    cancellation: Arc<RunCancellation>,
) -> Result<Value> {
    service.allowed(&cancellation, false).await?;
    input(&cwd, &source)?;
    let token = cancellation.clone();
    let loaded = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String, String)> {
        active(&token, false)?;
        let root = fs_boundary::root(&cwd).map_err(map_failure)?;
        let requested = Path::new(source.trim());
        let path = fs_boundary::normalize(&if requested.is_absolute() {
            PathBuf::from(fs_boundary::display_path(requested))
        } else {
            PathBuf::from(fs_boundary::display_path(&root.path)).join(requested)
        });
        let suffix = fs_boundary::inside(&root.path, &path).ok_or_else(|| {
            error(
                "image_tools_source_outside_workspace",
                StatusCode::FORBIDDEN,
            )
        })?;
        let entries = fs_boundary::inspect_file(&root, &suffix).map_err(map_failure)?;
        let source = entries.last().ok_or_else(invalid)?;
        if source.metadata.len() > MAX_IMAGE_BYTES as u64 {
            return Err(error(
                "image_tools_source_too_large",
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        if source.metadata.len() == 0 {
            return Err(invalid());
        }
        let bytes = fs_boundary::read_file(&root, &entries, MAX_IMAGE_BYTES, &token)
            .map_err(map_failure)?;
        let (width, height, mime) = media::raster_dimensions(&bytes).ok_or_else(invalid)?;
        if !["image/png", "image/jpeg", "image/webp"].contains(&mime) {
            return Err(invalid());
        }
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || u64::from(width) * u64::from(height) > 16_000_000
        {
            return Err(error(
                "image_tools_source_too_large",
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        let name = source
            .path
            .file_name()
            .ok_or_else(invalid)?
            .to_string_lossy()
            .encode_utf16()
            .take(150)
            .collect::<Vec<_>>();
        Ok((bytes, String::from_utf16_lossy(&name), mime.into()))
    })
    .await
    .map_err(|_| error("image_tools_source_unavailable", StatusCode::NOT_FOUND))?;
    // Match the oracle: cancellation wins during reads, but once upload has
    // started its atomic commit returns the actual saved reference even on close.
    active(&cancellation, false)?;
    let (bytes, name, mime) = loaded?;
    service.allowed(&cancellation, false).await?;
    service
        .media
        .upload(name, mime, bytes)
        .await
        .map_err(|failure| {
            if failure.code.starts_with("workflow_media_") {
                error(&failure.code, failure.status)
            } else {
                error(
                    "image_tools_import_failed",
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        })
}
