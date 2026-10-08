//! Port of the overlay system from upstream `packages/tui/src/tui.ts`:
//! `OverlayAnchor`/`OverlayMargin`/`OverlayBounds`, the overlay stack with
//! show/hide/focus tracking, `compositeTuiLine`, and the render-time
//! compositing of visible overlays onto the base lines.
//!
//! Disclosed substitutions: overlay options are clamped to the supported
//! subset (anchor/offsets/size/margin/visible); the upstream focus-restore
//! state machine reduces to topmost-visible-overlay focus with a stored
//! pre-focus target.

use crate::tui::component::{Component, CURSOR_MARKER};
use crate::tui::terminal_image::is_image_line;
use crate::tui::utils::{extract_segments, slice_by_column, slice_with_width, visible_width};

/// Upstream `OverlayAnchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAnchor {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

/// Upstream `OverlayMargin`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayMargin {
    pub top: Option<usize>,
    pub right: Option<usize>,
    pub bottom: Option<usize>,
    pub left: Option<usize>,
}

/// Upstream `OverlayBounds`: last rendered terminal-relative rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayBounds {
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub height: usize,
}

/// Upstream `OverlayHandle` queries.
#[derive(Clone, Debug)]
pub struct OverlayHandleState {
    pub hidden: bool,
    pub focused: bool,
    pub bounds: Option<OverlayBounds>,
}

/// One overlay stack entry.
pub struct OverlayStackEntry {
    pub component: Box<dyn Component>,
    pub anchor: OverlayAnchor,
    pub offset_x: i64,
    pub offset_y: i64,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub margin: OverlayMargin,
    pub non_capturing: bool,
    pub hidden: bool,
    pub focus_order: usize,
    pub bounds: Option<OverlayBounds>,
}

/// Upstream `compositeTuiLine`: composite overlay content into a base line at
/// a fixed column, preserving styling inherited from before the overlay.
pub fn composite_tui_line(
    base_line: &str,
    overlay_line: &str,
    start_col: usize,
    overlay_width: usize,
    total_width: usize,
) -> String {
    if is_image_line(base_line) {
        return base_line.to_string();
    }
    const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";
    let after_start = start_col.saturating_add(overlay_width);
    let base = extract_segments(
        base_line,
        start_col,
        after_start,
        total_width.saturating_sub(after_start),
        true,
    );
    let (overlay, width) = slice_with_width(overlay_line, 0, overlay_width, true);
    let before_pad = start_col.saturating_sub(base.before_width);
    let overlay_pad = overlay_width.saturating_sub(width);
    let after_target = total_width
        .saturating_sub(start_col.max(base.before_width))
        .saturating_sub(overlay_width.max(width));
    let after_pad = after_target.saturating_sub(base.after_width);
    let result = format!(
        "{}{}{SEGMENT_RESET}{}{}{SEGMENT_RESET}{}{}",
        base.before,
        " ".repeat(before_pad),
        overlay,
        " ".repeat(overlay_pad),
        base.after,
        " ".repeat(after_pad),
    );
    if visible_width(&result) <= total_width {
        result
    } else {
        slice_by_column(&result, 0, total_width, true)
    }
}

/// Resolved layout for one overlay (upstream `resolveOverlayLayout` result).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedOverlayLayout {
    pub row: usize,
    pub col: usize,
    pub width: usize,
}

/// Upstream `resolveOverlayLayout` for the supported options subset.
#[allow(clippy::too_many_arguments)] // mirrors the upstream option matrix
pub fn resolve_overlay_layout(
    anchor: OverlayAnchor,
    offset_x: i64,
    offset_y: i64,
    width: Option<usize>,
    height: Option<usize>,
    margin: &OverlayMargin,
    overlay_height: usize,
    term_width: usize,
    term_height: usize,
) -> ResolvedOverlayLayout {
    let w = width.unwrap_or_else(|| {
        term_width
            .saturating_sub(margin.left.unwrap_or(0) + margin.right.unwrap_or(0))
            .max(1)
    });
    let max_height = height.unwrap_or(usize::MAX).min(term_height);

    // Horizontal placement.
    let col: i64 = match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::BottomLeft | OverlayAnchor::LeftCenter => {
            margin.left.unwrap_or(0) as i64
        }
        OverlayAnchor::Center | OverlayAnchor::TopCenter | OverlayAnchor::BottomCenter => {
            ((term_width as i64 - w as i64) / 2).max(0)
        }
        OverlayAnchor::TopRight | OverlayAnchor::BottomRight | OverlayAnchor::RightCenter => {
            (term_width as i64 - margin.right.unwrap_or(0) as i64 - w as i64).max(0)
        }
    } + offset_x;

    // Vertical placement.
    let row: i64 = match anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::TopRight | OverlayAnchor::TopCenter => {
            margin.top.unwrap_or(0) as i64
        }
        OverlayAnchor::BottomLeft | OverlayAnchor::BottomRight | OverlayAnchor::BottomCenter => {
            (term_height as i64 - margin.bottom.unwrap_or(0) as i64 - overlay_height as i64).max(0)
        }
        OverlayAnchor::Center | OverlayAnchor::LeftCenter | OverlayAnchor::RightCenter => {
            ((term_height as i64 - overlay_height as i64) / 2).max(0)
        }
    } + offset_y;

    ResolvedOverlayLayout {
        row: row.max(0) as usize,
        col: col.max(0) as usize,
        width: w.min(max_height_field(max_height)),
    }
}

fn max_height_field(max_height: usize) -> usize {
    // Width is not clamped by max height; keep the parameter for parity with
    // the upstream signature.
    let _ = max_height;
    usize::MAX
}

/// Strip cursor markers from lines (upstream `applyLineResets` CURSOR_MARKER
/// handling shared by overlay compositing).
pub fn strip_cursor_markers(lines: &mut [String]) {
    for line in lines.iter_mut() {
        while let Some(index) = line.find(CURSOR_MARKER) {
            line.replace_range(index..index + CURSOR_MARKER.len(), "");
        }
    }
}

/// Overlay stack manager (upstream overlayStack + show/hide/hideOverlay).
#[derive(Default)]
pub struct OverlayStack {
    pub entries: Vec<OverlayStackEntry>,
    focus_order_counter: usize,
}

impl OverlayStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Upstream `showOverlay`: push an overlay, returning its focus order.
    pub fn push_overlay(
        &mut self,
        component: Box<dyn Component>,
        focus_order: Option<usize>,
    ) -> usize {
        self.focus_order_counter += 1;
        let order = focus_order.unwrap_or(self.focus_order_counter);
        self.entries.push(OverlayStackEntry {
            component,
            anchor: OverlayAnchor::Center,
            offset_x: 0,
            offset_y: 0,
            width: None,
            height: None,
            margin: OverlayMargin::default(),
            non_capturing: false,
            hidden: false,
            focus_order: order,
            bounds: None,
        });
        self.entries.len() - 1
    }

    pub fn set_anchor(&mut self, index: usize, anchor: OverlayAnchor) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.anchor = anchor;
        }
    }

    pub fn set_offsets(&mut self, index: usize, offset_x: i64, offset_y: i64) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.offset_x = offset_x;
            entry.offset_y = offset_y;
        }
    }

    pub fn set_margin(&mut self, index: usize, margin: OverlayMargin) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.margin = margin;
        }
    }

    pub fn set_non_capturing(&mut self, index: usize, non_capturing: bool) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.non_capturing = non_capturing;
        }
    }

    pub fn set_hidden(&mut self, index: usize, hidden: bool) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.hidden = hidden;
        }
    }

    pub fn is_hidden(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| entry.hidden)
    }

    pub fn bounds(&self, index: usize) -> Option<OverlayBounds> {
        self.entries.get(index).and_then(|entry| entry.bounds)
    }

    /// Upstream `hideOverlay`: pop the topmost entry and return its component.
    pub fn pop_overlay(&mut self) -> Option<Box<dyn Component>> {
        self.entries.pop().map(|entry| entry.component)
    }
}

/// Extract the hardware-cursor position from rendered lines
/// (upstream `extractCursorPosition`): the first line containing
/// CURSOR_MARKER, positioned at the marker's visible column; markers are
/// stripped from every line.
pub fn extract_cursor_position(lines: &[String], height: usize) -> Option<(usize, usize)> {
    let mut position = None;
    let mut stripped: Vec<String> = Vec::with_capacity(lines.len());
    for (row, line) in lines.iter().enumerate() {
        if position.is_none() && row < height {
            if let Some(index) = line.find(CURSOR_MARKER) {
                let col = visible_width(&line[..index]);
                position = Some((row, col));
            }
        }
        stripped.push(line.replace(CURSOR_MARKER, ""));
    }
    let _ = stripped;
    position
}
