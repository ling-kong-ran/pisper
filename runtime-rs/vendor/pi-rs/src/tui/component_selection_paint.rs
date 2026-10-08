//! Fullscreen selection painting from `tui-alt-screen.ts:1553–1617`.
//!
//! This is a pure stage, not the complete fullscreen compositor. Its caller
//! supplies the screen after search/indicator/overlay composition and the frame
//! used for that screen; flashes, cursor extraction and line resets follow it.
//! All scroll identities are owning handles. Projection stays signed until
//! after clipping: a content endpoint may be above/left of the visible screen.
use crate::tui::component_selection::{ComponentSelection, SelectionRange};
use crate::tui::layout::{get_scroll_view_box, LayoutFrame};
use crate::tui::terminal_image::is_image_line;
use crate::tui::utils::{
    extract_ansi_code, get_grapheme_cell_range, slice_by_column, visible_width,
};

/// Reassert inverse after every recognized ANSI token ending in `m`, including
/// resets and inverse-off. Do not normalize, merge, or discard the original
/// tokens. As with the existing text utilities, input is valid Rust UTF-8.
pub fn apply_selection_highlight(text: &str) -> String {
    let mut result = String::from("\x1b[7m");
    let mut index = 0;
    while index < text.len() {
        if let Some(ansi) = extract_ansi_code(text, index) {
            result.push_str(ansi);
            if ansi.ends_with('m') {
                result.push_str("\x1b[7m");
            }
            index += ansi.len();
        } else {
            let ch = text[index..].chars().next().expect("index is before end");
            result.push(ch);
            index += ch.len_utf8();
        }
    }
    result.push_str("\x1b[27m");
    result
}

#[derive(Clone, Copy)]
struct ScreenPoint {
    row: i128,
    col: i128,
    boundary: bool,
}

// Unlike content selection_columns, projected points can be negative. Keep
// the exact upstream endpoint fallback before applying the final min/max clip.
fn columns(
    line: &str,
    row: i128,
    first: ScreenPoint,
    last: ScreenPoint,
    min: i128,
    max: i128,
) -> (i128, i128) {
    let width = visible_width(line) as i128;
    let cell = |col| {
        usize::try_from(col)
            .ok()
            .and_then(|col| get_grapheme_cell_range(line, col))
    };
    let mut start = min.max(0);
    let mut end = width.min(max);
    if row == first.row {
        start = cell(first.col).map_or(first.col.min(width), |r| r.0 as i128);
    }
    if row == last.row {
        end = if last.boundary {
            last.col.min(width)
        } else {
            cell(last.col).map_or((last.col + 1).min(width), |r| r.1 as i128)
        };
    }
    (start.max(min), end.min(max))
}

/// Paint already-normalized bounds (normally [`ComponentSelection::bounds`]).
/// `None`, a missing scroll frame/box, empty clipped ranges and image lines are
/// returned byte-for-byte unchanged. Does not mutate input, selection, frame,
/// scroll state or timers. JS array identity is not exposed by this Vec API.
///
/// Internal i128 arithmetic keeps all i64 layout origins and usize content
/// cells signed through projection, without intermediate saturation. This is
/// the integer-cell API, not arbitrary JS number or lone-surrogate support.
pub fn apply_selection(
    screen: &[String],
    bounds: Option<&SelectionRange>,
    layout: Option<&LayoutFrame>,
    terminal_columns: usize,
) -> Vec<String> {
    let Some(selection) = bounds else {
        return screen.to_vec();
    };
    let mut first = ScreenPoint {
        row: selection.start.row as i128,
        col: selection.start.col as i128,
        boundary: selection.start.boundary,
    };
    let mut last = ScreenPoint {
        row: selection.end.row as i128,
        col: selection.end.col as i128,
        boundary: selection.end.boundary,
    };
    let mut min_row = 0;
    let mut max_row = screen.len() as i128 - 1;
    let mut min_col = 0;
    let mut max_col = terminal_columns as i128;
    if let Some(scroll) = &selection.start.scroll_view {
        let Some(b) = layout.and_then(|frame| get_scroll_view_box(frame, scroll)) else {
            return screen.to_vec();
        };
        min_row = 0.max(b.rect.y as i128).max(b.clip.y as i128);
        max_row = max_row
            .min(b.rect.y as i128 + b.rect.height as i128 - 1)
            .min(b.clip.y as i128 + b.clip.height as i128 - 1);
        min_col = 0.max(b.rect.x as i128).max(b.clip.x as i128);
        max_col = max_col
            .min(b.rect.x as i128 + b.rect.width as i128)
            .min(b.clip.x as i128 + b.clip.width as i128);
        let offset = b.rect.y as i128 - scroll.snapshot().scroll_top as i128;
        first.row += offset;
        last.row += offset;
        first.col += b.rect.x as i128;
        last.col += b.rect.x as i128;
    }
    screen
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let row = row as i128;
            if row < min_row
                || row > max_row
                || row < first.row
                || row > last.row
                || is_image_line(line)
            {
                return line.clone();
            }
            let (start, end) = columns(line, row, first, last, min_col, max_col);
            if end <= start {
                return line.clone();
            }
            // min_col >= 0, and end <= visible_width(line); the conversion is
            // made only after the signed range has passed its emptiness check.
            let (start, end) = (start as usize, end as usize);
            let before = slice_by_column(line, 0, start, true);
            let selected = slice_by_column(line, start, end - start, true);
            let after = slice_by_column(line, end, visible_width(line).saturating_sub(end), true);
            format!("{before}{}{after}", apply_selection_highlight(&selected))
        })
        .collect()
}

impl ComponentSelection {
    /// Paint this controller's current normalized bounds without consuming or
    /// changing the controller. Pass the actual frame for the supplied screen.
    pub fn apply_selection(
        &self,
        screen: &[String],
        layout: Option<&LayoutFrame>,
        terminal_columns: usize,
    ) -> Vec<String> {
        apply_selection(screen, self.bounds().as_ref(), layout, terminal_columns)
    }
}
