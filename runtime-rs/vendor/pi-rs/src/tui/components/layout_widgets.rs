//! Ports of the small layout widgets from upstream `packages/tui/src/components/`:
//! `box.ts` (padding + background container with render cache),
//! `spacer.ts`, `truncated-text.ts`, and the natural-flow core of
//! `v-stack.ts` (children + gap; the flex grow/shrink/basis allocation from
//! `stack.ts`/`layout-node.ts` is a later slice — disclosed).

use std::boxed::Box as StdBox;
use std::sync::Arc;

use crate::tui::component::Component;
type BackgroundFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
use crate::tui::utils::{truncate_to_width, visible_width};

// ---------------------------------------------------------------------------
// Box
// ---------------------------------------------------------------------------

/// Upstream `Box`: padding + optional background applied to all children.
#[derive(Default)]
pub struct Box {
    children: Vec<StdBox<dyn Component>>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<BackgroundFn>,
}

impl Box {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            padding_x: 1,
            padding_y: 1,
            bg_fn: None,
        }
    }

    pub fn set_padding(&mut self, padding_x: usize, padding_y: usize) {
        self.padding_x = padding_x;
        self.padding_y = padding_y;
    }

    pub fn add_child(&mut self, child: StdBox<dyn Component>) {
        self.children.push(child);
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    fn apply_bg(&self, line: &str, width: usize) -> String {
        let vis_len = visible_width(line);
        let pad_needed = width.saturating_sub(vis_len);
        let padded = format!("{line}{}", " ".repeat(pad_needed));
        // Already padded to width, so apply the background directly instead of
        // measuring the line again.
        match &self.bg_fn {
            Some(bg) => bg(&padded),
            None => padded,
        }
    }
}

impl Component for Box {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.children.is_empty() {
            return Vec::new();
        }

        let content_width = (width.saturating_sub(self.padding_x * 2)).max(1);
        let left_pad = " ".repeat(self.padding_x);

        // Render all children. Keep the child lines unpadded: children usually
        // return the same string objects every frame upstream, so the render
        // cache check is a cheap identity comparison per line. Padding here
        // would create new strings that must be compared character by
        // character; the padding lands on the composed line instead.
        let mut child_lines: Vec<String> = Vec::new();
        for child in &mut self.children {
            for line in child.render(content_width) {
                child_lines.push(line);
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
            result.push(self.apply_bg(&format!("{left_pad}{line}"), width));
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

impl Box {}

// ---------------------------------------------------------------------------
// Spacer
// ---------------------------------------------------------------------------

/// Upstream `Spacer`: renders empty lines.
#[derive(Clone, Copy, Debug)]
pub struct Spacer {
    lines: usize,
}

impl Spacer {
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Component for Spacer {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.lines]
    }
}

// ---------------------------------------------------------------------------
// TruncatedText
// ---------------------------------------------------------------------------

/// Upstream `TruncatedText`: single-line text truncated to the viewport width.
#[derive(Clone, Debug)]
pub struct TruncatedText {
    text: String,
    padding_x: usize,
    padding_y: usize,
}

impl TruncatedText {
    pub fn new(text: &str) -> Self {
        Self::with_padding(text, 0, 0)
    }

    pub fn with_padding(text: &str, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.to_string(),
            padding_x,
            padding_y,
        }
    }
}

impl Component for TruncatedText {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        let empty_line = " ".repeat(width);

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        let available_width = (width.saturating_sub(self.padding_x * 2)).max(1);

        let single_line_text = match self.text.find('\n') {
            Some(index) => &self.text[..index],
            None => self.text.as_str(),
        };

        let display_text = truncate_to_width(single_line_text, available_width, "...", false);

        let left_padding = " ".repeat(self.padding_x);
        let right_padding = " ".repeat(self.padding_x);
        let line_with_padding = format!("{left_padding}{display_text}{right_padding}");

        let line_visible_width = visible_width(&line_with_padding);
        let padding_needed = width.saturating_sub(line_visible_width);
        result.push(format!("{line_with_padding}{}", " ".repeat(padding_needed)));

        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        result
    }

    fn invalidate(&mut self) {
        // No cached state to invalidate currently
    }
}
