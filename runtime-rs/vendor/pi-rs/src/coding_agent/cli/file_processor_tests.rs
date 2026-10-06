//! Tests for the ported `coding-agent/src/cli/file-processor.ts` (text path
//! is deterministic and fully ported; the image seam is exercised with the
//! default base64 embedder).

use crate::coding_agent::cli::file_processor::{
    base64_embed_image, process_file_arguments, resolve_read_path, ProcessFileOptions,
    ProcessFilesError, ProcessedFiles,
};
use crate::tui::terminal_image::base64 as vendored_base64;

fn write_file(dir: &str, name: &str, contents: &[u8]) -> String {
    // Host-native join: the temp dir string is platform-flavored (posix
    // separators on linux), so a literal `\` would create a file whose name
    // embeds a backslash instead of a child of the directory.
    let path = std::path::Path::new(dir).join(name);
    std::fs::write(&path, contents).unwrap();
    path.to_string_lossy().into_owned()
}

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "pi-rust-file-processor-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().to_string()
}

fn process(dir: &str, file_args: &[&str]) -> Result<ProcessedFiles, ProcessFilesError> {
    process_file_arguments(
        &file_args
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>(),
        Some(ProcessFileOptions::default()),
        base64_embed_image,
        dir,
    )
}

/// Upstream text path: `<file name="…">\n<contents>\n</file>\n` with the BOM
/// stripped (`stripBom`).
#[test]
fn embeds_text_files_and_strips_bom() {
    let dir = temp_dir("text");
    write_file(&dir, "hello.txt", "\u{feff}body line\nsecond\n".as_bytes());
    let result = process(&dir, &["hello.txt"]).unwrap();
    let absolute = resolve_read_path("hello.txt", &dir);
    // The trailing blank line inside the tag is upstream-faithful: the
    // template is `<file name="…">\n${content}\n</file>\n` and the file ends
    // with its own newline.
    assert_eq!(
        result.text,
        format!("<file name=\"{absolute}\">\nbody line\nsecond\n\n</file>\n")
    );
    assert!(result.images.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// Upstream: empty files are skipped entirely.
#[test]
fn skips_empty_files() {
    let dir = temp_dir("empty");
    write_file(&dir, "empty.txt", b"");
    let result = process(&dir, &["empty.txt"]).unwrap();
    assert_eq!(result.text, "");
    std::fs::remove_dir_all(&dir).ok();
}

/// Upstream: a missing file exits (here: `FileNotFound` with the upstream
/// stderr message as Display).
#[test]
fn missing_file_is_an_error() {
    let dir = temp_dir("missing");
    let error = process(&dir, &["nope.txt"]).err().unwrap();
    match error {
        ProcessFilesError::FileNotFound(ref path) => {
            assert!(path.ends_with("nope.txt"));
            assert_eq!(error.to_string(), format!("Error: File not found: {path}"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Upstream: multiple files concatenate in order; `@` prefixes are stripped
/// by the resolve path.
#[test]
fn multiple_files_concatenate_in_order() {
    let dir = temp_dir("multi");
    write_file(&dir, "a.txt", b"A");
    write_file(&dir, "b.txt", b"B");
    let result = process(&dir, &["a.txt", "b.txt"]).unwrap();
    let a = resolve_read_path("a.txt", &dir);
    let b = resolve_read_path("b.txt", &dir);
    assert_eq!(
        result.text,
        format!("<file name=\"{a}\">\nA\n</file>\n<file name=\"{b}\">\nB\n</file>\n")
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The image branch through the default seam: PNG magic sniffs as an image,
/// the bytes are base64-embedded, and the text reference carries no hints.
#[test]
fn images_embed_base64_with_reference_text() {
    let dir = temp_dir("image");
    // Minimal valid PNG header (the mime sniffer checks the signature).
    let png: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];
    write_file(&dir, "pic.png", png);
    let result = process(&dir, &["pic.png"]).unwrap();
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].mime_type, "image/png");
    assert_eq!(result.images[0].data, vendored_base64::encode(png));
    let absolute = resolve_read_path("pic.png", &dir);
    assert_eq!(result.text, format!("<file name=\"{absolute}\"></file>\n"));
    std::fs::remove_dir_all(&dir).ok();
}

/// `resolveReadPath`'s macOS-screenshot fallback: the narrow-space variant
/// resolves when the plain name misses.
#[test]
fn resolve_read_path_tries_the_macos_screenshot_variant() {
    let dir = temp_dir("screenshot");
    // Upstream replace: " AM." -> U+202F + "AM." (the plain space is consumed).
    // Host-native join so the fixture file is a child of the temp dir on both
    // platforms and the expectation matches `resolve_read_path`'s own
    // separator flavor.
    let narrow = std::path::Path::new(&dir).join("Screenshot\u{202F}AM.");
    std::fs::write(&narrow, b"x").unwrap();
    assert_eq!(
        resolve_read_path("Screenshot AM.", &dir),
        narrow.to_string_lossy()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `resolveReadPath`'s curly-quote fallback (U+0027 typed, U+2019 on disk).
#[test]
fn resolve_read_path_tries_the_curly_quote_variant() {
    let dir = temp_dir("curly");
    // Host-native join (see the screenshot variant above).
    let curly = std::path::Path::new(&dir).join("Capture d\u{2019}cran.txt");
    std::fs::write(&curly, b"x").unwrap();
    assert_eq!(
        resolve_read_path("Capture d'cran.txt", &dir),
        curly.to_string_lossy()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_buffer_decoding_replaces_invalid_utf8_then_strips_only_one_bom() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("invalid.txt");
    std::fs::write(
        &file,
        b"\xef\xbb\xbf\xef\xbb\xbfhello\xff\xf0\x9f\x99\x82\xe2\x82",
    )
    .unwrap();
    let cwd = dir.path().to_str().unwrap();
    let result = process(cwd, &["invalid.txt"]).unwrap();
    let absolute = resolve_read_path("invalid.txt", cwd);
    assert_eq!(
        result.text,
        format!("<file name=\"{absolute}\">\n\u{feff}hello\u{fffd}🙂\u{fffd}\n</file>\n")
    );
    assert!(result.images.is_empty());
}
