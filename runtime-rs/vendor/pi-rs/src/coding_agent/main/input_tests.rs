use super::*;
use crate::coding_agent::cli::file_processor::base64_embed_image;
use serde_json::{json, Value};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::ReadBuf;
fn oracle() -> Value {
    serde_json::from_str(include_str!("input_oracle.json")).unwrap()
}
struct Chunks {
    bytes: Vec<u8>,
    pos: usize,
    chunk: usize,
    tty: bool,
}
impl AsyncRead for Chunks {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        assert!(!self.tty, "TTY must not be read");
        let end = (self.pos + self.chunk)
            .min(self.bytes.len())
            .min(self.pos + buffer.remaining());
        buffer.put_slice(&self.bytes[self.pos..end]);
        self.pos = end;
        Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn stdin_utf8_chunk_boundaries_and_exact_js_whitespace_match_real_node_stream() {
    for case in oracle()["stdin"].as_array().unwrap() {
        for chunk in [1, 2, 3, 4096] {
            let tty = case["tty"].as_bool().unwrap();
            let mut reader = Chunks {
                bytes: serde_json::from_value(case["bytes"].clone()).unwrap(),
                pos: 0,
                chunk,
                tty,
            };
            let value = read_piped_stdin(&mut reader, tty).await.unwrap();
            assert_eq!(json!(value), case["value"], "{} chunk={chunk}", case["id"]);
        }
    }
}
#[test]
fn diagnostics_match_upstream_non_tty_stderr_including_embedded_newlines() {
    let data = oracle();
    let input: Vec<AgentSessionRuntimeDiagnostic> =
        serde_json::from_value(data["diagnostics"]["input"].clone()).unwrap();
    let mut out = vec![];
    report_diagnostics(&input, &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        data["diagnostics"]["stderr"].as_str().unwrap()
    );
}
#[test]
fn real_files_stdin_and_messages_use_startup_paths_and_consume_only_first_message() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("first.txt"), "\u{feff}file content").unwrap();
    let mut parsed = Args {
        file_args: vec!["first.txt".into()],
        messages: vec!["first message".into(), "second message".into()],
        ..Default::default()
    };
    let result = prepare_initial_message(
        &mut parsed,
        false,
        Some("piped"),
        dir.path().to_str().unwrap(),
        base64_embed_image,
    )
    .unwrap();
    let path = crate::coding_agent::cli::file_processor::resolve_read_path(
        "first.txt",
        dir.path().to_str().unwrap(),
    );
    assert_eq!(
        result.initial_message,
        Some(format!(
            "piped<file name=\"{path}\">\nfile content\n</file>\nfirst message"
        ))
    );
    assert_eq!(parsed.messages, ["second message"]);
    assert!(result.initial_images.is_none());
}
#[test]
fn file_error_does_not_consume_message_and_no_files_does_not_read_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let mut parsed = Args {
        file_args: vec!["missing".into()],
        messages: vec!["keep".into()],
        ..Default::default()
    };
    assert!(prepare_initial_message(
        &mut parsed,
        true,
        None,
        dir.path().to_str().unwrap(),
        base64_embed_image
    )
    .is_err());
    assert_eq!(parsed.messages, ["keep"]);
    parsed.file_args.clear();
    let result = prepare_initial_message(
        &mut parsed,
        true,
        None,
        "nonexistent/path",
        base64_embed_image,
    )
    .unwrap();
    assert_eq!(result.initial_message, Some("keep".into()));
    assert!(parsed.messages.is_empty());
}
