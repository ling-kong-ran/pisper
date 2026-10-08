//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Shared support pieces for the r19 component small-pieces slice.
//!
//! - [`MessageBgBox`]: faithful port of upstream `packages/tui` `Box`
//!   (`box.ts`) — padding + optional background applied to every line. The
//!   vendored `tui::components::layout_widgets::Box` predates the bg-fn setter
//!   its private `bg_fn` needs, so the message components here carry this
//!   local, byte-faithful twin instead (documented seam S19.1 in
//!   `components/mod.rs`).
//! - [`plain_markdown_theme`]: an all-identity [`MarkdownTheme`] used where
//!   upstream defaults to the process-global `getMarkdownTheme()` (theme
//!   seam D1); callers thread the real theme in.
//! - The OSC-133 shell-integration zone markers shared by the user/assistant
//!   message components.

use std::sync::Arc;

use crate::tui::component::Component;
use crate::tui::components::markdown::MarkdownTheme;
use crate::tui::utils::{apply_background_to_line, visible_width};

/// Upstream `OSC133_ZONE_START` (`assistant-message.ts:7`).
pub const OSC133_ZONE_START: &str = "\x1b]133;A\x07";
/// Upstream `OSC133_ZONE_END` (`assistant-message.ts:8`).
pub const OSC133_ZONE_END: &str = "\x1b]133;B\x07";
/// Upstream `OSC133_ZONE_FINAL` (`assistant-message.ts:9`).
pub const OSC133_ZONE_FINAL: &str = "\x1b]133;C\x07";

/// Upstream tui `Box` (padding + background). `paddingX`/`paddingY` default
/// to 1 like upstream's constructor.
pub struct MessageBgBox {
    children: Vec<Box<dyn Component>>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
}

impl Default for MessageBgBox {
    fn default() -> Self {
        Self::new(1, 1, None)
    }
}

impl MessageBgBox {
    pub fn new(
        padding_x: usize,
        padding_y: usize,
        bg_fn: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
    ) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn,
        }
    }

    pub fn set_bg_fn(&mut self, bg_fn: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>) {
        // Upstream detects bg changes by sampling; a direct assignment is
        // equivalent because this port caches nothing.
        self.bg_fn = bg_fn;
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        let vis_len = visible_width(line);
        let pad_needed = width.saturating_sub(vis_len);
        let padded = format!("{line}{}", " ".repeat(pad_needed));
        match &self.bg_fn {
            Some(bg) => {
                let bg = Arc::clone(bg);
                apply_background_to_line(&padded, width, move |text| bg(text))
            }
            None => padded,
        }
    }
}

impl Component for MessageBgBox {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let content_width = (width.saturating_sub(self.padding_x * 2)).max(1);
        let left_pad = " ".repeat(self.padding_x);

        let mut child_lines: Vec<String> = Vec::new();
        for child in &mut self.children {
            for line in child.render(content_width) {
                child_lines.push(format!("{left_pad}{line}"));
            }
        }
        if child_lines.is_empty() {
            return Vec::new();
        }

        let mut result: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        for line in &child_lines {
            result.push(self.apply_bg(line, width));
        }
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        result
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

/// `theme.fg(color, …)` as a Markdown default-text style (upstream passes
/// `(text) => theme.fg("customMessageText", text)` as the Markdown `color`
/// option; Utf16Text round-trips through the lossless ANSI string).
pub(crate) fn theme_text_style(
    theme: &Arc<crate::coding_agent::modes::interactive::theme::Theme>,
    color: &'static str,
) -> crate::tui::components::markdown::StyleFn {
    let theme = Arc::clone(theme);
    Arc::new(move |text: &crate::tui::utf16::Utf16Text| {
        let plain = text.to_string_lossy();
        crate::tui::utf16::Utf16Text::from(theme.fg(color, &plain).expect("theme fg color"))
    })
}

fn identity_style() -> crate::tui::components::markdown::StyleFn {
    Arc::new(|text| text.clone())
}

/// Component adapter for the `DynamicBorder` the selector slice already ports
/// (`model_selector.rs` keeps it as a plain struct with an inherent render,
/// because its frozen module predates the r19 `mod.rs` unfreeze).
pub(crate) struct Border(
    pub(crate) crate::coding_agent::modes::interactive::components::model_selector::DynamicBorder,
);

impl Border {
    pub(crate) fn new(color: Option<Box<dyn Fn(&str) -> String + Send>>) -> Self {
        Self(
            crate::coding_agent::modes::interactive::components::model_selector::DynamicBorder::new(
                color,
            ),
        )
    }
}

impl Component for Border {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.0.render(width)
    }
}

/// An all-identity [`MarkdownTheme`] (upstream `getMarkdownTheme()` seam D1).
pub fn plain_markdown_theme() -> MarkdownTheme {
    MarkdownTheme {
        heading: identity_style(),
        link: identity_style(),
        link_url: identity_style(),
        code: identity_style(),
        code_block: identity_style(),
        code_block_border: identity_style(),
        quote: identity_style(),
        quote_border: identity_style(),
        hr: identity_style(),
        list_bullet: identity_style(),
        bold: identity_style(),
        italic: identity_style(),
        strikethrough: identity_style(),
        underline: identity_style(),
        highlight_code: None,
        code_block_indent: Some("  ".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::text::Text;

    fn red_bg() -> Arc<dyn Fn(&str) -> String + Send + Sync> {
        Arc::new(|text| format!("\x1b[41m{text}\x1b[49m"))
    }

    /// Oracle: the deps.ts `Box` patch in tests/fixtures/interactive_r19_oracle is a
    /// verbatim port of upstream `box.ts` `render`/`applyBg`; this mirrors the
    /// same padding + per-cell background pipeline.
    #[test]
    fn box_padding_and_bg_match_upstream_box() {
        let mut inner = Box::new(MessageBgBox::new(1, 1, Some(red_bg())));
        inner.add_child(Box::new(Text::with_options("hi", 0, 0, None)));
        let lines = inner.render(10);
        // top pad, content (left pad + text padded to 8), bottom pad
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\x1b[41m"));
        assert!(lines[1].contains(" hi "));
        assert!(lines[2].contains("\x1b[41m"));
    }

    #[test]
    fn empty_box_renders_nothing() {
        let mut inner = Box::new(MessageBgBox::new(1, 1, None));
        assert!(inner.render(10).is_empty());
    }
}
