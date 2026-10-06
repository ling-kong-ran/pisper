//! 生成文件遵循 release 的秒级命名和 writeFile 覆盖语义；通用输出不做栅格校验。
use super::{catalog, DriverResult, VisualError};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn safe_name_checked(value: &Value) -> Result<String, VisualError> {
    let value = catalog::string_or_empty(value)?;
    let value = value.trim_matches(catalog::js_whitespace);
    let extension =
        Regex::new(r"\.[A-Za-z0-9]{2,5}$").map_err(|error| VisualError::new(error.to_string()))?;
    let invalid =
        Regex::new(r"[^\p{L}\p{N}._-]+").map_err(|error| VisualError::new(error.to_string()))?;
    let value = extension.replace(value, "");
    let value = invalid.replace_all(&value, "-");
    Ok(catalog::slice_utf16(value.trim_matches('-'), 80))
}

pub(super) fn safe_name(value: &Value) -> Result<String, VisualError> {
    safe_name_checked(value)
}

fn filename(
    prompt: &str,
    output_name: Option<&str>,
    extension: &str,
    now: DateTime<Utc>,
) -> Result<String, VisualError> {
    let name = safe_name_checked(&Value::String(output_name.unwrap_or_default().into()))?;
    let name = if name.is_empty() {
        let prompt = safe_name_checked(&Value::String(prompt.into()))?;
        catalog::slice_utf16(&prompt, 42)
    } else {
        name
    };
    let name = if name.is_empty() { "visual" } else { &name };
    let stamp = now.format("%Y-%m-%dT%H-%M-%S");
    Ok(format!("{stamp}-{name}{extension}"))
}

async fn save_at(
    cwd: &Path,
    prompt: &str,
    output_name: Option<&str>,
    result: &DriverResult,
    now: DateTime<Utc>,
) -> Result<PathBuf, VisualError> {
    let directory = cwd.join("generated").join("visuals");
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(filename(prompt, output_name, &result.extension, now)?);
    tokio::fs::write(&path, &result.bytes).await?;
    Ok(path)
}

pub(super) async fn save_visual_output(
    cwd: &Path,
    prompt: &str,
    output_name: Option<&str>,
    result: &DriverResult,
) -> Result<PathBuf, VisualError> {
    save_at(cwd, prompt, output_name, result, Utc::now()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn safe_names_match_node_oracle_unicode_and_utf16_boundaries() {
        for (input, expected) in [
            (json!(" logo.png "), "logo"),
            (json!("../目录/图 logo.png"), "..-目录-图-logo"),
            (json!("😀"), ""),
            (json!("hello.tar.gz"), "hello.tar"),
            (json!("x.jpeg"), "x"),
            (json!("x.abcdef"), "x.abcdef"),
            (json!("!!!___--..."), "___--..."),
            (json!("Ⅰ²½一𐐀"), "Ⅰ²½一𐐀"),
            (json!("\u{feff}name.png\u{feff}"), "name"),
            (json!(0), ""),
            (json!(false), ""),
            (Value::Null, ""),
            (json!(["one", "two"]), "one-two"),
        ] {
            assert_eq!(safe_name_checked(&input).unwrap(), expected);
        }
        assert_eq!(
            safe_name(&json!(format!("{}𐐀", "a".repeat(79)))).unwrap(),
            format!("{}\u{fffd}", "a".repeat(79))
        );
        assert_eq!(
            safe_name_checked(&json!({"toString":"invalid"}))
                .unwrap_err()
                .message,
            "Cannot convert object to primitive value"
        );
    }

    #[test]
    fn filenames_use_utc_seconds_explicit_name_prompt42_and_visual_fallback() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 1, 2, 3).unwrap();
        assert_eq!(
            filename("ignored", Some(" logo.png "), ".webp", now).unwrap(),
            "2026-10-06T01-02-03-logo.webp"
        );
        assert_eq!(
            filename("😀", None, ".mp4", now).unwrap(),
            "2026-10-06T01-02-03-visual.mp4"
        );
        assert_eq!(
            filename(&format!("{}𐐀", "a".repeat(41)), Some("😀"), ".jpg", now).unwrap(),
            format!("2026-10-06T01-02-03-{}\u{fffd}.jpg", "a".repeat(41))
        );
    }

    #[tokio::test]
    async fn output_creates_new_cwd_preserves_large_bytes_and_overwrites_same_second() {
        let root = std::env::temp_dir().join(format!("pisper-visual-output-{}", Uuid::new_v4()));
        let cwd = root.join("new-cwd").join("visual-test");
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 1, 2, 3).unwrap();
        let mut result = DriverResult {
            bytes: vec![7; 8 * 1024 * 1024 + 1],
            mime_type: "image/webp".into(),
            extension: ".webp".into(),
            remote_id: None,
        };
        let path = save_at(&cwd, "prompt", Some("config-test"), &result, now)
            .await
            .unwrap();
        assert_eq!(
            path,
            cwd.join("generated/visuals/2026-10-06T01-02-03-config-test.webp")
        );
        assert_eq!(std::fs::read(&path).unwrap(), result.bytes);
        result.bytes = b"replacement without raster decode".to_vec();
        let replaced = save_at(&cwd, "prompt", Some("config-test"), &result, now)
            .await
            .unwrap();
        assert_eq!(path, replaced);
        assert_eq!(std::fs::read(&path).unwrap(), result.bytes);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn failed_write_does_not_return_a_generated_path() {
        let root =
            std::env::temp_dir().join(format!("pisper-visual-write-error-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("generated"), b"not a directory").unwrap();
        let result = DriverResult {
            bytes: b"data".to_vec(),
            mime_type: "video/mp4".into(),
            extension: ".mp4".into(),
            remote_id: None,
        };
        assert!(save_visual_output(&root, "prompt", None, &result)
            .await
            .is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
