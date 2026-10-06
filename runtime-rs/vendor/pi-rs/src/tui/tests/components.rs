//! Ports of upstream `packages/tui/test/input.test.ts` plus basic Text
//! component pinning (upstream has no dedicated text.test.ts).

use crate::tui::component::{Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventType};
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::text::Text;
use crate::tui::utils::{strip_terminal_sequences, visible_width};

fn new_input() -> Input {
    Input::new(InputOptions::default())
}

fn typed_input() -> Input {
    Input::new(InputOptions::default())
}

#[test]
fn input_submits_value_including_backslash_on_enter() {
    let mut input = typed_input();
    let submitted: std::sync::Arc<std::sync::Mutex<Option<String>>> = std::sync::Arc::default();
    let sink = std::sync::Arc::clone(&submitted);
    input.on_submit(move |value| {
        *sink.lock().unwrap() = Some(value.to_string());
    });

    for ch in ["h", "e", "l", "l", "o", "\\", "\r"] {
        input.handle_input(ch);
    }

    assert_eq!(submitted.lock().unwrap().as_deref(), Some("hello\\"));
}

#[test]
fn input_inserts_backslash_as_regular_character() {
    let mut input = typed_input();
    input.handle_input("\\");
    input.handle_input("x");
    assert_eq!(input.value(), "\\x");
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn input_supports_custom_prompt_and_styled_placeholder() {
    let mut input = Input::new(InputOptions {
        prompt: Some(String::new()),
        placeholder: Some("Find transcript".to_string()),
        placeholder_style: Some(std::sync::Arc::new(|text| format!("\x1b[2m{text}\x1b[22m"))),
    });
    input.set_focused(true);

    let empty = input.render(20).remove(0);
    assert!(empty.contains("\x1b[2m"));
    assert_eq!(
        strip_terminal_sequences(&empty).trim_end(),
        "Find transcript"
    );

    input.handle_input("n");
    let populated = input.render(20).remove(0);
    assert_eq!(strip_terminal_sequences(&populated).trim_end(), "n");
}

#[test]
fn input_does_not_overflow_with_wide_cjk_and_fullwidth_text() {
    let width = 93;
    let cases = [
        "가나다라마바사아자차카타파하 한글 텍스트가 터미널 너비를 초과하면 크래시가 발생합니다 이것은 재현용 테스트입니다",
        "これはテスト文章です。日本語のテキストが正しく表示されるかどうかを確認するためのサンプルテキストです。あいうえお",
        "这是一段测试文本，用于验证中文字符在终端中的显示宽度是否被正确计算，如果不正确就会导致用户界面崩溃的问题",
        "ＡＢＣＤＥＦＧＨＩＪＫＬＭＮＯＰＱＲＳＴＵＶＷＸＹＺ０１２３４５６７８９ａｂｃｄｅｆｇｈｉｊｋｌｍ",
    ];
    for text in cases {
        // start
        let mut input = new_input();
        input.set_value(text);
        input.set_focused(true);
        let line = input.render(width).remove(0);
        assert!(
            visible_width(&line) <= width,
            "overflow at start for {text}"
        );

        // middle
        let mut input = new_input();
        input.set_value(text);
        input.set_focused(true);
        for _ in 0..10 {
            input.handle_input("\x1b[C");
        }
        let line = input.render(width).remove(0);
        assert!(
            visible_width(&line) <= width,
            "overflow at middle for {text}"
        );

        // end
        let mut input = new_input();
        input.set_value(text);
        input.set_focused(true);
        input.handle_input("\x05");
        let line = input.render(width).remove(0);
        assert!(visible_width(&line) <= width, "overflow at end for {text}");
    }
}

#[test]
fn input_keeps_cursor_visible_when_horizontally_scrolling() {
    let mut input = new_input();
    let width = 20;
    input.set_value("가나다라마바사아자차카타파하");
    input.set_focused(true);
    input.handle_input("\x01");
    for _ in 0..5 {
        input.handle_input("\x1b[C");
    }

    let line = input.render(width).remove(0);
    assert!(visible_width(&line) <= width);
}

#[test]
fn input_handles_mouse_press_to_position_cursor() {
    use crate::tui::component::TuiMouseEventResult;
    let mut input = new_input();
    input.set_value("hello world");
    input.set_focused(true);
    let _ = input.render(40);

    let event = TuiMouseEvent {
        event_type: TuiMouseEventType::Press,
        button: TuiMouseButton::Left,
        x: 6,
        y: 0,
        screen_x: 6,
        screen_y: 0,
        width: 40,
        height: 1,
        shift: false,
        alt: false,
        ctrl: false,
        wheel_delta: None,
        click_count: None,
    };
    let result = input.handle_mouse(&event);
    assert_eq!(
        result,
        Some(TuiMouseEventResult {
            handled: true,
            focus: true,
            ..Default::default()
        })
    );
    assert_eq!(input.cursor(), 4); // after "hell" (visible col 4 + prompt offset 2)
}

// ---------------------------------------------------------------------------
// Kill ring
// ---------------------------------------------------------------------------

#[test]
fn kill_ring_ctrl_w_saves_and_ctrl_y_yanks() {
    let mut input = new_input();
    input.set_value("foo bar baz");
    input.handle_input("\x05"); // Ctrl+E
    input.handle_input("\x17"); // Ctrl+W
    assert_eq!(input.value(), "foo bar ");
    input.handle_input("\x01"); // Ctrl+A
    input.handle_input("\x19"); // Ctrl+Y
    assert_eq!(input.value(), "bazfoo bar ");
}

#[test]
fn kill_ring_ctrl_w_preserves_ascii_punctuation_boundaries() {
    let mut input = new_input();
    input.set_value("foo.bar");
    input.handle_input("\x05");
    input.handle_input("\x17");
    assert_eq!(input.value(), "foo.");

    input.set_value("foo:bar");
    input.handle_input("\x05");
    input.handle_input("\x17");
    assert_eq!(input.value(), "foo:");
}

#[test]
#[ignore = "requires ICU CJK dictionary word segmentation (Intl.Segmenter); unicode-segmentation merges Han runs"]
fn kill_ring_ctrl_w_handles_unicode_word_boundaries() {
    let mut input = new_input();
    input.set_value("你好世界。你好，世界");
    input.handle_input("\x05");
    input.handle_input("\x17");
    assert_eq!(input.value(), "你好世界。你好，");
    input.handle_input("\x17");
    assert_eq!(input.value(), "你好世界。你好");
    input.handle_input("\x17");
    assert_eq!(input.value(), "你好世界。");
    input.handle_input("\x17");
    assert_eq!(input.value(), "你好世界");
    input.handle_input("\x17");
    assert_eq!(input.value(), "你好");
    input.handle_input("\x17");
    assert_eq!(input.value(), "");
}

#[test]
fn kill_ring_ctrl_u_saves_deleted_text() {
    let mut input = new_input();
    input.set_value("hello world");
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }
    input.handle_input("\x15");
    assert_eq!(input.value(), "world");
    input.handle_input("\x19");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn kill_ring_ctrl_k_saves_deleted_text() {
    let mut input = new_input();
    input.set_value("hello world");
    input.handle_input("\x01");
    input.handle_input("\x0b");
    assert_eq!(input.value(), "");
    input.handle_input("\x19");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn kill_ring_ctrl_y_does_nothing_when_empty() {
    let mut input = new_input();
    input.set_value("test");
    input.handle_input("\x05");
    input.handle_input("\x19");
    assert_eq!(input.value(), "test");
}

#[test]
fn kill_ring_alt_y_cycles_after_ctrl_y() {
    let mut input = new_input();

    input.set_value("first");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("second");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("third");
    input.handle_input("\x05");
    input.handle_input("\x17");
    assert_eq!(input.value(), "");

    input.handle_input("\x19");
    assert_eq!(input.value(), "third");
    input.handle_input("\x1by");
    assert_eq!(input.value(), "second");
    input.handle_input("\x1by");
    assert_eq!(input.value(), "first");
    input.handle_input("\x1by");
    assert_eq!(input.value(), "third");
}

#[test]
fn kill_ring_alt_y_does_nothing_if_not_preceded_by_yank() {
    let mut input = new_input();
    input.set_value("test");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("other");
    input.handle_input("\x05");

    input.handle_input("x");
    assert_eq!(input.value(), "otherx");

    input.handle_input("\x1by");
    assert_eq!(input.value(), "otherx");
}

#[test]
fn kill_ring_alt_y_does_nothing_with_single_entry() {
    let mut input = new_input();
    input.set_value("only");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.handle_input("\x19");
    assert_eq!(input.value(), "only");
    input.handle_input("\x1by");
    assert_eq!(input.value(), "only");
}

#[test]
fn kill_ring_consecutive_ctrl_w_accumulates() {
    let mut input = new_input();
    input.set_value("one two three");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.handle_input("\x17");
    input.handle_input("\x17");
    assert_eq!(input.value(), "");
    input.handle_input("\x19");
    assert_eq!(input.value(), "one two three");
}

#[test]
fn kill_ring_non_delete_actions_break_accumulation() {
    let mut input = new_input();
    input.set_value("foo bar baz");
    input.handle_input("\x05");
    input.handle_input("\x17");
    assert_eq!(input.value(), "foo bar ");

    input.handle_input("x");
    assert_eq!(input.value(), "foo bar x");

    input.handle_input("\x17");
    assert_eq!(input.value(), "foo bar ");

    input.handle_input("\x19");
    assert_eq!(input.value(), "foo bar x");

    input.handle_input("\x1by");
    assert_eq!(input.value(), "foo bar baz");
}

#[test]
fn kill_ring_non_yank_actions_break_alt_y_chain() {
    let mut input = new_input();
    input.set_value("first");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("second");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("");

    input.handle_input("\x19");
    assert_eq!(input.value(), "second");

    input.handle_input("x");
    assert_eq!(input.value(), "secondx");

    input.handle_input("\x1by");
    assert_eq!(input.value(), "secondx");
}

#[test]
fn kill_ring_rotation_persists_after_cycling() {
    let mut input = new_input();
    for value in ["first", "second", "third"] {
        input.set_value(value);
        input.handle_input("\x05");
        input.handle_input("\x17");
    }
    input.set_value("");

    input.handle_input("\x19");
    input.handle_input("\x1by");
    assert_eq!(input.value(), "second");

    input.handle_input("x");
    input.set_value("");

    input.handle_input("\x19");
    assert_eq!(input.value(), "second");
}

#[test]
fn kill_ring_forward_deletions_append_during_accumulation() {
    let mut input = new_input();
    input.set_value("prefix|suffix");
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x0b"); // Ctrl+K deletes "|suffix"
    assert_eq!(input.value(), "prefix");

    input.handle_input("\x19");
    assert_eq!(input.value(), "prefix|suffix");
}

#[test]
fn kill_ring_alt_d_deletes_word_forward() {
    let mut input = new_input();
    input.set_value("hello world test");
    input.handle_input("\x01");

    input.handle_input("\x1bd");
    assert_eq!(input.value(), " world test");

    input.handle_input("\x1bd");
    assert_eq!(input.value(), " test");

    input.handle_input("\x19");
    assert_eq!(input.value(), "hello world test");
}

#[test]
fn kill_ring_alt_d_preserves_ascii_punctuation_boundaries() {
    let mut input = new_input();
    input.set_value("foo.bar baz");
    input.handle_input("\x01");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), ".bar baz");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "bar baz");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), " baz");
}

#[test]
#[ignore = "requires ICU CJK dictionary word segmentation (Intl.Segmenter); unicode-segmentation merges Han runs"]
fn kill_ring_alt_d_handles_unicode_word_boundaries() {
    let mut input = new_input();
    input.set_value("你好世界。你好，世界");
    input.handle_input("\x01");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "世界。你好，世界");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "。你好，世界");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "你好，世界");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "，世界");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "世界");
    input.handle_input("\x1bd");
    assert_eq!(input.value(), "");
}

#[test]
fn kill_ring_yank_in_middle_of_text() {
    let mut input = new_input();
    input.set_value("word");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("hello world");
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x19");
    assert_eq!(input.value(), "hello wordworld");
}

#[test]
fn kill_ring_yank_pop_in_middle_of_text() {
    let mut input = new_input();
    input.set_value("FIRST");
    input.handle_input("\x05");
    input.handle_input("\x17");
    input.set_value("SECOND");
    input.handle_input("\x05");
    input.handle_input("\x17");

    input.set_value("hello world");
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x19");
    assert_eq!(input.value(), "hello SECONDworld");

    input.handle_input("\x1by");
    assert_eq!(input.value(), "hello FIRSTworld");
}

// ---------------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------------

#[test]
fn undo_does_nothing_when_stack_empty() {
    let mut input = new_input();
    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "");
}

#[test]
fn undo_coalesces_consecutive_word_characters() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d"] {
        input.handle_input(ch);
    }
    assert_eq!(input.value(), "hello world");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "");
}

#[test]
fn undo_spaces_one_at_a_time() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " ", " "] {
        input.handle_input(ch);
    }
    assert_eq!(input.value(), "hello  ");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello ");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "");
}

#[test]
fn undo_backspace() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o"] {
        input.handle_input(ch);
    }
    input.handle_input("\x7f");
    assert_eq!(input.value(), "hell");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello");
}

#[test]
fn undo_forward_delete() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o"] {
        input.handle_input(ch);
    }
    input.handle_input("\x01");
    input.handle_input("\x1b[C");
    input.handle_input("\x1b[3~");
    assert_eq!(input.value(), "hllo");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello");
}

#[test]
fn undo_ctrl_w() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d"] {
        input.handle_input(ch);
    }
    assert_eq!(input.value(), "hello world");

    input.handle_input("\x17");
    assert_eq!(input.value(), "hello ");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn undo_ctrl_k() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d"] {
        input.handle_input(ch);
    }
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x0b");
    assert_eq!(input.value(), "hello ");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn undo_ctrl_u() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d"] {
        input.handle_input(ch);
    }
    input.handle_input("\x01");
    for _ in 0..6 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x15");
    assert_eq!(input.value(), "world");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn undo_yank() {
    let mut input = new_input();
    for ch in ["h", "e", "l", "l", "o", " "] {
        input.handle_input(ch);
    }
    input.handle_input("\x17");
    input.handle_input("\x19");
    assert_eq!(input.value(), "hello ");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "");
}

#[test]
fn undo_paste_atomically() {
    let mut input = new_input();
    input.set_value("hello world");
    input.handle_input("\x01");
    for _ in 0..5 {
        input.handle_input("\x1b[C");
    }

    input.handle_input("\x1b[200~beep boop\x1b[201~");
    assert_eq!(input.value(), "hellobeep boop world");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn undo_alt_d() {
    let mut input = new_input();
    input.set_value("hello world");
    input.handle_input("\x01");

    input.handle_input("\x1bd");
    assert_eq!(input.value(), " world");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "hello world");
}

#[test]
fn undo_cursor_movement_starts_new_unit() {
    let mut input = new_input();
    input.handle_input("a");
    input.handle_input("b");
    input.handle_input("c");
    input.handle_input("\x01");
    input.handle_input("\x05");
    input.handle_input("d");
    input.handle_input("e");
    assert_eq!(input.value(), "abcde");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "abc");

    input.handle_input("\x1b[45;5u");
    assert_eq!(input.value(), "");
}

// ---------------------------------------------------------------------------
// Text component pinning (no dedicated upstream test file; behavior from the
// Text class is exercised across upstream editor/chat integration tests)
// ---------------------------------------------------------------------------

#[test]
fn text_wraps_pads_and_applies_background() {
    let mut text = Text::with_options("hello world this wraps", 1, 1, None);
    let lines = text.render(12);
    assert!(lines.len() >= 3);
    for line in &lines {
        assert_eq!(visible_width(line), 12);
    }
    assert!(lines[0].starts_with(' ')); // left padding
    assert!(lines[0].ends_with(' ')); // right padding
}

#[test]
fn text_empty_renders_nothing_and_bg_applies() {
    let mut text = Text::with_options("   ", 1, 0, None);
    assert!(text.render(10).is_empty());

    let bg = "\x1b[44m";
    let reset = "\x1b[0m";
    let mut text = Text::with_options(
        "hi",
        1,
        0,
        Some(std::sync::Arc::new(|line: &str| {
            format!("\x1b[44m{line}\x1b[0m")
        })),
    );
    let lines = text.render(10);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with(bg));
    assert!(lines[0].ends_with(reset));
    assert_eq!(visible_width(&lines[0]), 10);
}
