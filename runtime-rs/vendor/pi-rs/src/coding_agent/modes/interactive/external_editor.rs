//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/external-editor.ts`
//! (46 lines, sha256 …) — launch `$EDITOR`-style command on a temp file and
//! read the result back.
//!
//! Upstream test: `test/external-editor.test.ts` (three scenarios ported in
//! `interactive_tests.rs`); the temp-directory choreography core is separated
//! from the child-process spawn so the scenarios can run against a stub
//! runner, plus real-spawn tests for the empty-content and failed-exit paths.

use std::future::Future;
use std::path::Path;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::coding_agent::utils::text::strip_bom;

/// Upstream `ExternalEditorOptions`.
#[derive(Debug, Clone)]
pub struct ExternalEditorOptions {
    pub command: String,
    pub content: String,
}

/// Upstream `ExternalEditorResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEditorResult {
    Complete { content: String },
    Failed,
}

/// The child-process seam: run `editor` with `args` (the prompt file path is
/// appended by the caller) and resolve to the exit code, `None` on spawn error.
pub trait EditorRunner {
    fn run(
        &self,
        editor: &str,
        editor_args: &[String],
        file_path: &Path,
    ) -> impl Future<Output = Option<i32>> + Send;
}

/// Upstream `editInExternalEditor` with an injected runner (the temp-dir
/// choreography, BOM/newline normalization, and cleanup are verbatim).
pub async fn edit_in_external_editor_with<R: EditorRunner>(
    options: &ExternalEditorOptions,
    runner: &R,
) -> ExternalEditorResult {
    let directory = std::env::temp_dir().join(format!(
        "pi-editor-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("mkdtemp equivalent");
    let file_path = directory.join("prompt.md");
    let result = async {
        std::fs::write(&file_path, &options.content).expect("write prompt");
        let mut parts = options.command.split(' ');
        let editor = parts.next().unwrap_or_default().to_string();
        let editor_args: Vec<String> = parts.map(str::to_string).collect();
        println!("Launching external editor: {}", options.command);
        println!("Pi will resume when the editor exits.");

        let exit_code = runner.run(&editor, &editor_args, &file_path).await;

        if exit_code != Some(0) {
            return ExternalEditorResult::Failed;
        }

        let content = std::fs::read_to_string(&file_path).unwrap_or_default();
        ExternalEditorResult::Complete {
            // stripBom(...).replace(/\n$/, "")
            content: strip_bom(&content)
                .strip_suffix('\n')
                .map(str::to_string)
                .unwrap_or(content),
        }
    }
    .await;
    // Cleanup is best effort.
    let _ = std::fs::remove_dir_all(&directory);
    result
}

/// The real spawn runner mirroring upstream: `stdio: "inherit"`,
/// `shell: process.platform === "win32"`.
pub struct ProcessEditorRunner;

impl EditorRunner for ProcessEditorRunner {
    async fn run(&self, editor: &str, editor_args: &[String], file_path: &Path) -> Option<i32> {
        let mut command = build_command(editor, editor_args, file_path)?;
        match command.status().await {
            Ok(status) => status.code(),
            Err(_) => None,
        }
    }
}

/// Build the child command. Upstream: `spawn(editor, [...args, filePath], {
/// stdio: "inherit", shell: process.platform === "win32" })` — node's win32
/// shell mode runs `cmd /d /s /c "<editor> <args…> <filePath>"`.
#[cfg(windows)]
fn build_command(
    editor: &str,
    editor_args: &[String],
    file_path: &Path,
) -> Option<tokio::process::Command> {
    let mut line = editor.to_string();
    for arg in editor_args {
        line.push(' ');
        line.push_str(arg);
    }
    line.push(' ');
    line.push_str(&file_path.to_string_lossy());
    let mut command = tokio::process::Command::new("cmd");
    command.arg("/d").arg("/s").arg("/c");
    command.as_std_mut().raw_arg(line);
    Some(command)
}

#[cfg(not(windows))]
fn build_command(
    editor: &str,
    editor_args: &[String],
    file_path: &Path,
) -> Option<tokio::process::Command> {
    let mut command = tokio::process::Command::new(editor);
    command.args(editor_args).arg(file_path);
    Some(command)
}

/// Upstream `editInExternalEditor`.
pub async fn edit_in_external_editor(options: &ExternalEditorOptions) -> ExternalEditorResult {
    edit_in_external_editor_with(options, &ProcessEditorRunner).await
}
