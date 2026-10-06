//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/visual-truncate.ts` (v1.0.0):
//! width-aware truncation shared by tool renderers and bash-execution, plus
//! the [`VisualLinePreview`] collapsed-output component.

use std::sync::Arc;

use crate::tui::component::Component;
use crate::tui::components::text::Text;

/// Upstream `VisualTruncateResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualTruncateResult {
    /// The visual lines to display.
    pub visual_lines: Vec<String>,
    /// Number of visual lines that were skipped (hidden).
    pub skipped_count: usize,
}

/// Truncate text to a maximum number of visual lines, accounting for line
/// wrapping at the terminal width (upstream `truncateToVisualLines`).
///
/// `padding_x` mirrors the upstream `paddingX` Text parameter: `0` when the
/// result will be placed in a Box (the Box adds its own padding), `1` for a
/// plain Container. `keep` selects which visual lines survive: the last ones
/// (the default) or the first ones (v1.0.0).
pub fn truncate_to_visual_lines(
    text: &str,
    max_visual_lines: usize,
    width: usize,
    padding_x: usize,
    keep: Keep,
) -> VisualTruncateResult {
    if text.is_empty() {
        return VisualTruncateResult {
            visual_lines: Vec::new(),
            skipped_count: 0,
        };
    }

    // Temporary Text component to render and get visual lines.
    let mut temp_text = Text::with_options(text, padding_x, 0, None);
    let mut all_visual_lines = temp_text.render(width);

    if all_visual_lines.len() <= max_visual_lines {
        return VisualTruncateResult {
            skipped_count: 0,
            visual_lines: all_visual_lines,
        };
    }

    let skipped_count = all_visual_lines.len() - max_visual_lines;
    let visual_lines = match keep {
        Keep::End => all_visual_lines.split_off(skipped_count),
        Keep::Start => {
            all_visual_lines.truncate(max_visual_lines);
            all_visual_lines
        }
    };
    VisualTruncateResult {
        visual_lines,
        skipped_count,
    }
}

/// Which visual lines [`truncate_to_visual_lines`] keeps (upstream the
/// `keep` parameter, v1.0.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Keep {
    /// The last `max_visual_lines` lines (the previous behavior).
    #[default]
    End,
    /// The first `max_visual_lines` lines.
    Start,
}

/// Options of [`VisualLinePreview`] (upstream `VisualLinePreviewOptions`).
pub struct VisualLinePreviewOptions {
    /// Styled text; may contain newlines.
    pub text: String,
    pub max_visual_lines: usize,
    /// Which visual lines to keep. The hint goes before kept end lines and
    /// after kept start lines.
    pub keep: Keep,
    /// Styled hint line for the given number of hidden visual lines.
    pub format_hint: Arc<dyn Fn(usize) -> String + Send + Sync>,
}

/// Collapsed tool output limited to a number of visual lines, like bash
/// output (v1.0.0 `VisualLinePreview`). Limiting logical lines instead lets a
/// single long line (such as minified JSON) wrap across the whole screen.
/// Caches its lines per width, since it renders on every frame for every
/// result in the transcript.
pub struct VisualLinePreview {
    options: VisualLinePreviewOptions,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl VisualLinePreview {
    pub fn new(options: VisualLinePreviewOptions) -> Self {
        VisualLinePreview {
            options,
            cached_width: None,
            cached_lines: None,
        }
    }
}

impl Component for VisualLinePreview {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.cached_lines.is_none() || self.cached_width != Some(width) {
            let preview = truncate_to_visual_lines(
                &self.options.text,
                self.options.max_visual_lines,
                width,
                0,
                self.options.keep,
            );
            let mut lines = preview.visual_lines;
            if preview.skipped_count > 0 {
                let hint = crate::tui::utils::truncate_to_width(
                    &(self.options.format_hint)(preview.skipped_count),
                    width,
                    "...",
                    true,
                );
                match self.options.keep {
                    Keep::Start => lines.push(hint),
                    Keep::End => lines.insert(0, hint),
                }
            }
            self.cached_lines = Some(lines);
            self.cached_width = Some(width);
        }
        self.cached_lines.clone().expect("cache populated above")
    }

    fn invalidate(&mut self) {
        self.cached_width = None;
        self.cached_lines = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `visual_truncate` —
    /// byte-identical against the real upstream `Text` wrapping.
    #[test]
    fn matches_oracle() {
        let long = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // 12 words, exactly as in the oracle scenario
        let wrap = "word ".repeat(12).trim_end().to_string();
        let pad40 = |s: &str| format!("{s:<40}");

        let empty = truncate_to_visual_lines("", 5, 40, 0, Keep::End);
        assert_eq!(empty.visual_lines, Vec::<String>::new());
        assert_eq!(empty.skipped_count, 0);

        let abc = truncate_to_visual_lines("a\nb\nc", 5, 40, 0, Keep::End);
        assert_eq!(abc.visual_lines, vec![pad40("a"), pad40("b"), pad40("c")]);
        assert_eq!(abc.skipped_count, 0);

        let five = truncate_to_visual_lines("a\nb\nc\nd\ne", 2, 40, 0, Keep::End);
        assert_eq!(five.visual_lines, vec![pad40("d"), pad40("e")]);
        assert_eq!(five.skipped_count, 3);

        let tail = truncate_to_visual_lines(&long, 3, 40, 1, Keep::End);
        assert_eq!(
            tail.visual_lines,
            vec![pad40(" line 27"), pad40(" line 28"), pad40(" line 29")]
        );
        assert_eq!(tail.skipped_count, 27);

        let wrapped_pad1 = truncate_to_visual_lines(&wrap, 2, 20, 1, Keep::End);
        assert_eq!(
            wrapped_pad1.visual_lines,
            vec![
                format!("{:<20}", " word word word"),
                format!("{:<20}", " word word word"),
            ]
        );
        assert_eq!(wrapped_pad1.skipped_count, 2);

        // paddingX = 0 keeps lines unindented (Box provides its own padding);
        // rows are padded to the render width (20), not the 40 used above.
        let wrapped_pad0 = truncate_to_visual_lines(&wrap, 2, 20, 0, Keep::End);
        assert_eq!(
            wrapped_pad0.visual_lines,
            vec![
                "word word word word ".to_string(),
                "word word word word ".to_string()
            ]
        );
        assert_eq!(wrapped_pad0.skipped_count, 1);
    }
}
