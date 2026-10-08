//! Ports of upstream `packages/tui/test/keybindings.test.ts`,
//! `test/fuzzy.test.ts` and `test/word-navigation.test.ts`.

use crate::tui::fuzzy::{fuzzy_filter, fuzzy_match};
use crate::tui::keybindings::{tui_keybindings, KeybindingsManager};
use crate::tui::undo_stack::UndoStack;
use crate::tui::word_navigation::{
    find_word_backward, find_word_forward, KillRing, KillRingPushOptions,
};

// ---------------------------------------------------------------------------
// keybindings.test.ts
// ---------------------------------------------------------------------------

fn manager() -> KeybindingsManager {
    KeybindingsManager::new(&tui_keybindings(), &[])
}

#[test]
fn keybindings_binds_ctrl_j_as_default_newline_alias() {
    let keybindings = manager();
    assert_eq!(
        keybindings.get_keys("tui.input.newLine"),
        vec!["shift+enter", "ctrl+j"]
    );
    assert!(keybindings.matches("\n", "tui.input.newLine"));
    assert!(keybindings.matches("\x1b[106;5u", "tui.input.newLine"));
}

#[test]
fn keybindings_binds_modified_and_unmodified_viewport_navigation() {
    let keybindings = manager();
    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLineStart"),
        vec!["home", "ctrl+home", "ctrl+a"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLineEnd"),
        vec!["end", "ctrl+end", "ctrl+e"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.pageUp"),
        vec!["pageUp", "ctrl+pageUp"]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.pageDown"),
        vec!["pageDown", "ctrl+pageDown"]
    );
}

#[test]
fn keybindings_leaves_prompt_history_navigation_unbound_by_default() {
    let keybindings = manager();
    assert!(keybindings
        .get_keys("tui.editor.historyPrevious")
        .is_empty());
    assert!(keybindings.get_keys("tui.editor.historyNext").is_empty());
}

#[test]
fn keybindings_binds_alt_screen_shortcuts() {
    let keybindings = manager();
    assert_eq!(keybindings.get_keys("tui.altScreen.pageUp"), vec!["pageUp"]);
    assert_eq!(
        keybindings.get_keys("tui.altScreen.pageDown"),
        vec!["pageDown"]
    );
    assert!(keybindings.get_keys("tui.altScreen.halfPageUp").is_empty());
    assert!(keybindings
        .get_keys("tui.altScreen.halfPageDown")
        .is_empty());
    assert!(keybindings.get_keys("tui.altScreen.lineUp").is_empty());
    assert!(keybindings.get_keys("tui.altScreen.lineDown").is_empty());
    assert_eq!(
        keybindings.get_keys("tui.altScreen.previousPrompt"),
        vec!["ctrl+shift+up", "ctrl+up"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.nextPrompt"),
        vec!["ctrl+shift+down", "ctrl+down"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.search"),
        vec!["ctrl+shift+f"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchNext"),
        vec!["enter", "ctrl+g"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchPrevious"),
        vec!["shift+enter", "ctrl+shift+g"]
    );
    assert_eq!(
        keybindings.get_keys("tui.altScreen.searchClose"),
        vec!["escape"]
    );
    assert_eq!(keybindings.get_keys("tui.altScreen.top"), vec!["home"]);
    assert_eq!(keybindings.get_keys("tui.altScreen.bottom"), vec!["end"]);
}

#[test]
fn keybindings_does_not_evict_selector_confirm_when_submit_is_rebound() {
    let keybindings = KeybindingsManager::new(
        &tui_keybindings(),
        &[(
            "tui.input.submit",
            vec!["enter".into(), "ctrl+enter".into()],
        )],
    );
    assert_eq!(
        keybindings.get_keys("tui.input.submit"),
        vec!["enter", "ctrl+enter"]
    );
    assert_eq!(keybindings.get_keys("tui.select.confirm"), vec!["enter"]);
}

#[test]
fn keybindings_does_not_evict_cursor_bindings_on_key_reuse() {
    let keybindings = KeybindingsManager::new(
        &tui_keybindings(),
        &[("tui.select.up", vec!["up".into(), "ctrl+p".into()])],
    );
    assert_eq!(keybindings.get_keys("tui.select.up"), vec!["up", "ctrl+p"]);
    assert_eq!(keybindings.get_keys("tui.editor.cursorUp"), vec!["up"]);
}

#[test]
fn keybindings_reports_direct_user_binding_conflicts() {
    let keybindings = KeybindingsManager::new(
        &tui_keybindings(),
        &[
            ("tui.input.submit", vec!["ctrl+x".into()]),
            ("tui.select.confirm", vec!["ctrl+x".into()]),
        ],
    );
    assert_eq!(
        keybindings.get_conflicts(),
        vec![crate::tui::keybindings::KeybindingConflict {
            key: "ctrl+x".to_string(),
            keybindings: vec![
                "tui.input.submit".to_string(),
                "tui.select.confirm".to_string()
            ],
        }]
    );
    assert_eq!(
        keybindings.get_keys("tui.editor.cursorLeft"),
        vec!["left", "ctrl+b"]
    );
}

// ---------------------------------------------------------------------------
// fuzzy.test.ts
// ---------------------------------------------------------------------------

#[test]
fn fuzzy_empty_query_matches_everything_with_score_zero() {
    let result = fuzzy_match("", "anything");
    assert!(result.matches);
    assert_eq!(result.score, 0.0);
}

#[test]
fn fuzzy_query_longer_than_text_does_not_match() {
    assert!(!fuzzy_match("longquery", "short").matches);
}

#[test]
fn fuzzy_exact_match_has_negative_score() {
    let result = fuzzy_match("test", "test");
    assert!(result.matches);
    assert!(result.score < 0.0);
}

#[test]
fn fuzzy_characters_must_appear_in_order() {
    assert!(fuzzy_match("abc", "aXbXc").matches);
    assert!(!fuzzy_match("abc", "cba").matches);
}

#[test]
fn fuzzy_case_insensitive_matching() {
    assert!(fuzzy_match("ABC", "abc").matches);
    assert!(fuzzy_match("abc", "ABC").matches);
}

#[test]
fn fuzzy_consecutive_matches_score_better_than_scattered() {
    let consecutive = fuzzy_match("foo", "foobar");
    let scattered = fuzzy_match("foo", "f_o_o_bar");
    assert!(consecutive.matches);
    assert!(scattered.matches);
    assert!(consecutive.score < scattered.score);
}

#[test]
fn fuzzy_word_boundary_matches_score_better() {
    let at_boundary = fuzzy_match("fb", "foo-bar");
    let not_at_boundary = fuzzy_match("fb", "afbx");
    assert!(at_boundary.matches);
    assert!(not_at_boundary.matches);
    assert!(at_boundary.score < not_at_boundary.score);
}

#[test]
fn fuzzy_matches_swapped_alpha_numeric_tokens() {
    assert!(fuzzy_match("codex52", "gpt-5.2-codex").matches);
}

#[test]
fn fuzzy_filter_empty_query_returns_all_items_unchanged() {
    let items = vec![
        "apple".to_string(),
        "banana".to_string(),
        "cherry".to_string(),
    ];
    let result = fuzzy_filter(items.clone(), "", |x| x);
    assert_eq!(result, items);
}

#[test]
fn fuzzy_filter_excludes_non_matching_items() {
    let items = vec![
        "apple".to_string(),
        "banana".to_string(),
        "cherry".to_string(),
    ];
    let result = fuzzy_filter(items, "an", |x| x);
    assert!(result.contains(&"banana".to_string()));
    assert!(!result.contains(&"apple".to_string()));
    assert!(!result.contains(&"cherry".to_string()));
}

#[test]
fn fuzzy_filter_sorts_by_match_quality() {
    let items = vec![
        "a_p_p".to_string(),
        "app".to_string(),
        "application".to_string(),
    ];
    let result = fuzzy_filter(items, "app", |x| x);
    assert_eq!(result[0], "app");
}

#[test]
fn fuzzy_filter_prioritizes_exact_matches_over_longer_prefix_matches() {
    let items = vec!["clone".to_string(), "cl".to_string()];
    let result = fuzzy_filter(items, "cl", |x| x);
    assert_eq!(result, vec!["cl".to_string(), "clone".to_string()]);
}

#[test]
fn fuzzy_filter_supports_custom_get_text() {
    let items = vec![("foo", 1u8), ("bar", 2u8), ("foobar", 3u8)];
    let result = fuzzy_filter(items, "foo", |item| item.0);
    assert_eq!(result.len(), 2);
    assert!(result.iter().map(|item| item.0).any(|name| name == "foo"));
    assert!(result
        .iter()
        .map(|item| item.0)
        .any(|name| name == "foobar"));
}

#[test]
fn fuzzy_filter_matches_slash_separated_queries_against_reordered_text() {
    let items = vec![("gpt-5.5", "openai-codex")];
    let result = fuzzy_filter(items, "openai-codex/gpt-5.5", |model| match model {
        ("gpt-5.5", "openai-codex") => "gpt-5.5 openai-codex",
        _ => "",
    });
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], ("gpt-5.5", "openai-codex"));
}

// ---------------------------------------------------------------------------
// word-navigation.test.ts
// ---------------------------------------------------------------------------

fn no_options() -> crate::tui::word_navigation::WordNavigationOptions<'static> {
    Default::default()
}

#[test]
fn word_backward_basic_words() {
    let text = "hello world";
    assert_eq!(find_word_backward(text, 11, &no_options()), 6);
    assert_eq!(find_word_backward(text, 6, &no_options()), 0);
}

#[test]
fn word_backward_dotted() {
    let text = "foo.bar";
    assert_eq!(find_word_backward(text, 7, &no_options()), 4);
    assert_eq!(find_word_backward(text, 4, &no_options()), 3);
    assert_eq!(find_word_backward(text, 3, &no_options()), 0);
}

#[test]
fn word_backward_colon() {
    let text = "foo:bar";
    assert_eq!(find_word_backward(text, 7, &no_options()), 4);
    assert_eq!(find_word_backward(text, 4, &no_options()), 3);
    assert_eq!(find_word_backward(text, 3, &no_options()), 0);
}

#[test]
fn word_backward_path() {
    let text = "path/to/file";
    assert_eq!(find_word_backward(text, 12, &no_options()), 8);
    assert_eq!(find_word_backward(text, 8, &no_options()), 7);
    // "/to" is one word-like segment with "/" as punctuation boundary.
    assert_eq!(find_word_backward(text, 7, &no_options()), 5);
    assert_eq!(find_word_backward(text, 5, &no_options()), 4);
    assert_eq!(find_word_backward(text, 4, &no_options()), 0);
}

/// ICU dictionary-style segmentation for the CJK fixture: each CJK char is a
/// separate word-like segment (upstream relies on Intl.Segmenter's CJK
/// dictionary breaking, which unicode-segmentation does not provide — hence
/// the explicit segmenter, the upstream `WordNavigationOptions.segment` escape
/// hatch). CJK cursor offsets are bytes (each Han char is 3 bytes).
fn cjk_aware_segments(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    // ICU's CJK dictionary groups Han chars into pairs ("你好"/"世界").
    let mut cjk_pair = 0usize;
    for c in text.chars() {
        if ('\u{2E80}'..='\u{9FFF}').contains(&c) {
            current.push(c);
            cjk_pair += 1;
            if cjk_pair == 2 {
                out.push(std::mem::take(&mut current));
                cjk_pair = 0;
            }
        } else if c.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.push(c.to_string());
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[test]
fn word_backward_cjk_mixed() {
    let text = "你好世界 test";
    let options = crate::tui::word_navigation::WordNavigationOptions {
        segment: Some(&|text: &str| cjk_aware_segments(text)),
        ..Default::default()
    };
    // Byte offsets: 你好世界 = 12 bytes, space = 1, "test" = 4.
    assert_eq!(find_word_backward(text, text.len(), &options), 13);
    assert_eq!(find_word_backward(text, 13, &options), 6);
    assert_eq!(find_word_backward(text, 6, &options), 0);
}

#[test]
fn word_backward_whitespace_at_boundaries() {
    let text = "  hello  ";
    assert_eq!(find_word_backward(text, 9, &no_options()), 2);
    assert_eq!(find_word_backward(text, 2, &no_options()), 0);
}

#[test]
fn word_backward_punctuation_run() {
    let text = "foo...bar";
    assert_eq!(find_word_backward(text, 9, &no_options()), 6);
    assert_eq!(find_word_backward(text, 6, &no_options()), 3);
    assert_eq!(find_word_backward(text, 3, &no_options()), 0);
}

#[test]
fn word_backward_cursor_at_zero_returns_zero() {
    assert_eq!(find_word_backward("hello", 0, &no_options()), 0);
}

#[test]
fn word_forward_basic_words() {
    let text = "hello world";
    assert_eq!(find_word_forward(text, 0, &no_options()), 5);
    assert_eq!(find_word_forward(text, 5, &no_options()), 11);
}

#[test]
fn word_forward_dotted() {
    let text = "foo.bar";
    assert_eq!(find_word_forward(text, 0, &no_options()), 3);
    assert_eq!(find_word_forward(text, 3, &no_options()), 4);
    assert_eq!(find_word_forward(text, 4, &no_options()), 7);
}

#[test]
fn word_forward_colon() {
    let text = "foo:bar";
    assert_eq!(find_word_forward(text, 0, &no_options()), 3);
    assert_eq!(find_word_forward(text, 3, &no_options()), 4);
    assert_eq!(find_word_forward(text, 4, &no_options()), 7);
}

#[test]
fn word_forward_path() {
    let text = "path/to/file";
    assert_eq!(find_word_forward(text, 0, &no_options()), 4);
    assert_eq!(find_word_forward(text, 4, &no_options()), 5);
    assert_eq!(find_word_forward(text, 5, &no_options()), 7);
    assert_eq!(find_word_forward(text, 7, &no_options()), 8);
    assert_eq!(find_word_forward(text, 8, &no_options()), 12);
}

#[test]
fn word_forward_cjk_mixed() {
    let text = "你好世界 test";
    let first_end = find_word_forward(text, 0, &no_options());
    assert!(first_end > 0);
    assert!(first_end <= 4);
    // Walk to end.
    let mut pos = 0;
    while pos < text.len() {
        let next = find_word_forward(text, pos, &no_options());
        if next == pos {
            break;
        }
        pos = next;
    }
    assert_eq!(pos, text.len());
}

#[test]
fn word_forward_whitespace_at_boundaries() {
    let text = "  hello  ";
    assert_eq!(find_word_forward(text, 0, &no_options()), 7);
    assert_eq!(find_word_forward(text, 7, &no_options()), 9);
}

// ---------------------------------------------------------------------------
// kill-ring.ts / undo-stack.ts (upstream has no dedicated unit tests; the
// editor tests exercise them — basic behavior pinned here)
// ---------------------------------------------------------------------------

#[test]
fn kill_ring_accumulates_rotates_and_peeks() {
    let mut ring = KillRing::default();
    ring.push(
        "foo",
        KillRingPushOptions {
            prepend: false,
            accumulate: false,
        },
    );
    ring.push(
        "bar",
        KillRingPushOptions {
            prepend: true,
            accumulate: true,
        },
    );
    assert_eq!(ring.peek().map(String::as_str), Some("barfoo"));
    ring.push(
        "baz",
        KillRingPushOptions {
            prepend: false,
            accumulate: false,
        },
    );
    assert_eq!(ring.len(), 2);
    assert_eq!(ring.peek().map(String::as_str), Some("baz"));
    ring.rotate();
    assert_eq!(ring.peek().map(String::as_str), Some("barfoo"));
    ring.push(
        "",
        KillRingPushOptions {
            prepend: false,
            accumulate: false,
        },
    );
    assert_eq!(ring.len(), 2);
}

#[test]
fn undo_stack_pushes_clones_and_pops() {
    let mut stack: UndoStack<Vec<u8>> = UndoStack::new();
    let state = vec![1u8, 2, 3];
    stack.push(&state);
    let mut mutated = state.clone();
    mutated.push(4);
    stack.push(&mutated);
    assert_eq!(stack.len(), 2);
    assert_eq!(stack.pop(), Some(vec![1, 2, 3, 4]));
    assert_eq!(stack.pop(), Some(vec![1, 2, 3]));
    assert!(stack.pop().is_none());
    stack.push(&state);
    stack.clear();
    assert!(stack.is_empty());
}
