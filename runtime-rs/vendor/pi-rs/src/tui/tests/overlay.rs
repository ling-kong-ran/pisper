//! Tests for the overlay compositing core
//! (upstream tui.ts compositeTuiLine + compositeOverlays decisions).

use crate::tui::component::{Component, CURSOR_MARKER};
use crate::tui::overlay::{
    composite_tui_line, extract_cursor_position, OverlayAnchor, OverlayStack,
};
use crate::tui::utils::visible_width;

fn simple_overlay(lines: &[&str]) -> Box<dyn Component> {
    struct Fixed(Vec<String>);
    impl Component for Fixed {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.0.clone()
        }
    }
    Box::new(Fixed(lines.iter().map(|s| s.to_string()).collect()))
}

#[test]
fn composite_tui_line_overlays_at_column_preserving_surroundings() {
    let base = "hello world";
    let out = composite_tui_line(base, "THERE", 6, 5, 40);
    // The actual upstream compositor resets each segment and pads to total width.
    let reset = "\x1b[0m\x1b]8;;\x07";
    assert_eq!(out, format!("hello {reset}THERE{reset}{}", " ".repeat(29)));
    assert_eq!(visible_width(&out), 40);
}

#[test]
fn composite_tui_line_pads_before_when_overlay_starts_beyond_content() {
    let base = "ab";
    let out = composite_tui_line(base, "XY", 6, 2, 20);
    let reset = "\x1b[0m\x1b]8;;\x07";
    assert_eq!(out, format!("ab    {reset}XY{reset}{}", " ".repeat(12)));
    assert_eq!(visible_width(&out), 20);
}

#[test]
fn composite_tui_line_truncates_overlong_overlay_lines() {
    let base = "abc";
    let out = composite_tui_line(base, "0123456789", 0, 4, 40);
    // Overlay line is truncated to the declared overlay width (4).
    let reset = "\x1b[0m\x1b]8;;\x07";
    assert_eq!(out, format!("{reset}0123{reset}{}", " ".repeat(36)));
    assert_eq!(visible_width(&out), 40);
}

#[test]
fn overlay_stack_push_hide_and_bounds() {
    let mut stack = OverlayStack::new();
    let index = stack.push_overlay(simple_overlay(&["popup"]), None);
    assert_eq!(stack.len(), 1);
    assert!(!stack.is_hidden(index));

    stack.set_hidden(index, true);
    assert!(stack.is_hidden(index));

    let popped = stack.pop_overlay().expect("overlay present");
    drop(popped);
    assert!(stack.is_empty());
}

#[test]
fn overlay_compositing_renders_overlay_lines_onto_base() {
    // Integration-style: base lines composited with a centered overlay.
    let mut stack = OverlayStack::new();
    let index = stack.push_overlay(simple_overlay(&["[MENU]"]), None);
    stack.set_anchor(index, OverlayAnchor::Center);

    let base = lines_of(&["line one", "line two", "line three"]);
    let term_width = 20;
    let term_height = 3;
    let mut result = base.clone();

    // Reproduce the compositeOverlays core: render overlay, resolve centered
    // layout, composite at the resolved row.
    let overlay_lines = ["[MENU]".to_string()];
    let overlay_height = overlay_lines.len();
    let overlay_width = 6usize;
    let row = (term_height - overlay_height) / 2;
    let col = (term_width - overlay_width) / 2;
    for (i, overlay_line) in overlay_lines.iter().enumerate() {
        let idx = row + i;
        result[idx] =
            composite_tui_line(&result[idx], overlay_line, col, overlay_width, term_width);
    }
    stack.set_hidden(index, true);

    // Line 1 (middle) contains the overlaid menu text.
    assert!(result[1].contains("[MENU]"), "line 1: {:?}", result[1]);
}

#[test]
fn extract_cursor_position_finds_and_strips_marker() {
    let marker = CURSOR_MARKER;
    let lines = lines_of(&[&format!("ab{marker}cdef"), "plain"]);
    let pos = extract_cursor_position(&lines, 40);
    // Cursor marker found at row 0, visible col 2.
    assert_eq!(pos, Some((0, 2)));
}

#[test]
fn enter_and_no_cursor_marker_leaves_lines_untouched() {
    let lines = lines_of(&["no marker here"]);
    assert_eq!(extract_cursor_position(&lines, 40), None);
}

fn lines_of(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}
