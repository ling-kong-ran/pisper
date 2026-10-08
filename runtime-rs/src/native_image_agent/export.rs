use super::{
    active, error,
    fs_boundary::{self, Entry, Failure},
    service::{GeneratedFile, ImageAgentService},
    Result, MAX_IMAGE_BYTES,
};
use crate::{
    native_workflow::{image_protocol, media},
    workflow_engine::RunCancellation,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
fn invalid() -> super::WorkflowError {
    error("image_tools_export_invalid", StatusCode::BAD_REQUEST)
}
fn map_failure(failure: Failure) -> super::WorkflowError {
    match failure {
        Failure::Invalid => invalid(),
        Failure::Changed => error("image_tools_export_changed", StatusCode::BAD_REQUEST),
        Failure::Io => error(
            "image_tools_export_failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        Failure::Cancelled => error("image_tools_export_cancelled", StatusCode::CONFLICT),
    }
}
pub(super) struct Stage {
    root: PathBuf,
    entries: Vec<Entry>,
    files: Vec<GeneratedFile>,
}
impl Stage {
    fn cleanup(&self) -> Result<()> {
        fs_boundary::cleanup(&self.root, &self.entries).map_err(|_| {
            error(
                "image_tools_export_cleanup_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })
    }
    fn verify(&self) -> Result<()> {
        fs_boundary::verify(&self.root, &self.entries).map_err(map_failure)
    }
}
fn build(cwd: &Path, bytes: &[u8], atlas: &Value, cancellation: &RunCancellation) -> Result<Stage> {
    active(cancellation, true)?;
    let root = fs_boundary::root(cwd).map_err(map_failure)?;
    let mut entries = vec![root.clone()];
    fs_boundary::child(&root.path, &mut entries, "generated").map_err(map_failure)?;
    fs_boundary::child(&root.path, &mut entries, "image-assets").map_err(map_failure)?;
    fs_boundary::new_directory(&root.path, &mut entries, &uuid::Uuid::new_v4().to_string())
        .map_err(map_failure)?;
    let mut stage = Stage {
        root: root.path,
        entries,
        files: Vec::new(),
    };
    let result = (|| -> Result<()> {
        let path = fs_boundary::write_file(
            &stage.root,
            &stage.entries,
            "atlas.png",
            bytes,
            cancellation,
        )
        .map_err(map_failure)?;
        stage.files.push(GeneratedFile {
            path,
            mime_type: "image/png".into(),
        });
        let mut metadata=serde_json::to_vec_pretty(&json!({"image":"atlas.png","width":atlas["width"],"height":atlas["height"],"frames":atlas["frames"]})).map_err(|_|invalid())?;
        metadata.push(b'\n');
        let path = fs_boundary::write_file(
            &stage.root,
            &stage.entries,
            "frames.json",
            &metadata,
            cancellation,
        )
        .map_err(map_failure)?;
        stage.files.push(GeneratedFile {
            path,
            mime_type: "application/json".into(),
        });
        Ok(())
    })();
    if let Err(failure) = result {
        stage.cleanup()?;
        return Err(failure);
    }
    Ok(stage)
}
fn validate(cwd: &Path, output: &Value) -> Result<Value> {
    let text = cwd.to_string_lossy();
    if !cwd.is_absolute()
        || text.trim().is_empty()
        || text.encode_utf16().count() > 4096
        || text.chars().any(|character| character < '\u{20}')
    {
        return Err(invalid());
    }
    let parsed = image_protocol::output(output).map_err(|_| invalid())?;
    let atlas = parsed
        .get("atlas")
        .filter(|atlas| atlas.is_object())
        .ok_or_else(invalid)?;
    let frames = atlas["frames"]
        .as_array()
        .filter(|frames| !frames.is_empty())
        .ok_or_else(invalid)?;
    let count = parsed["frames"].as_array().ok_or_else(invalid)?.len();
    let width = atlas["width"]
        .as_f64()
        .filter(|value| value.fract() == 0.0)
        .ok_or_else(invalid)?;
    let height = atlas["height"]
        .as_f64()
        .filter(|value| value.fract() == 0.0)
        .ok_or_else(invalid)?;
    if atlas["media"]["mimeType"] != "image/png"
        || frames.len() != count
        || frames.iter().any(|frame| {
            ["x", "y", "width", "height"].iter().any(|key| {
                frame[*key]
                    .as_f64()
                    .is_none_or(|value| value.fract() != 0.0)
            }) || frame["x"].as_f64().unwrap_or(f64::INFINITY)
                + frame["width"].as_f64().unwrap_or(f64::INFINITY)
                > width
                || frame["y"].as_f64().unwrap_or(f64::INFINITY)
                    + frame["height"].as_f64().unwrap_or(f64::INFINITY)
                    > height
        })
    {
        return Err(invalid());
    }
    Ok(atlas.clone())
}
pub(super) async fn export(
    service: &ImageAgentService,
    cwd: PathBuf,
    output: Value,
    cancellation: Arc<RunCancellation>,
) -> Result<Vec<GeneratedFile>> {
    service.allowed(&cancellation, true).await?;
    let atlas = validate(&cwd, &output)?;
    if atlas["media"]["size"].as_u64().unwrap_or(u64::MAX) > MAX_IMAGE_BYTES as u64 {
        return Err(error(
            "image_tools_export_too_large",
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    let stored = service
        .media
        .read(atlas["media"]["id"].as_str().ok_or_else(invalid)?.into())
        .await
        .map_err(|failure| {
            if failure.code == "workflow_media_invalid" {
                invalid()
            } else {
                error(
                    "image_tools_export_failed",
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        });
    active(&cancellation, true)?;
    let stored = stored?;
    if stored.metadata["media"] != atlas["media"]
        || stored.buffer.len() as u64 != atlas["media"]["size"].as_u64().unwrap_or(0)
    {
        return Err(invalid());
    }
    if stored.buffer.len() > MAX_IMAGE_BYTES {
        return Err(error(
            "image_tools_export_too_large",
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    let (width, height, mime) = media::raster_dimensions(&stored.buffer).ok_or_else(invalid)?;
    if mime != "image/png"
        || atlas["width"].as_f64() != Some(f64::from(width))
        || atlas["height"].as_f64() != Some(f64::from(height))
    {
        return Err(invalid());
    }
    media::validate_bytes(&stored.buffer, "image/png").map_err(|_| invalid())?;
    service.allowed(&cancellation, true).await?;
    let token = cancellation.clone();
    let stage = tokio::task::spawn_blocking(move || build(&cwd, &stored.buffer, &atlas, &token))
        .await
        .map_err(|_| {
            error(
                "image_tools_export_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })??;
    let verdict = service.allowed(&cancellation, true).await;
    tokio::task::spawn_blocking(move || {
        let checked = verdict.and_then(|_| stage.verify());
        match checked {
            Ok(()) => Ok(stage.files),
            Err(failure) => {
                stage.cleanup()?;
                active(&cancellation, true)?;
                Err(failure)
            }
        }
    })
    .await
    .map_err(|_| {
        error(
            "image_tools_export_failed",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?
}
