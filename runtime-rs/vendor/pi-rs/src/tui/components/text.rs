//! Port of upstream `packages/tui/src/components/text.ts`: a component that
//! displays multi-line text with word wrapping, horizontal padding, and an
//! optional background painter.

use crate::tui::component::Component;
use crate::tui::utils::{apply_background_to_line, visible_width, wrap_text_with_ansi};

/// Background painter (upstream `customBgFn`).
pub type BgFn = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Upstream `Text`.
#[derive(Clone, Default)]
pub struct Text {
    text: String,
    padding_x: usize,
    padding_y: usize,
    custom_bg_fn: Option<BgFn>,

    // Cache for rendered output.
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Text {
    pub fn new(text: &str) -> Self {
        Self::with_options(text, 1, 1, None)
    }

    pub fn with_options(
        text: &str,
        padding_x: usize,
        padding_y: usize,
        custom_bg_fn: Option<BgFn>,
    ) -> Self {
        Self {
            text: text.to_string(),
            padding_x,
            padding_y,
            custom_bg_fn,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.invalidate();
    }

    pub fn set_custom_bg_fn(&mut self, custom_bg_fn: Option<BgFn>) {
        self.custom_bg_fn = custom_bg_fn;
        self.invalidate();
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Check cache.
        if let (Some(lines), Some(cached_text), Some(cached_width)) =
            (&self.cached_lines, &self.cached_text, self.cached_width)
        {
            if *cached_text == self.text && cached_width == width {
                return lines.clone();
            }
        }

        // Don't render anything if there's no actual text.
        if self.text.is_empty() || self.text.trim().is_empty() {
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(Vec::new());
            return Vec::new();
        }

        // Replace tabs with 3 spaces.
        let normalized_text = self.text.replace('\t', "   ");

        // Reduce margins when necessary so content and padding fit.
        let padding_x = self.padding_x.min(width.saturating_sub(1) / 2);
        let content_width = (width - padding_x * 2).max(1);

        // Wrap text (preserves ANSI codes, does NOT pad).
        let wrapped_lines = wrap_text_with_ansi(&normalized_text, content_width);

        let left_margin = " ".repeat(padding_x);
        let right_margin = " ".repeat(padding_x);
        let mut content_lines: Vec<String> = Vec::new();

        for line in wrapped_lines {
            let line_with_margins = format!("{left_margin}{line}{right_margin}");
            match &self.custom_bg_fn {
                Some(bg) => {
                    let bg = bg.clone();
                    content_lines.push(apply_background_to_line(
                        &line_with_margins,
                        width,
                        move |text| bg(text),
                    ));
                }
                None => {
                    let visible_len = visible_width(&line_with_margins);
                    let padding_needed = width.saturating_sub(visible_len);
                    content_lines
                        .push(format!("{line_with_margins}{}", " ".repeat(padding_needed)));
                }
            }
        }

        // Top/bottom padding (empty lines).
        let empty_line = " ".repeat(width);
        let mut empty_lines: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            let line = match &self.custom_bg_fn {
                Some(bg) => {
                    let bg = bg.clone();
                    apply_background_to_line(&empty_line, width, move |text| bg(text))
                }
                None => empty_line.clone(),
            };
            empty_lines.push(line);
        }

        let mut result = std::mem::take(&mut empty_lines);
        result.extend(content_lines);
        result.extend(empty_lines);

        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(result.clone());

        result
    }

    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}
