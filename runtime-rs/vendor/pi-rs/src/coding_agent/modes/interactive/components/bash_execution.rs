//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/bash-execution.ts` (220 lines, sha256
//! `59e252e04122eeb849daa7a5cdb02f98c7cac24fec4f0aed6291df10c5d861e8`):
//! bash command execution display with streaming output, context truncation
//! (same limits as the bash tool) and a collapsed visual-line preview.
//!
//! The upstream constructor assembles a Container frame (spacer / border /
//! content container / border) whose content container it rebuilds on every
//! state change; the port exposes the same rebuilt child sequence as
//! [`BashExecutionComponent::content_payload`] and renders the identical frame
//! through [`BashExecutionComponent::render`].

use std::sync::Arc;

use crate::coding_agent::core::tools::truncate::{
    truncate_tail, TruncationOptions, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use crate::coding_agent::modes::interactive::components::loader::Loader;
use crate::coding_agent::modes::interactive::components::model_selector::{
    key_hint, key_text, theme_fg,
};
use crate::coding_agent::modes::interactive::components::visual_truncate::{
    truncate_to_visual_lines, Keep,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::utils::ansi::strip_ansi;
use crate::tui::component::Component;
use crate::tui::components::text::Text;

const PREVIEW_LINES: usize = 20;

/// Upstream `TruncationResult` subset `setComplete` accepts.
#[derive(Clone, Debug, Default)]
pub struct BashTruncationResult {
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BashStatus {
    Running,
    Complete,
    Cancelled,
    Error,
}

/// The rebuilt content-container child sequence (upstream pushes Text widgets,
/// an inline preview object and the loader).
#[derive(Clone, Debug)]
pub enum ChildLine {
    /// A `Text(text, paddingX, 0)` child.
    Text(ChildText),
    /// The inline cached preview object (`truncateToVisualLines` at width).
    Preview(PreviewPayload),
    /// The live loader child.
    Loader,
}

#[derive(Clone, Debug)]
pub struct ChildText {
    pub text: String,
    pub padding_x: usize,
}

/// The inline preview object upstream pushes (`render(width)` caching the
/// `truncateToVisualLines` result per width).
#[derive(Clone, Debug)]
pub struct PreviewPayload {
    pub styled_input: String,
    pub max_lines: usize,
}

impl PreviewPayload {
    pub fn render(&self, width: usize) -> Vec<String> {
        truncate_to_visual_lines(&self.styled_input, self.max_lines, width, 1, Keep::End)
            .visual_lines
    }
}

/// Upstream `BashExecutionComponent`.
pub struct BashExecutionComponent {
    command: String,
    exclude_from_context: bool,
    output_lines: Vec<String>,
    status: BashStatus,
    exit_code: Option<i32>,
    loader: Loader,
    truncation_result: Option<BashTruncationResult>,
    full_output_path: Option<String>,
    expanded: bool,
    theme: Arc<Theme>,
}

impl BashExecutionComponent {
    /// Upstream constructor (`excludeFromContext` = the `!!` prefix dim mode).
    pub fn new(theme: Arc<Theme>, command: &str, exclude_from_context: bool) -> Self {
        let color_key: &'static str = if exclude_from_context {
            "dim"
        } else {
            "bashMode"
        };
        let loader_spinner_theme = Arc::clone(&theme);
        let loader_message_theme = Arc::clone(&theme);
        let loader = Loader::new(
            Arc::new(move |s: &str| theme_fg(&loader_spinner_theme, color_key, s)),
            Arc::new(move |s: &str| theme_fg(&loader_message_theme, "muted", s)),
            format!("Running... ({} to cancel)", key_text("tui.select.cancel")),
            None,
        );
        Self {
            command: command.to_string(),
            exclude_from_context,
            output_lines: Vec::new(),
            status: BashStatus::Running,
            exit_code: None,
            loader,
            truncation_result: None,
            full_output_path: None,
            expanded: false,
            theme,
        }
    }

    /// Upstream `setExpanded`.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
        self.update_display();
    }

    /// Upstream `appendOutput`: strip ANSI, normalize line endings, continue
    /// incomplete trailing lines.
    pub fn append_output(&mut self, chunk: &str) {
        let clean = strip_ansi(chunk).replace("\r\n", "\n").replace('\r', "\n");
        let new_lines: Vec<String> = clean.split('\n').map(str::to_string).collect();
        if !self.output_lines.is_empty() && !new_lines.is_empty() {
            let last = self.output_lines.len() - 1;
            self.output_lines[last].push_str(&new_lines[0]);
            self.output_lines.extend_from_slice(&new_lines[1..]);
        } else {
            self.output_lines.extend(new_lines);
        }
        self.update_display();
    }

    /// Upstream `setComplete`.
    pub fn set_complete(
        &mut self,
        exit_code: Option<i32>,
        cancelled: bool,
        truncation_result: Option<BashTruncationResult>,
        full_output_path: Option<&str>,
    ) {
        self.exit_code = exit_code;
        self.status = if cancelled {
            BashStatus::Cancelled
        } else if exit_code.is_some_and(|code| code != 0) {
            BashStatus::Error
        } else {
            BashStatus::Complete
        };
        self.truncation_result = truncation_result;
        self.full_output_path = full_output_path.map(str::to_string);
        self.loader.stop();
        self.update_display();
    }

    /// Upstream `updateDisplay` (the rebuilt content-container children).
    fn update_display(&mut self) -> Vec<ChildLine> {
        let full_output = self.output_lines.join("\n");
        let context_truncation = truncate_tail(
            &full_output,
            TruncationOptions {
                max_lines: Some(DEFAULT_MAX_LINES),
                max_bytes: Some(DEFAULT_MAX_BYTES),
            },
        );

        let available_lines: Vec<String> = if context_truncation.content.is_empty() {
            Vec::new()
        } else {
            context_truncation
                .content
                .split('\n')
                .map(str::to_string)
                .collect()
        };

        let preview_logical_lines: Vec<String> = if available_lines.len() > PREVIEW_LINES {
            available_lines[available_lines.len() - PREVIEW_LINES..].to_vec()
        } else {
            available_lines.clone()
        };
        let hidden_line_count = available_lines.len() - preview_logical_lines.len();

        let mut children: Vec<ChildLine> = Vec::new();

        // Command header
        children.push(ChildLine::Text(ChildText {
            text: theme_fg(
                &self.theme,
                "bashMode",
                &self.theme.bold(&format!("$ {}", self.command)),
            ),
            padding_x: 1,
        }));

        // Output
        if !available_lines.is_empty() {
            if self.expanded {
                let display_text = available_lines
                    .iter()
                    .map(|line| theme_fg(&self.theme, "muted", line))
                    .collect::<Vec<_>>()
                    .join("\n");
                children.push(ChildLine::Text(ChildText {
                    text: format!("\n{display_text}"),
                    padding_x: 1,
                }));
            } else {
                let styled_output = preview_logical_lines
                    .iter()
                    .map(|line| theme_fg(&self.theme, "muted", line))
                    .collect::<Vec<_>>()
                    .join("\n");
                let styled_input = format!("\n{styled_output}");
                children.push(ChildLine::Preview(PreviewPayload {
                    styled_input,
                    max_lines: PREVIEW_LINES,
                }));
            }
        }

        // Loader or status
        if self.status == BashStatus::Running {
            children.push(ChildLine::Loader);
        } else {
            let mut status_parts: Vec<String> = Vec::new();
            if hidden_line_count > 0 {
                if self.expanded {
                    status_parts.push(format!(
                        "{}{}{}",
                        theme_fg(&self.theme, "muted", "("),
                        key_hint(&self.theme, "app.tools.expand", "to collapse"),
                        theme_fg(&self.theme, "muted", ")")
                    ));
                } else {
                    status_parts.push(format!(
                        "{}{}{}",
                        theme_fg(
                            &self.theme,
                            "muted",
                            &format!("... {hidden_line_count} more lines (")
                        ),
                        key_hint(&self.theme, "app.tools.expand", "to expand"),
                        theme_fg(&self.theme, "muted", ")")
                    ));
                }
            }
            match self.status {
                BashStatus::Cancelled => {
                    status_parts.push(theme_fg(&self.theme, "warning", "(cancelled)"));
                }
                BashStatus::Error => {
                    status_parts.push(theme_fg(
                        &self.theme,
                        "error",
                        &format!("(exit {})", self.exit_code.unwrap_or_default()),
                    ));
                }
                _ => {}
            }
            let was_truncated = self.truncation_result.as_ref().is_some_and(|r| r.truncated)
                || context_truncation.truncated;
            if was_truncated {
                if let Some(path) = &self.full_output_path {
                    status_parts.push(theme_fg(
                        &self.theme,
                        "warning",
                        &format!("Output truncated. Full output: {path}"),
                    ));
                }
            }
            if !status_parts.is_empty() {
                children.push(ChildLine::Text(ChildText {
                    text: format!("\n{}", status_parts.join("\n")),
                    padding_x: 1,
                }));
            }
        }
        children
    }

    /// The deterministic content payload (mirrors the rebuilt container's
    /// children; the shell mounts them into its frame).
    pub fn content_payload(&mut self) -> Vec<ChildLine> {
        self.update_display()
    }

    /// Upstream `getOutput`.
    pub fn get_output(&self) -> String {
        self.output_lines.join("\n")
    }

    /// Upstream `getCommand`.
    pub fn get_command(&self) -> &str {
        &self.command
    }

    pub fn loader(&mut self) -> &mut Loader {
        &mut self.loader
    }

    pub fn exclude_from_context(&self) -> bool {
        self.exclude_from_context
    }
}

impl Component for BashExecutionComponent {
    /// The full frame: spacer / top border / content children / bottom border.
    fn render(&mut self, width: usize) -> Vec<String> {
        let color_key: &'static str = if self.exclude_from_context {
            "dim"
        } else {
            "bashMode"
        };
        let border = theme_fg(&self.theme, color_key, &"\u{2500}".repeat(width.max(1)));
        let mut lines = vec![String::new(), border.clone()];
        let payload = self.update_display();
        for child in payload {
            match child {
                ChildLine::Text(text) => {
                    let mut text_component =
                        Text::with_options(&text.text, text.padding_x, 0, None);
                    lines.extend(text_component.render(width));
                }
                ChildLine::Preview(preview) => {
                    lines.extend(preview.render(width));
                }
                ChildLine::Loader => {
                    lines.extend(self.loader.render(width));
                }
            }
        }
        lines.push(border);
        lines
    }

    fn invalidate(&mut self) {
        self.update_display();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `bash_execution` —
    /// output accumulation, preview lines, status lines, expansion.
    #[test]
    fn bash_execution_matches_oracle() {
        let theme = dark();
        let mut comp = BashExecutionComponent::new(theme.clone(), "echo hi", false);
        comp.append_output("first\nsec");
        comp.append_output("ond\nthird\r\nfourth\rfifth");
        assert_eq!(comp.get_output(), "first\nsecond\nthird\nfourth\nfifth");

        let payload = comp.content_payload();
        // header + preview + loader
        assert!(matches!(&payload[0], ChildLine::Text(_)));
        assert!(matches!(&payload[1], ChildLine::Preview(_)));
        assert!(matches!(payload[2], ChildLine::Loader));
        let ChildLine::Preview(preview) = &payload[1] else {
            panic!("preview expected");
        };
        let preview_lines = preview.render(40);
        assert!(preview_lines.len() <= PREVIEW_LINES + 1);
        // the "\n" prefix wraps to a padding-only first row (upstream Text render)
        assert!(
            preview_lines[0].trim().is_empty(),
            "leading blank from the \\n prefix"
        );

        comp.set_expanded(true);
        let payload = comp.content_payload();
        let ChildLine::Text(text) = &payload[1] else {
            panic!("expanded text expected");
        };
        assert!(text.text.starts_with('\n'));
        assert!(text.text.contains("second"));

        comp.set_complete(Some(0), false, None, None);
        let payload = comp.content_payload();
        assert!(
            matches!(payload.last(), Some(ChildLine::Text(_))),
            "status line"
        );

        comp.set_complete(Some(3), false, None, Some("/tmp/full.out"));
        let payload = comp.content_payload();
        let Some(ChildLine::Text(status)) = payload.last() else {
            panic!("status line expected");
        };
        assert!(status.text.contains("(exit 3)"));

        comp.set_complete(
            None,
            true,
            Some(BashTruncationResult { truncated: true }),
            Some("/tmp/full2.out"),
        );
        let payload = comp.content_payload();
        let Some(ChildLine::Text(status)) = payload.last() else {
            panic!("status line expected");
        };
        assert!(status.text.contains("(cancelled)"));
        assert!(status
            .text
            .contains("Output truncated. Full output: /tmp/full2.out"));

        assert_eq!(comp.get_command(), "echo hi");
    }

    /// Context truncation with the default limits (oracle `big*` rows).
    #[test]
    fn long_output_truncates_like_the_bash_tool() {
        let theme = dark();
        let mut big = BashExecutionComponent::new(theme, "big", false);
        let long_output = (0..DEFAULT_MAX_LINES + 10)
            .map(|i| format!("L{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        big.append_output(&long_output);
        big.set_complete(Some(0), false, None, Some("/tmp/big.out"));
        assert_eq!(
            big.get_output().split('\n').count(),
            (DEFAULT_MAX_LINES + 10) as usize
        );
        let payload = big.content_payload();
        let preview = payload.iter().find_map(|p| match p {
            ChildLine::Preview(preview) => Some(preview.clone()),
            _ => None,
        });
        let preview = preview.expect("collapsed preview");
        let lines = preview.render(40);
        // 2001 visual lines (leading blank + 2000) capped to the last 20 — the
        // blank row is the first to be truncated (upstream slice(-maxLines)).
        assert_eq!(
            lines.len(),
            PREVIEW_LINES,
            "preview caps at 20 visual lines"
        );
    }

    /// The dim border color for `!!`-prefixed (context-excluded) commands.
    #[test]
    fn excluded_commands_use_dim_border() {
        let theme = dark();
        let mut excluded = BashExecutionComponent::new(theme, "!! ls", true);
        assert!(excluded.exclude_from_context());
        let rendered = excluded.render(20);
        assert!(rendered[1].contains("\x1b[38;2;126;136;142m"));
    }

    #[test]
    fn truncate_and_strip_helpers_match_oracle_probes() {
        let result = truncate_tail(
            "a\nb\nc",
            TruncationOptions {
                max_lines: Some(2),
                max_bytes: Some(100),
            },
        );
        assert_eq!(result.content, "b\nc");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m plain"), "red plain");
    }
}
