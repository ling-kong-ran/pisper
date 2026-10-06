//! Ports of upstream `packages/tui/test/editor.test.ts` describe blocks that
//! cover the core editing state machine: prompt history, state accessors,
//! backslash+Enter, Kitty CSI-u, word wrapping, kill ring, undo, character
//! jump and paste markers. The autocomplete describe block joins with the
//! select-list slice; the ICU-CJK wrap cases use the same limitation note as
//! word-navigation.

use crate::tui::component::Component;
use crate::tui::components::editor::{word_wrap_line, Editor, EditorOptions};
use crate::tui::utils::{strip_terminal_sequences, visible_width};

fn new_editor() -> Editor {
    Editor::new(None, EditorOptions::default())
}

// ---------------------------------------------------------------------------
// Prompt history navigation
// ---------------------------------------------------------------------------

#[test]
fn history_does_nothing_on_up_arrow_when_history_is_empty() {
    let mut editor = new_editor();
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn history_shows_most_recent_entry_on_up_arrow_when_empty() {
    let mut editor = new_editor();
    editor.add_to_history("first prompt");
    editor.add_to_history("second prompt");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second prompt");
}

#[test]
fn history_cycles_through_entries_on_repeated_up_arrow() {
    let mut editor = new_editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("third");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
}

#[test]
fn history_jumps_to_start_before_entering_history_from_draft() {
    let mut editor = new_editor();
    editor.add_to_history("prompt");
    editor.set_text("draft");
    editor.handle_input("\x1b[D");
    editor.handle_input("\x1b[D");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "draft");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "prompt");

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "draft");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn history_navigates_forward_with_down_arrow() {
    let mut editor = new_editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("third");
    editor.set_text("draft");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A"); // start of draft (move to line start)
    editor.handle_input("\x1b[A"); // third
    editor.handle_input("\x1b[A"); // second
    editor.handle_input("\x1b[A"); // first

    editor.handle_input("\x1b[B"); // second
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[B"); // third
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1b[B"); // draft
    assert_eq!(editor.get_text(), "draft");
}

// ---------------------------------------------------------------------------
// public state accessors
// ---------------------------------------------------------------------------

#[test]
fn accessors_return_cursor_position() {
    let mut editor = new_editor();
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("a");
    editor.handle_input("b");
    editor.handle_input("c");
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x1b[D");
    assert_eq!(editor.get_cursor(), (0, 2));
}

#[test]
fn accessors_return_lines_as_a_defensive_copy() {
    let mut editor = new_editor();
    editor.set_text("a\nb");
    let lines = editor.get_lines();
    assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
    drop(lines);
    assert_eq!(editor.get_lines(), vec!["a".to_string(), "b".to_string()]);
}

// ---------------------------------------------------------------------------
// Backslash+Enter newline workaround
// ---------------------------------------------------------------------------

#[test]
fn backslash_inserts_immediately_without_buffering() {
    let mut editor = new_editor();
    editor.handle_input("\\");
    assert_eq!(editor.get_text(), "\\");
}

#[test]
fn backslash_converts_standalone_to_newline_on_enter() {
    let mut editor = new_editor();
    editor.handle_input("\\");
    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "\n");
}

#[test]
fn backslash_inserts_normally_when_followed_by_other_characters() {
    let mut editor = new_editor();
    editor.handle_input("\\");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "\\x");
}

#[test]
fn backslash_does_not_trigger_newline_when_not_immediately_before_cursor() {
    let mut editor = new_editor();
    let submitted = std::sync::Arc::new(std::sync::Mutex::new(false));
    let sink = std::sync::Arc::clone(&submitted);
    editor.on_submit(move |_| {
        *sink.lock().unwrap() = true;
    });

    editor.handle_input("\\");
    editor.handle_input("x");
    editor.handle_input("\r");
    assert!(*submitted.lock().unwrap());
}

#[test]
fn backslash_only_removes_one_backslash_when_multiple_are_present() {
    let mut editor = new_editor();
    editor.handle_input("\\");
    editor.handle_input("\\");
    editor.handle_input("\\");
    assert_eq!(editor.get_text(), "\\\\\\");

    editor.handle_input("\r");
    // Expected: two backslashes followed by a newline.
    let expected: String = ['\\', '\\', '\n'].iter().collect();
    assert_eq!(editor.get_text(), expected);
}

// ---------------------------------------------------------------------------
// Kitty CSI-u handling
// ---------------------------------------------------------------------------

#[test]
fn kitty_ignores_printable_csi_u_with_unsupported_modifiers() {
    let mut editor = new_editor();
    editor.handle_input("\x1b[99;9u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn kitty_inserts_shifted_csi_u_letters_as_text() {
    let mut editor = new_editor();
    editor.handle_input("\x1b[69;2u");
    assert_eq!(editor.get_text(), "E");
}

#[test]
fn kitty_inserts_shifted_modify_other_keys_letters_as_text() {
    let mut editor = new_editor();
    editor.handle_input("\x1b[27;2;69~");
    assert_eq!(editor.get_text(), "E");
}

// ---------------------------------------------------------------------------
// Word wrapping (pure wordWrapLine cases + render-level invariants)
// ---------------------------------------------------------------------------

fn chunks_of(text: &str, width: usize) -> Vec<String> {
    word_wrap_line(text, width, None)
        .into_iter()
        .map(|chunk| chunk.text)
        .collect()
}

#[test]
fn wrap_breaks_at_word_boundaries_instead_of_mid_word() {
    let chunks = chunks_of(
        "Hello world this is a test of word wrapping functionality",
        40,
    );
    for chunk in &chunks {
        let trimmed = chunk.trim_end();
        if let Some(last) = trimmed.chars().last() {
            assert!(
                last.is_ascii_alphanumeric() || ".,!?;:".contains(last),
                "line ends unexpectedly with {last}"
            );
        }
    }
}

#[test]
fn wrap_does_not_start_lines_with_leading_whitespace() {
    let chunks = chunks_of("Word1 Word2 Word3 Word4 Word5 Word6", 20);
    for chunk in chunks.iter().skip(1) {
        assert!(
            !chunk.starts_with(' '),
            "chunk starts with whitespace: {chunk:?}"
        );
    }
}

#[test]
fn wrap_breaks_long_words_at_character_level() {
    let chunks = chunks_of("https://example.com/very/long/path/that/exceeds/width", 30);
    for chunk in &chunks {
        assert!(visible_width(chunk) <= 30, "chunk too wide: {chunk:?}");
    }
}

#[test]
fn wrap_wraps_word_to_next_line_when_it_ends_exactly_at_width() {
    let chunks = chunks_of("hello world test", 11);
    assert_eq!(chunks, vec!["hello ".to_string(), "world test".to_string()]);
}

#[test]
fn wrap_keeps_whitespace_at_width_boundary_on_same_line() {
    let chunks = chunks_of("hello world test", 12);
    assert_eq!(chunks, vec!["hello world ".to_string(), "test".to_string()]);
}

#[test]
fn wrap_handles_unbreakable_word_filling_width_exactly_followed_by_space() {
    let chunks = chunks_of("aaaaaaaaaaaa aaaa", 12);
    assert_eq!(
        chunks,
        vec!["aaaaaaaaaaaa".to_string(), " aaaa".to_string()]
    );
}

#[test]
fn wrap_preserves_multiple_spaces_within_words_on_same_line() {
    let mut editor = new_editor();
    editor.set_text("Word1   Word2    Word3");
    let lines = editor.render(50);
    let content = strip_terminal_sequences(&lines[1]).trim().to_string();
    assert!(content.contains("Word1   Word2"));
}

#[test]
fn wrap_handles_empty_string_with_borders() {
    let mut editor = new_editor();
    editor.set_text("");
    let lines = editor.render(40);
    assert_eq!(lines.len(), 3);
}

#[test]
fn wrap_handles_single_word_that_fits_exactly() {
    let mut editor = new_editor();
    editor.set_text("1234567890");
    let lines = editor.render(11);
    assert_eq!(lines.len(), 3);
    let content = strip_terminal_sequences(&lines[1]);
    assert!(content.contains("1234567890"));
}

#[test]
fn wrap_breaks_long_words_at_character_level_in_render() {
    let mut editor = new_editor();
    editor.set_text("Check https://example.com/very/long/path/that/exceeds/width here");
    let lines = editor.render(30);
    for line in lines.iter().skip(1).take(lines.len() - 2) {
        assert_eq!(visible_width(line), 30, "line {line:?} overflowed");
    }
}

// ---------------------------------------------------------------------------
// Character jump (Ctrl+])
// ---------------------------------------------------------------------------

#[test]
fn jump_forward_finds_next_occurrence_of_character() {
    let mut editor = new_editor();
    editor.set_text("hello world");
    editor.handle_input("\x01"); // Ctrl+A - go to start
    editor.handle_input("\x1d"); // Ctrl+] enters jump-forward mode
    editor.handle_input("o");
    // First 'o' after cursor (0) is at index 4.
    assert_eq!(editor.get_cursor(), (0, 4));
}

#[test]
fn jump_forward_multiline_searches_forward() {
    let mut editor = new_editor();
    editor.set_text("ab\ncb");
    editor.handle_input("\x01"); // Ctrl+A - go to start
    editor.handle_input("\x1d");
    editor.handle_input("c");
    // 'c' is on line 1 col 0.
    assert_eq!(editor.get_cursor(), (1, 0));
}

#[test]
fn jump_backward_finds_previous_occurrence() {
    let mut editor = new_editor();
    editor.set_text("abcba");
    // Move to end.
    editor.handle_input("\x05"); // Ctrl+E
    editor.handle_input("\x1b\x1d"); // Ctrl+Alt+] (backward jump trigger)
    editor.handle_input("b");
    // Backward from 5: previous 'b' is at index 3.
    assert_eq!(editor.get_cursor(), (0, 3));
}

// ---------------------------------------------------------------------------
// Paste markers
// ---------------------------------------------------------------------------

#[test]
fn paste_large_paste_creates_atomic_marker_and_backspace_removes_it() {
    let mut editor = new_editor();
    let content = (0..15)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.handle_input(&format!("\x1b[200~{content}\x1b[201~"));

    // Large paste becomes a marker.
    assert!(
        editor.get_text().starts_with("[paste #1 +15 lines]"),
        "text: {}",
        editor.get_text()
    );

    // Backspace removes the whole marker.
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn paste_small_multiline_paste_inserts_normally() {
    let mut editor = new_editor();
    editor.handle_input("\x1b[200~a\nb\x1b[201~");
    assert_eq!(editor.get_text(), "a\nb");
}

#[test]
fn submit_expands_paste_markers_and_trims() {
    let mut editor = new_editor();
    let content = (0..15)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    editor.handle_input(&format!("\x1b[200~{content}\x1b[201~"));
    editor.handle_input("tail");

    let submitted = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = std::sync::Arc::clone(&submitted);
    editor.on_submit(move |text| {
        *sink.lock().unwrap() = text.to_string();
    });
    editor.handle_input("\r");

    let value = submitted.lock().unwrap().clone();
    assert!(value.starts_with("line 0"));
    assert!(value.ends_with("tail"));
}

// ---------------------------------------------------------------------------
// Undo (core)
// ---------------------------------------------------------------------------

#[test]
fn undo_restores_previous_state_across_newlines() {
    let mut editor = new_editor();
    editor.handle_input("a");
    editor.handle_input("b");
    editor.handle_input("\x1b[13;2u"); // Shift+Enter (new line)
    assert_eq!(editor.get_text(), "ab\n");
    editor.handle_input("c");
    assert_eq!(editor.get_text(), "ab\nc");

    editor.handle_input("\x1b[45;5u"); // Ctrl+-
    assert_eq!(editor.get_text(), "ab\n");
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "ab");
}

#[test]
fn submit_clears_editor_and_undo_stack() {
    let mut editor = new_editor();
    editor.handle_input("hello");
    editor.handle_input("\r");

    assert_eq!(editor.get_text(), "");
    // Undo after submit does nothing (stack cleared).
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}
