//! Ports of the pure-function upstream unit tests in
//! `packages/tui/test/terminal-colors.test.ts` (the `parseOscColorResponse`
//! and `parseTerminalColorSchemeReport` describes; the TUI-integration blocks
//! are ported in `tui_tests.rs`).

use crate::tui::terminal_colors::{
    parse_osc_color_response, parse_terminal_color_scheme_report, OscColorTarget, RgbColor,
    TerminalColorScheme,
};

#[test]
fn parses_16_bit_osc11_rgb_responses() {
    assert_eq!(
        parse_osc_color_response("\x1b]11;rgb:0000/8000/ffff\x07"),
        Some((
            OscColorTarget::Background,
            Some(RgbColor {
                r: 0,
                g: 128,
                b: 255
            })
        ))
    );
}

#[test]
fn parses_osc11_hex_responses() {
    assert_eq!(
        parse_osc_color_response("\x1b]11;#ffffff\x1b\\"),
        Some((
            OscColorTarget::Background,
            Some(RgbColor {
                r: 255,
                g: 255,
                b: 255
            })
        ))
    );
    assert_eq!(
        parse_osc_color_response("\x1b]11;#000000\x07"),
        Some((
            OscColorTarget::Background,
            Some(RgbColor { r: 0, g: 0, b: 0 })
        ))
    );
}

#[test]
fn rejects_non_strict_osc_color_responses() {
    assert_eq!(parse_osc_color_response("x\x1b]11;#ffffff\x07"), None);
    assert_eq!(parse_osc_color_response("\x1b]11;#ffffff\x07x"), None);
}

/// Upstream `parses OSC 10, 11, and 4 replies`.
#[test]
fn parses_osc_10_11_and_4_replies() {
    assert_eq!(
        parse_osc_color_response("\x1b]10;rgb:ffff/ffff/ffff\x07"),
        Some((
            OscColorTarget::Foreground,
            Some(RgbColor {
                r: 255,
                g: 255,
                b: 255
            })
        ))
    );
    assert_eq!(
        parse_osc_color_response("\x1b]4;13;#ff0080\x1b\\"),
        Some((
            OscColorTarget::Index(13),
            Some(RgbColor {
                r: 255,
                g: 0,
                b: 128
            })
        ))
    );
    // A reply with an unparseable color still reports its target.
    assert_eq!(
        parse_osc_color_response("\x1b]4;1;bogus\x07"),
        Some((OscColorTarget::Index(1), None))
    );
    assert_eq!(parse_osc_color_response("\x1b]12;#ffffff\x07"), None);
}

#[test]
fn parses_color_scheme_reports() {
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n"),
        Some(TerminalColorScheme::Dark)
    );
    assert_eq!(
        parse_terminal_color_scheme_report("\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n"),
        Some(TerminalColorScheme::Light)
    );
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?997;3n"), None);
    assert_eq!(parse_terminal_color_scheme_report("\x1b[?996n"), None);
    assert_eq!(parse_terminal_color_scheme_report("x\x1b[?997;1n"), None);
}
