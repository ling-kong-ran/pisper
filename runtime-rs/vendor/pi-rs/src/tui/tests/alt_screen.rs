//! Tests for the alternate-screen frame planner
//! (upstream tui-alt-screen.ts `doRender` row-diff behavior).

use crate::tui::alt_screen::{
    compute_alt_screen_frame, enter_alt_screen_sequences, exit_alt_screen_sequences,
    AltScreenState, END_SYNCHRONIZED_OUTPUT, HIDE_CURSOR, SHOW_CURSOR,
};

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn first_frame_clears_and_paints_every_row() {
    let mut state = AltScreenState::new();
    let frame = compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);

    assert!(frame.ops.contains(&"\x1b[2J".to_string()));
    // Rows are positioned 1-based and cleared before painting.
    assert!(frame.ops.contains(&"\x1b[1;1H\x1b[2Kalpha".to_string()));
    assert!(frame.ops.contains(&"\x1b[2;1H\x1b[2Kbeta".to_string()));
    assert!(frame.ops.contains(&"\x1b[3;1H\x1b[2K".to_string()));
    assert!(frame.ops.first().is_some_and(|op| op.contains("2026")));
    assert!(frame
        .ops
        .last()
        .is_some_and(|op| op == END_SYNCHRONIZED_OUTPUT));
}

#[test]
fn unchanged_screen_produces_sync_only_frame() {
    let mut state = AltScreenState::new();
    compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);

    let frame = compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);
    assert!(
        frame.ops.iter().all(|op| !op.contains("\x1b[2K")),
        "no row writes expected"
    );
    assert!(frame.ops.contains(&HIDE_CURSOR.to_string()));
}

#[test]
fn changed_row_is_repainted_in_place() {
    let mut state = AltScreenState::new();
    compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);

    let frame = compute_alt_screen_frame(&mut state, lines(&["alpha", "BETA"]), 40, 3, None, false);
    let row_writes: Vec<&String> = frame
        .ops
        .iter()
        .filter(|op| op.contains("\x1b[2K"))
        .collect();
    assert_eq!(row_writes, vec!["\x1b[2;1H\x1b[2KBETA"]);
}

#[test]
fn size_change_triggers_full_clear() {
    let mut state = AltScreenState::new();
    compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);
    let before = state.full_redraws;

    let frame = compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 60, 3, None, false);
    assert!(frame.ops.contains(&"\x1b[2J".to_string()));
    assert_eq!(state.full_redraws, before + 1);
}

#[test]
fn hardware_cursor_is_positioned_and_shown_or_hidden() {
    let mut state = AltScreenState::new();
    compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, false);

    let frame = compute_alt_screen_frame(
        &mut state,
        lines(&["alpha", "beta"]),
        40,
        3,
        Some((1, 3)),
        true,
    );
    assert!(frame.ops.contains(&"\x1b[2;4H".to_string()));
    assert!(frame.ops.contains(&SHOW_CURSOR.to_string()));
    assert_eq!(frame.hardware_cursor, Some((1, 3)));

    let frame = compute_alt_screen_frame(&mut state, lines(&["alpha", "beta"]), 40, 3, None, true);
    assert!(frame.ops.contains(&HIDE_CURSOR.to_string()));
}

#[test]
fn screen_taller_than_height_is_clamped_to_the_last_rows() {
    let mut state = AltScreenState::new();
    let frame = compute_alt_screen_frame(
        &mut state,
        lines(&["r0", "r1", "r2", "r3"]),
        40,
        2,
        None,
        false,
    );
    // Upstream slices the tail: the last `height` rows are visible.
    assert!(frame.ops.contains(&"\x1b[1;1H\x1b[2Kr2".to_string()));
    assert!(frame.ops.contains(&"\x1b[2;1H\x1b[2Kr3".to_string()));
}

#[test]
fn enter_and_exit_sequences_match_upstream() {
    let enter = enter_alt_screen_sequences(false);
    assert_eq!(enter.len(), 1);
    assert!(enter[0].starts_with("\x1b[?1049h"));
    assert!(enter[0].contains("\x1b[?7l"));
    assert!(enter[0].ends_with("\x1b[2J\x1b[H\x1b[?25l"));

    let exit = exit_alt_screen_sequences();
    assert_eq!(exit.len(), 1);
    assert!(exit[0].starts_with("\x1b[?2026h\x1b[?1049l"));
    assert!(exit[0].ends_with("\x1b[?25h\x1b[?2026l"));
}
