//! Ports of the pure-function upstream unit tests for `packages/tui/src/utils.ts`:
//! `test/truncate-to-width.test.ts`, `test/wrap-ansi.test.ts` and the
//! pure-function parts of `test/tab-width.test.ts`.

use crate::tui::utils::*;

#[test]
fn visible_width_counts_tabs_inline_and_skips_ansi_inline() {
    assert_eq!(visible_width("\t\x1b[31m界\x1b[0m"), 5);
}

#[test]
fn visible_width_counts_indic_conjunct_spacing_code_points_within_grapheme_clusters() {
    assert_eq!(visible_width("र्क"), 2);
    assert_eq!(visible_width("नेटवर्क"), 5);
    assert_eq!(visible_width("सर्वाधिकार सुरक्षित। ऑर्डर पर क्लिक करें"), 33);
    assert_eq!(visible_width("র্ক"), 2);
    assert_eq!(visible_width("ર્ક"), 2);
    assert_eq!(visible_width("ର୍କ"), 2);
    assert_eq!(visible_width("ర్క"), 2);
    assert_eq!(visible_width("ര്‍ക"), 2);
}

#[test]
fn visible_width_keeps_ordinary_combining_marks_zero_width() {
    assert_eq!(visible_width("e\u{301}"), 1);
    assert_eq!(visible_width("čřžůú"), 5);
    assert_eq!(visible_width("שָׁ"), 1);
    assert_eq!(visible_width("بّ"), 1);
    assert_eq!(visible_width("རྐ"), 1);
    assert_eq!(visible_width("ᜠ᜴"), 1);
    assert_eq!(visible_width("가〮"), 2);
    assert_eq!(visible_width("가〯"), 2);
}

#[test]
fn visible_width_keeps_cjk_and_japanese_width_accounting_unchanged() {
    assert_eq!(visible_width("网络"), 4);
    assert_eq!(visible_width("ネットワーク"), 12);
    assert_eq!(visible_width("が"), 2);
    assert_eq!(visible_width("か\u{3099}"), 2);
}

#[test]
fn visible_width_counts_myanmar_marks_that_terminals_allocate_cells_for() {
    assert_eq!(visible_width("ကာ"), 2);
    assert_eq!(visible_width("ကေ"), 2);
    assert_eq!(visible_width("က်"), 2);
    assert_eq!(visible_width("ကျ"), 2);
    assert_eq!(visible_width("ကြ"), 2);
    assert_eq!(visible_width("ကဳ"), 2);
    assert_eq!(visible_width("ကဴ"), 2);
    assert_eq!(visible_width("ကဵ"), 2);
    assert_eq!(visible_width("ကး"), 2);
    assert_eq!(visible_width("ကို"), 1);
    assert_eq!(visible_width("က္"), 1);
}

#[test]
fn visible_width_keeps_thai_and_lao_am_clusters_at_their_normal_cell_width() {
    assert_eq!(visible_width("ำ"), 1);
    assert_eq!(visible_width("ຳ"), 1);
    assert_eq!(visible_width("กำ"), 2);
    assert_eq!(visible_width("ກຳ"), 2);
}

#[test]
fn normalize_terminal_output_normalizes_thai_and_lao_am_vowels_only_for_output() {
    assert_eq!(normalize_terminal_output("ำ"), "\u{e4d}\u{e32}");
    assert_eq!(normalize_terminal_output("ຳ"), "\u{ecd}\u{eb2}");
    assert_eq!(
        visible_width(&normalize_terminal_output("ำabc")),
        visible_width("ำabc")
    );
    assert_eq!(
        visible_width(&normalize_terminal_output("ຳabc")),
        visible_width("ຳabc")
    );
}

#[test]
fn truncate_keeps_output_within_width_for_very_large_unicode_input() {
    let text = "🙂界".repeat(100_000);
    let truncated = truncate_to_width(&text, 40, "…", false);

    assert!(visible_width(&truncated) <= 40);
    assert!(truncated.ends_with("…\x1b[0m"));
}

#[test]
fn truncate_preserves_ansi_styling_and_resets_before_and_after_ellipsis() {
    let text = format!("\x1b[31m{}\x1b[0m", "hello ".repeat(1000));
    let truncated = truncate_to_width(&text, 20, "…", false);

    assert!(visible_width(&truncated) <= 20);
    assert!(truncated.contains("\x1b[31m"));
    assert!(truncated.ends_with("\x1b[0m…\x1b[0m"));
}

#[test]
fn truncate_closes_a_bel_terminated_osc8_link_when_truncating_its_label() {
    let open = "\x1b]8;;https://example.com\x07";
    let close = "\x1b]8;;\x07";
    let text = format!("{open}some-longer-label-here{close}");

    assert_eq!(
        truncate_to_width(&text, 15, "...", false),
        format!("{open}some-longer-{close}\x1b[0m...\x1b[0m")
    );
}

#[test]
fn truncate_handles_malformed_ansi_escape_prefixes_without_hanging() {
    let text = format!("abc\x1bnot-ansi {}", "🙂".repeat(1000));
    let truncated = truncate_to_width(&text, 20, "…", false);

    assert!(visible_width(&truncated) <= 20);
}

#[test]
fn truncate_clips_wide_ellipsis_safely_and_brackets_it_with_resets() {
    assert_eq!(truncate_to_width("abcdef", 1, "🙂", false), "");
    assert_eq!(
        truncate_to_width("abcdef", 2, "🙂", false),
        "\x1b[0m🙂\x1b[0m"
    );
    assert!(visible_width(&truncate_to_width("abcdef", 2, "🙂", false)) <= 2);
}

#[test]
fn truncate_returns_the_original_text_when_it_already_fits_even_if_ellipsis_is_too_wide() {
    assert_eq!(truncate_to_width("a", 2, "🙂", false), "a");
    assert_eq!(truncate_to_width("界", 2, "🙂", false), "界");
}

#[test]
fn truncate_pads_truncated_output_to_requested_width() {
    let truncated = truncate_to_width("🙂界🙂界🙂界", 8, "…", true);
    assert_eq!(visible_width(&truncated), 8);
}

#[test]
fn truncate_adds_a_trailing_reset_when_truncating_without_an_ellipsis() {
    let truncated = truncate_to_width(&format!("\x1b[31m{}", "hello".repeat(100)), 10, "", false);
    assert!(visible_width(&truncated) <= 10);
    assert!(truncated.ends_with("\x1b[0m"));
}

#[test]
fn truncate_keeps_a_contiguous_prefix_instead_of_skipping_a_wide_grapheme() {
    let truncated = truncate_to_width("🙂\t界 \x1b_abc\x07", 7, "…", true);
    assert_eq!(truncated, "🙂\t\x1b[0m…\x1b[0m ");
}

#[test]
fn wrap_underline_should_not_apply_underline_style_before_the_styled_text() {
    let underline_on = "\x1b[4m";
    let underline_off = "\x1b[24m";
    let url = "https://example.com/very/long/path/that/will/wrap";
    let text = format!("read this thread {underline_on}{url}{underline_off}");

    let wrapped = wrap_text_with_ansi(&text, 40);

    assert_eq!(wrapped[0], "read this thread");
    assert!(wrapped[1].starts_with(underline_on));
    assert!(wrapped[1].contains("https://"));
}

#[test]
fn wrap_underline_should_not_have_whitespace_before_underline_reset_code() {
    let underline_on = "\x1b[4m";
    let underline_off = "\x1b[24m";
    let text = format!("{underline_on}underlined text here {underline_off}more");

    let wrapped = wrap_text_with_ansi(&text, 18);

    assert!(!wrapped[0].contains(&format!(" {underline_off}")));
}

#[test]
fn wrap_underline_should_not_bleed_to_padding() {
    let underline_on = "\x1b[4m";
    let underline_off = "\x1b[24m";
    let url = "https://example.com/very/long/path/that/will/definitely/wrap";
    let text = format!("prefix {underline_on}{url}{underline_off} suffix");

    let wrapped = wrap_text_with_ansi(&text, 30);

    for line in wrapped.iter().skip(1).take(wrapped.len().saturating_sub(2)) {
        if line.contains(underline_on) {
            assert!(line.ends_with(underline_off));
            assert!(!line.ends_with("\x1b[0m"));
        }
    }
}

#[test]
fn wrap_background_should_preserve_background_color_across_wrapped_lines() {
    let bg_blue = "\x1b[44m";
    let reset = "\x1b[0m";
    let text = format!("{bg_blue}hello world this is blue background text{reset}");

    let wrapped = wrap_text_with_ansi(&text, 15);

    for line in &wrapped {
        assert!(line.contains(bg_blue));
    }
    for line in wrapped.iter().take(wrapped.len() - 1) {
        assert!(!line.ends_with(reset));
    }
}

#[test]
fn wrap_background_should_reset_underline_but_preserve_background() {
    let underline_on = "\x1b[4m";
    let underline_off = "\x1b[24m";
    let reset = "\x1b[0m";

    let text = format!(
        "\x1b[41mprefix {underline_on}UNDERLINED_CONTENT_THAT_WRAPS{underline_off} suffix{reset}"
    );

    let wrapped = wrap_text_with_ansi(&text, 20);

    for line in &wrapped {
        let has_bg_color = line.contains("[41m") || line.contains(";41m") || line.contains("[41;");
        assert!(has_bg_color);
    }

    for line in wrapped.iter().take(wrapped.len() - 1) {
        let has_underline = (line.contains("[4m") || line.contains("[4;") || line.contains(";4m"))
            && !line.contains(underline_off);
        if has_underline {
            assert!(line.ends_with(underline_off));
            assert!(!line.ends_with(reset));
        }
    }
}

#[test]
fn wrap_should_handle_lf_crlf_and_cr_line_endings() {
    assert_eq!(
        wrap_text_with_ansi("first\nsecond\r\nthird\rfourth", 80),
        vec!["first", "second", "third", "fourth"]
    );
}

#[test]
fn wrap_should_preserve_ansi_state_across_crlf_and_cr_line_endings() {
    assert_eq!(
        wrap_text_with_ansi("\x1b[31mfirst\r\nsecond\rthird\x1b[0m", 80),
        vec!["\x1b[31mfirst", "\x1b[31msecond", "\x1b[31mthird\x1b[0m"]
    );
}

#[test]
fn wrap_should_wrap_plain_text_correctly() {
    let wrapped = wrap_text_with_ansi("hello world this is a test", 10);

    assert!(wrapped.len() > 1);
    for line in &wrapped {
        assert!(visible_width(line) <= 10);
    }
}

#[test]
fn wrap_should_break_cjk_runs_at_grapheme_boundaries_after_latin_text() {
    let text = "This is an example 中文汉字测试段落内容中文汉字测试段落内容.";
    let wrapped = wrap_text_with_ansi(text, 40);

    assert_eq!(
        wrapped,
        vec![
            "This is an example 中文汉字测试段落内容",
            "中文汉字测试段落内容."
        ]
    );
    for line in &wrapped {
        assert!(visible_width(line) <= 40);
    }
}

#[test]
fn wrap_should_preserve_color_codes_when_wrapping_cjk_runs() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let text = format!("{red}This is an example 中文汉字测试段落内容中文汉字测试段落内容.{reset}");
    let wrapped = wrap_text_with_ansi(&text, 40);

    assert_eq!(wrapped.len(), 2);
    assert_eq!(
        wrapped[0],
        format!("{red}This is an example 中文汉字测试段落内容")
    );
    assert_eq!(wrapped[1], format!("{red}中文汉字测试段落内容.{reset}"));
    for line in &wrapped {
        assert!(visible_width(line) <= 40);
    }
}

#[test]
fn wrap_should_ignore_osc_133_semantic_markers_in_visible_width() {
    assert_eq!(visible_width("\x1b]133;A\x07hello\x1b]133;B\x07"), 5);
}

#[test]
fn wrap_should_ignore_osc_sequences_terminated_with_st_in_visible_width() {
    assert_eq!(visible_width("\x1b]133;A\x1b\\hello\x1b]133;B\x1b\\"), 5);
}

#[test]
fn wrap_should_treat_isolated_regional_indicators_as_width_2() {
    assert_eq!(visible_width("🇨"), 2);
    assert_eq!(visible_width("🇨🇳"), 2);
}

#[test]
fn wrap_should_truncate_trailing_whitespace_that_exceeds_width() {
    let wrapped = wrap_text_with_ansi("  ", 1);
    assert!(visible_width(&wrapped[0]) <= 1);
}

#[test]
fn wrap_should_preserve_color_codes_across_wraps() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let text = format!("{red}hello world this is red{reset}");

    let wrapped = wrap_text_with_ansi(&text, 10);

    for line in wrapped.iter().skip(1) {
        assert!(line.starts_with(red));
    }
    for line in wrapped.iter().take(wrapped.len() - 1) {
        assert!(!line.ends_with(reset));
    }
}

#[test]
fn wrap_osc8_re_emits_open_at_the_start_of_continuation_lines() {
    let url = "https://example.com";
    let input = format!("\x1b]8;;{url}\x1b\\0123456789\x1b]8;;\x1b\\");
    let lines = wrap_text_with_ansi(&input, 6);

    for line in &lines {
        let stripped = strip_terminal_sequences(line);
        let stripped = strip_osc8(&stripped, url);
        if !stripped.trim().is_empty() {
            assert!(
                line.starts_with(&format!("\x1b]8;;{url}\x1b\\"))
                    || line.contains(&format!("\x1b]8;;{url}\x1b\\")),
                "Line {line:?} has visible text but no OSC 8 re-open"
            );
        }
    }
}

/// Second-stage strip for the upstream test's OSC 8 removal (BEL or ST form).
fn strip_osc8(line: &str, url: &str) -> String {
    let mut result = String::new();
    let mut rest = line;
    while let Some(pos) = rest.find("\x1b]8;;") {
        result.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let end = after
            .find('\x07')
            .map(|p| p + 1)
            .or_else(|| after.find("\x1b\\").map(|p| p + 2));
        let Some(end) = end else { break };
        rest = &after[end..];
    }
    result.push_str(rest);
    let _ = url;
    result
}

#[test]
fn wrap_osc8_closes_before_each_line_break() {
    let url = "https://example.com";
    let input = format!("\x1b]8;;{url}\x1b\\0123456789\x1b]8;;\x1b\\");
    let lines = wrap_text_with_ansi(&input, 6);

    for line in lines.iter().take(lines.len() - 1) {
        if line.contains(&format!("\x1b]8;;{url}\x1b\\")) {
            assert!(
                line.ends_with("\x1b]8;;\x1b\\"),
                "Non-final line {line:?} is inside a hyperlink but does not close it"
            );
        }
    }
}

#[test]
fn wrap_osc8_preserves_bel_terminators_when_wrapping_oauth_style_hyperlinks() {
    let url = format!("https://example.com/oauth/{}", "a".repeat(32));
    let input = format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07");
    let lines = wrap_text_with_ansi(&input, 20);

    assert!(lines.len() > 1);
    for line in &lines {
        assert!(
            line.contains(&format!("\x1b]8;;{url}\x07")),
            "Line {line:?} does not reopen the hyperlink with BEL"
        );
        assert!(
            !line.contains(&format!("\x1b]8;;{url}\x1b\\")),
            "Line {line:?} reopens the hyperlink with ST"
        );
    }
    for line in lines.iter().take(lines.len() - 1) {
        assert!(
            line.ends_with("\x1b]8;;\x07"),
            "Line {line:?} does not close the hyperlink with BEL"
        );
    }
}

#[test]
fn wrap_osc8_does_not_emit_sequences_on_lines_outside_the_hyperlink() {
    let url = "https://example.com";
    let input = format!("before \x1b]8;;{url}\x1b\\link\x1b]8;;\x1b\\ after");
    let lines = wrap_text_with_ansi(&input, 80);

    assert_eq!(lines.len(), 1);
    let open_count = lines[0].matches(&format!("\x1b]8;;{url}\x1b\\")).count();
    let close_count = lines[0].matches("\x1b]8;;\x1b\\").count();
    assert_eq!(open_count, 1);
    assert_eq!(close_count, 1);
}

#[test]
fn tab_width_keeps_slice_helper_widths_consistent_with_visible_width() {
    let text = "out 192M\t.pi/skill-tests/results-ha";
    let (sliced, width) = slice_with_width(text, 0, 10, true);

    assert_eq!(sliced, "out 192M");
    assert_eq!(width, 8);
    assert_eq!(visible_width(&sliced), width);
}

#[test]
fn tab_width_keeps_overlay_segment_widths_consistent_with_visible_width() {
    let text = "out 192M\t.pi/skill-tests/results-ha";
    let segments = extract_segments(text, 10, 13, 10, true);

    assert_eq!(segments.before, "out 192M");
    assert_eq!(segments.before_width, 8);
    assert_eq!(visible_width(&segments.before), segments.before_width);

    let tab_fits = extract_segments(text, 11, 13, 10, true);
    assert_eq!(tab_fits.before, "out 192M\t");
    assert_eq!(tab_fits.before_width, 11);
    assert_eq!(visible_width(&tab_fits.before), tab_fits.before_width);
}

#[test]
fn tab_width_keeps_tabs_inside_terminal_control_sequences_byte_identical() {
    let control_sequences = [
        "\x1b]8;;https://example.test/a\tb\x07",
        "\x1b]0;window\ttitle\x1b\\",
        "\x1b_payload\tdata\x1b\\",
    ];

    for control_sequence in control_sequences {
        assert_eq!(
            normalize_terminal_output(&format!("{control_sequence}label\ttext")),
            format!("{control_sequence}label   text")
        );
    }
}
