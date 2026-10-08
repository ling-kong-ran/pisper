//! Tests for the ported `coding-agent/src/cli/initial-message.ts` (upstream
//! `test/initial-message.test.ts`, ported in full).

use crate::ai::types::content::ImageContent;
use crate::coding_agent::cli::args::Args;
use crate::coding_agent::cli::initial_message::{build_initial_message, InitialMessageInput};

fn create_args(messages: &[&str]) -> Args {
    Args {
        messages: messages.iter().map(|message| message.to_string()).collect(),
        ..Args::default()
    }
}

fn image(data: &str) -> ImageContent {
    ImageContent {
        data: data.to_string(),
        mime_type: "image/png".to_string(),
    }
}

/// "merges piped stdin with the first CLI message into one prompt"
#[test]
fn merges_stdin_with_first_cli_message() {
    let mut parsed = create_args(&["Summarize the text given"]);
    let result = build_initial_message(InitialMessageInput {
        parsed: &mut parsed,
        file_text: None,
        file_images: Vec::new(),
        stdin_content: Some("README contents\n"),
    });
    assert_eq!(
        result.initial_message.as_deref(),
        Some("README contents\nSummarize the text given")
    );
    assert!(parsed.messages.is_empty());
}

/// "uses stdin as the initial prompt when no CLI message is present"
#[test]
fn uses_stdin_when_no_cli_message() {
    let mut parsed = create_args(&[]);
    let result = build_initial_message(InitialMessageInput {
        parsed: &mut parsed,
        file_text: None,
        file_images: Vec::new(),
        stdin_content: Some("README contents"),
    });
    assert_eq!(result.initial_message.as_deref(), Some("README contents"));
    assert!(parsed.messages.is_empty());
}

/// "combines stdin, file text, and first CLI message in one prompt"
#[test]
fn combines_stdin_file_text_and_first_message() {
    let mut parsed = create_args(&["Explain it", "Second message"]);
    let result = build_initial_message(InitialMessageInput {
        parsed: &mut parsed,
        file_text: Some("file\n"),
        file_images: Vec::new(),
        stdin_content: Some("stdin\n"),
    });
    assert_eq!(
        result.initial_message.as_deref(),
        Some("stdin\nfile\nExplain it")
    );
    assert_eq!(parsed.messages, vec!["Second message".to_string()]);
}

/// The `fileImages` pass-through branch of `buildInitialMessage`.
#[test]
fn file_images_pass_through() {
    let mut parsed = create_args(&[]);
    let result = build_initial_message(InitialMessageInput {
        parsed: &mut parsed,
        file_text: Some("file\n"),
        file_images: vec![image("aGk=")],
        stdin_content: None,
    });
    assert_eq!(result.initial_message.as_deref(), Some("file\n"));
    assert_eq!(result.initial_images, Some(vec![image("aGk=")]));
}

/// `initialImages` stays undefined without attachments.
#[test]
fn no_images_stays_none() {
    let mut parsed = create_args(&["hello"]);
    let result = build_initial_message(InitialMessageInput {
        parsed: &mut parsed,
        file_text: None,
        file_images: Vec::new(),
        stdin_content: None,
    });
    assert_eq!(result.initial_message.as_deref(), Some("hello"));
    assert_eq!(result.initial_images, None);
}
