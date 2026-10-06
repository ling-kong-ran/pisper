//! Tests for the differential renderer core (upstream `doRender` decision
//! tree in tui-main-screen.ts) and the TuiBase input dispatch chain.

use crate::tui::renderer::{
    compute_frame, reset_render_state, FrameKind, RenderOptions, RenderState, WriteOp,
};

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn first_render_writes_every_line_without_clearing() {
    let mut state = RenderState::new();
    let frame = compute_frame(
        &mut state,
        &RenderOptions::default(),
        lines(&["hello", "world"]),
        80,
        24,
    );
    assert_eq!(frame.kind, FrameKind::FirstRender);
    assert!(!frame.ops.contains(&WriteOp::ClearScreen));
    assert!(frame.ops.contains(&WriteOp::WriteLine("hello".to_string())));
    assert!(frame.ops.contains(&WriteOp::WriteLine("world".to_string())));
    assert_eq!(state.full_redraws, 1);
}

#[test]
fn unchanged_content_produces_no_change_frame() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a", "b"]), 80, 24);
    let frame = compute_frame(&mut state, &options, lines(&["a", "b"]), 80, 24);
    assert_eq!(frame.kind, FrameKind::NoChange);
    assert!(frame.ops.is_empty());
}

#[test]
fn partial_frame_repaints_only_the_changed_range() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a", "b", "c"]), 80, 24);

    let frame = compute_frame(&mut state, &options, lines(&["a", "B", "c"]), 80, 24);
    assert_eq!(
        frame.kind,
        FrameKind::Partial {
            first_changed: 1,
            last_changed: 1
        }
    );
    let written: Vec<&String> = frame
        .ops
        .iter()
        .filter_map(|op| match op {
            WriteOp::WriteLine(line) => Some(line),
            _ => None,
        })
        .collect();
    assert_eq!(written, vec!["B"]);
}

#[test]
fn appended_lines_repaint_from_the_previous_end() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a"]), 80, 24);

    let frame = compute_frame(&mut state, &options, lines(&["a", "b", "c"]), 80, 24);
    assert_eq!(
        frame.kind,
        FrameKind::Partial {
            first_changed: 1,
            last_changed: 2
        }
    );
    let written: Vec<&String> = frame
        .ops
        .iter()
        .filter_map(|op| match op {
            WriteOp::WriteLine(line) => Some(line),
            _ => None,
        })
        .collect();
    assert_eq!(written, vec!["b", "c"]);
}

#[test]
fn width_change_triggers_full_clear_rerender() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a"]), 80, 24);
    let before_redraws = state.full_redraws;

    let frame = compute_frame(&mut state, &options, lines(&["a"]), 40, 24);
    assert_eq!(frame.kind, FrameKind::FullRender { clear_screen: true });
    assert!(frame.ops.contains(&WriteOp::ClearScreen));
    assert_eq!(state.full_redraws, before_redraws + 1);
}

#[test]
fn height_change_triggers_full_clear_rerender() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a"]), 80, 24);

    let frame = compute_frame(&mut state, &options, lines(&["a"]), 80, 30);
    assert_eq!(frame.kind, FrameKind::FullRender { clear_screen: true });
}

#[test]
fn clear_on_shrink_full_renders_when_content_shrinks() {
    let mut state = RenderState::new();
    let mut options = RenderOptions {
        clear_on_shrink: true,
    };
    compute_frame(&mut state, &options, lines(&["a", "b", "c"]), 80, 24);

    let frame = compute_frame(&mut state, &options, lines(&["a"]), 80, 24);
    assert_eq!(frame.kind, FrameKind::FullRender { clear_screen: true });
    options.clear_on_shrink = false;
    let _ = options;
}

#[test]
fn reset_render_state_clears_tracking() {
    let mut state = RenderState::new();
    let options = RenderOptions::default();
    compute_frame(&mut state, &options, lines(&["a"]), 80, 24);
    reset_render_state(&mut state);
    assert!(state.previous_lines.is_empty());
    assert_eq!(state.previous_width, 0);

    // After reset, the next frame is a first render again.
    let frame = compute_frame(&mut state, &options, lines(&["a"]), 80, 24);
    assert_eq!(frame.kind, FrameKind::FirstRender);
}
