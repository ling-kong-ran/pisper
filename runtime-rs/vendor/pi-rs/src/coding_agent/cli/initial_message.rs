//! Port of upstream `coding-agent/src/cli/initial-message.ts` (sha256
//! b9df15ffa876…): combine stdin content, `@file` text and the first CLI
//! message into the initial prompt for non-interactive mode.

use super::args::Args;

/// Upstream `InitialMessageInput`. `file_images` is carried through unchanged
/// (the port's [`crate::ai::types::content::ImageContent`]).
pub struct InitialMessageInput<'a> {
    pub parsed: &'a mut Args,
    pub file_text: Option<&'a str>,
    pub file_images: Vec<crate::ai::types::content::ImageContent>,
    pub stdin_content: Option<&'a str>,
}

/// Upstream `InitialMessageResult`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct InitialMessageResult {
    pub initial_message: Option<String>,
    pub initial_images: Option<Vec<crate::ai::types::content::ImageContent>>,
}

/// Upstream `buildInitialMessage`.
pub fn build_initial_message(input: InitialMessageInput<'_>) -> InitialMessageResult {
    let mut parts: Vec<String> = Vec::new();
    if let Some(stdin_content) = input.stdin_content {
        parts.push(stdin_content.to_string());
    }
    if let Some(file_text) = input.file_text {
        if !file_text.is_empty() {
            parts.push(file_text.to_string());
        }
    }

    if !input.parsed.messages.is_empty() {
        parts.push(input.parsed.messages[0].clone());
        input.parsed.messages.remove(0);
    }

    InitialMessageResult {
        initial_message: if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        },
        initial_images: if input.file_images.is_empty() {
            None
        } else {
            Some(input.file_images)
        },
    }
}

#[cfg(test)]
#[path = "initial_message_tests.rs"]
mod tests;
