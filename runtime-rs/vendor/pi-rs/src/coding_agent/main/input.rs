//! main.ts piped input, file/message preparation and non-TTY diagnostics.
//! Image processing defaults to the shared native backend, with an injectable
//! binding for hosts. RPC must not call the piped-stdin helper because RPC owns
//! the same byte stream.
use crate::coding_agent::{
    cli::{
        args::Args,
        file_processor::{
            process_file_arguments, ProcessFileOptions, ProcessFilesError, ProcessImageFn,
        },
        initial_message::{build_initial_message, InitialMessageInput, InitialMessageResult},
    },
    core::agent_session_services::{AgentSessionRuntimeDiagnostic, DiagnosticType},
};
use std::io::{self, Write};
use tokio::io::{AsyncRead, AsyncReadExt};

/// ECMAScript WhiteSpace + LineTerminator, not Rust's wider Unicode whitespace
/// predicate (U+0085 and U+001C must survive, while U+FEFF must be trimmed).
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
/// Never polls a TTY. Buffered UTF-8 decoding also preserves codepoints split
/// across read boundaries and replaces malformed sequences like Node's decoder.
pub async fn read_piped_stdin<R: AsyncRead + Unpin>(
    reader: &mut R,
    is_tty: bool,
) -> io::Result<Option<String>> {
    if is_tty {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    let text = String::from_utf8_lossy(&bytes);
    let text = js_trim(&text);
    Ok((!text.is_empty()).then(|| text.to_owned()))
}
/// Files resolve against the process/startup cwd, not the selected session cwd.
/// As upstream, the first CLI message is consumed only after file reads succeed.
pub fn prepare_initial_message(
    parsed: &mut Args,
    auto_resize_images: bool,
    stdin_content: Option<&str>,
    startup_cwd: &str,
    process_image: ProcessImageFn,
) -> Result<InitialMessageResult, ProcessFilesError> {
    if parsed.file_args.is_empty() {
        return Ok(build_initial_message(InitialMessageInput {
            parsed,
            file_text: None,
            file_images: vec![],
            stdin_content,
        }));
    }
    let files = process_file_arguments(
        &parsed.file_args,
        Some(ProcessFileOptions {
            auto_resize_images: Some(auto_resize_images),
        }),
        process_image,
        startup_cwd,
    )?;
    Ok(build_initial_message(InitialMessageInput {
        parsed,
        file_text: Some(&files.text),
        file_images: files.images,
        stdin_content,
    }))
}
/// Plain/non-TTY rendering. A terminal host may color these same diagnostics.
pub fn report_diagnostics(
    diagnostics: &[AgentSessionRuntimeDiagnostic],
    stderr: &mut dyn Write,
) -> io::Result<()> {
    for diagnostic in diagnostics {
        let prefix = match diagnostic.kind {
            DiagnosticType::Error => "Error: ",
            DiagnosticType::Warning => "Warning: ",
            DiagnosticType::Info => "",
        };
        writeln!(stderr, "{prefix}{}", diagnostic.message)?;
    }
    Ok(())
}
#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
