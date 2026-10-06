//! 附件文本嗅探与 MIME 契约，按 release asset-content.mjs 的边界处理。
use std::path::Path;
/// Node Buffer.from(..., 'base64') 接受 URL-safe 字符、缺省 padding 和夹杂空白。
pub fn decode_base64(value: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(value.len().saturating_mul(3) / 4);
    let mut bits = 0u32;
    let mut length = 0u8;
    for byte in value.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        bits = (bits << 6) | u32::from(digit);
        length += 6;
        if length >= 8 {
            length -= 8;
            result.push((bits >> length) as u8);
        }
        bits &= (1u32 << length) - 1;
    }
    result
}
pub fn truncate_utf16(value: &str, limit: usize) -> String {
    let mut length = 0;
    value
        .chars()
        .take_while(|c| {
            length += c.len_utf16();
            length <= limit
        })
        .collect()
}
pub fn safe_name(value: &str) -> String {
    let value = if value.is_empty() { "附件" } else { value };
    value
        .chars()
        .map(|c| {
            if ['\r', '\n', '<', '>'].contains(&c) {
                '_'
            } else {
                c
            }
        })
        .take(180)
        .collect()
}
pub fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}
pub fn mime_from_name(name: &str) -> &'static str {
    match extension(name).as_str() {
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".bmp" => "image/bmp",
        ".svg" => "image/svg+xml",
        ".txt" | ".log" | ".ps1" | ".cs" | ".dart" | ".env" | ".ini" | ".kt" | ".kts"
        | ".properties" | ".rst" => "text/plain",
        ".md" | ".markdown" => "text/markdown",
        ".csv" => "text/csv",
        ".json" | ".ipynb" => "application/json",
        ".jsonl" => "application/x-ndjson",
        ".js" | ".jsx" => "text/javascript",
        ".ts" | ".tsx" => "text/typescript",
        ".css" | ".less" => "text/css",
        ".html" | ".vue" => "text/html",
        ".xml" => "application/xml",
        ".yaml" | ".yml" => "application/yaml",
        ".toml" => "application/toml",
        ".py" => "text/x-python",
        ".java" => "text/x-java",
        ".go" => "text/x-go",
        ".rs" => "text/x-rust",
        ".sh" => "text/x-shellscript",
        ".sql" => "application/sql",
        ".c" | ".h" => "text/x-c",
        ".cc" | ".cpp" | ".hpp" => "text/x-c++",
        ".graphql" => "application/graphql",
        ".lua" => "text/x-lua",
        ".m" => "text/x-objective-c",
        ".mm" => "text/x-objective-c++",
        ".php" => "text/x-php",
        ".rb" => "text/x-ruby",
        ".sass" => "text/x-sass",
        ".scss" => "text/x-scss",
        ".swift" => "text/x-swift",
        ".tex" => "application/x-tex",
        ".pdf" => "application/pdf",
        ".docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ".pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ".xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ".mp4" | ".m4v" => "video/mp4",
        ".webm" => "video/webm",
        ".mov" => "video/quicktime",
        ".avi" => "video/x-msvideo",
        ".mkv" => "video/x-matroska",
        ".mpeg" | ".mpg" => "video/mpeg",
        ".ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}
pub fn is_image(name: &str) -> bool {
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"].contains(&extension(name).as_str())
}
pub fn is_document(name: &str) -> bool {
    [
        ".pdf", ".docx", ".pptx", ".xlsx", ".odt", ".odp", ".ods", ".rtf", ".epub",
    ]
    .contains(&extension(name).as_str())
}
pub fn mime_text(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or("").trim().to_lowercase();
    mime.starts_with("text/")
        || [
            "application/json",
            "application/ld+json",
            "application/graphql",
            "application/sql",
            "application/toml",
            "application/xml",
            "application/yaml",
            "application/x-ndjson",
        ]
        .contains(&mime.as_str())
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}
pub fn known_text(name: &str, mime: &str) -> bool {
    mime_text(mime)
        || [
            ".txt",
            ".md",
            ".markdown",
            ".json",
            ".jsonl",
            ".js",
            ".jsx",
            ".ts",
            ".tsx",
            ".css",
            ".html",
            ".xml",
            ".yaml",
            ".yml",
            ".csv",
            ".log",
            ".py",
            ".java",
            ".go",
            ".rs",
            ".sh",
            ".ps1",
            ".toml",
            ".sql",
            ".c",
            ".cc",
            ".cpp",
            ".cs",
            ".dart",
            ".env",
            ".graphql",
            ".h",
            ".hpp",
            ".ini",
            ".ipynb",
            ".kt",
            ".kts",
            ".less",
            ".lua",
            ".m",
            ".mm",
            ".php",
            ".properties",
            ".rb",
            ".rst",
            ".sass",
            ".scss",
            ".swift",
            ".tex",
            ".vue",
        ]
        .contains(&extension(name).as_str())
}
pub fn decode_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains('\0') {
        return None;
    }
    let controls = text
        .chars()
        .filter(|c| (*c as u32) < 32 && !['\n', '\r', '\t', '\u{000c}', '\u{0008}'].contains(c))
        .count();
    if controls as f64 > (2.0_f64).max(text.encode_utf16().count() as f64 * 0.01) {
        None
    } else {
        Some(text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_and_utf8_are_distinct() {
        assert_eq!(decode_text("中文🦀".as_bytes()), Some("中文🦀"));
        assert_eq!(decode_text(&[0xff]), None);
        assert_eq!(decode_text(b"a\0b"), None);
        assert_eq!(safe_name("a\n<b>.txt"), "a__b_.txt");
    }
    #[test]
    fn node_base64_boundaries_and_utf16_limit() {
        assert_eq!(decode_base64("YQ"), b"a");
        assert_eq!(decode_base64("Y! W\nJj==ignored"), b"abc");
        assert_eq!(decode_base64("-_8"), [251, 255]);
        assert_eq!(decode_base64("Y"), b"");
        assert_eq!(truncate_utf16("a🦀b", 3), "a🦀");
    }
}
