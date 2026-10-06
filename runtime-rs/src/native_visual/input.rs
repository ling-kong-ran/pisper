//! 通用视觉源文件按扩展名读取，不继承工作流的解码、目录或总量限制。
use super::{catalog, SourceImage, VisualError, VisualKind};
use futures::future::try_join_all;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 24 * 1024 * 1024;

fn resolve(cwd: &Path, value: &Value) -> Result<PathBuf, VisualError> {
    let input = catalog::string_or_empty(value)?;
    let input = input.trim_matches(catalog::js_whitespace);
    let path = std::path::absolute(cwd.join(input))?;
    let mut normalized = PathBuf::new();
    // Node resolve 先做词法归一化再 stat；不能 canonicalize 后改变符号链接路径或拒绝外部文件。
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

async fn load_image(input: &Value, cwd: &Path) -> Result<SourceImage, VisualError> {
    let path = resolve(cwd, input)?;
    let info = tokio::fs::metadata(&path).await.ok();
    let Some(info) = info.filter(|info| info.is_file()) else {
        return Err(VisualError::new(format!(
            "编辑源图片不存在：{}",
            path.display()
        )));
    };
    if info.len() > MAX_SOURCE_BYTES {
        return Err(VisualError::new(format!(
            "编辑源图片超过 24 MB：{}",
            path.display()
        )));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let mime_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => {
            return Err(VisualError::new(format!(
                "图片编辑仅支持 PNG、JPEG 或 WebP：{}",
                path.display()
            )));
        }
    };
    let bytes = tokio::fs::read(&path).await?;
    Ok(SourceImage {
        path,
        mime_type: mime_type.into(),
        bytes,
    })
}

pub(super) async fn load_source_images(
    inputs: &Value,
    cwd: &Path,
) -> Result<Vec<SourceImage>, VisualError> {
    let inputs = inputs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|value| catalog::truthy(value))
        .take(8);
    try_join_all(inputs.map(|input| load_image(input, cwd))).await
}

pub(super) async fn load_mask_image(
    input: &Value,
    cwd: &Path,
) -> Result<Option<SourceImage>, VisualError> {
    if !catalog::truthy(input) {
        return Ok(None);
    }
    let mask = load_image(input, cwd).await?;
    if mask.mime_type != "image/png" {
        return Err(VisualError::new("图片编辑蒙版必须是 PNG。"));
    }
    Ok(Some(mask))
}

pub(super) fn validate_video_inputs(
    kind: VisualKind,
    source_images: &Value,
    mask_path: &Value,
) -> Result<(), VisualError> {
    let has_length = match source_images {
        Value::Array(values) => !values.is_empty(),
        Value::String(value) => !value.is_empty(),
        Value::Object(_) => catalog::truthy(&source_images["length"]),
        _ => false,
    };
    if kind == VisualKind::Video && (has_length || catalog::truthy(mask_path)) {
        return Err(VisualError::new("视频生成暂不支持图片编辑参数。"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pisper-visual-input-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn extension_only_sources_filter_and_truncate_without_aggregate_limit() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("arbitrary.PNG"), b"not raster bytes").unwrap();
        std::fs::write(fixture.0.join("source.JpEg"), b"jpeg bytes").unwrap();
        std::fs::write(fixture.0.join("source.webp"), b"webp bytes").unwrap();
        let inputs = json!([
            null,
            false,
            0,
            "",
            "arbitrary.PNG",
            "source.JpEg",
            "source.webp",
            "arbitrary.PNG",
            "source.JpEg",
            "source.webp",
            "arbitrary.PNG",
            "source.JpEg",
            "missing.png"
        ]);
        let images = load_source_images(&inputs, &fixture.0).await.unwrap();
        assert_eq!(images.len(), 8);
        assert_eq!(images[0].bytes, b"not raster bytes");
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[1].mime_type, "image/jpeg");
        assert_eq!(images[2].mime_type, "image/webp");
        assert!(load_source_images(&json!("arbitrary.PNG"), &fixture.0)
            .await
            .unwrap()
            .is_empty());
        assert!(load_source_images(&json!({}), &fixture.0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn source_stat_limit_accepts_24_mib_and_rejects_only_greater_size() {
        let fixture = Fixture::new();
        let path = fixture.0.join("boundary.png");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_SOURCE_BYTES).unwrap();
        let images = load_source_images(&json!(["boundary.png"]), &fixture.0)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.len() as u64, MAX_SOURCE_BYTES);
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        let error = load_source_images(&json!(["boundary.png"]), &fixture.0)
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.message,
            format!("编辑源图片超过 24 MB：{}", path.display())
        );
    }

    #[tokio::test]
    async fn relative_absolute_outside_cwd_and_lexical_paths_are_allowed() {
        let fixture = Fixture::new();
        let cwd = fixture.0.join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let outside = fixture.0.join("outside.png");
        std::fs::write(&outside, b"outside").unwrap();
        let images = load_source_images(
            &json!([
                "../outside.png",
                outside.to_str().unwrap(),
                "../absent/../outside.png"
            ]),
            &cwd,
        )
        .await
        .unwrap();
        assert_eq!(images.len(), 3);
        assert!(images
            .iter()
            .all(|image| image.path == outside && image.bytes == b"outside"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stat_follows_valid_symlink_and_preserves_lexical_link_path() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("outside.bin"), b"link bytes").unwrap();
        std::os::unix::fs::symlink("outside.bin", fixture.0.join("link.png")).unwrap();
        let images = load_source_images(&json!(["link.png"]), &fixture.0)
            .await
            .unwrap();
        assert_eq!(images[0].bytes, b"link bytes");
        assert_eq!(images[0].path, fixture.0.join("link.png"));
    }

    #[tokio::test]
    async fn missing_directory_extension_and_mask_errors_match_reference() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("file.gif"), b"gif").unwrap();
        std::fs::write(fixture.0.join("mask.jpeg"), b"jpeg").unwrap();
        std::fs::write(fixture.0.join("mask.png"), b"unparsed png").unwrap();
        for input in ["missing.png", ""] {
            let path = resolve(&fixture.0, &json!(input)).unwrap();
            let error = load_image(&json!(input), &fixture.0).await.err().unwrap();
            assert_eq!(
                error.message,
                format!("编辑源图片不存在：{}", path.display())
            );
        }
        let error = load_image(&json!("file.gif"), &fixture.0)
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.message,
            format!(
                "图片编辑仅支持 PNG、JPEG 或 WebP：{}",
                fixture.0.join("file.gif").display()
            )
        );
        let error = load_mask_image(&json!("mask.jpeg"), &fixture.0)
            .await
            .err()
            .unwrap();
        assert_eq!(error.message, "图片编辑蒙版必须是 PNG。");
        assert!(load_mask_image(&Value::Null, &fixture.0)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            load_mask_image(&json!("mask.png"), &fixture.0)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"unparsed png"
        );
    }

    #[test]
    fn video_editing_rejection_uses_raw_length_and_mask_truthiness() {
        for sources in [json!([null]), json!("path.png"), json!({"length":1})] {
            assert_eq!(
                validate_video_inputs(VisualKind::Video, &sources, &Value::Null)
                    .unwrap_err()
                    .message,
                "视频生成暂不支持图片编辑参数。"
            );
            assert!(validate_video_inputs(VisualKind::Image, &sources, &Value::Null).is_ok());
        }
        for mask in [json!("mask.png"), json!([]), json!({})] {
            assert!(validate_video_inputs(VisualKind::Video, &json!([]), &mask).is_err());
        }
        assert!(validate_video_inputs(VisualKind::Video, &json!([]), &json!(false)).is_ok());
        assert!(validate_video_inputs(VisualKind::Video, &json!(true), &Value::Null).is_ok());
    }
}
