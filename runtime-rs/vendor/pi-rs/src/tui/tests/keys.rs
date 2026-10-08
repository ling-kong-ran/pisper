//! Ports of upstream `packages/tui/test/keys.test.ts`.
//!
//! The upstream suite runs serially in one JS process; the Rust tests share a
//! global Kitty-protocol flag and process environment, so every test takes a
//! process-wide lock to preserve the same serialized semantics.

use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::tui::keys::*;

fn keys_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn kitty_alternate_keys_match_ctrl_letters_with_base_layout_key() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    // Cyrillic 'с' = 1089, Latin 'c' = 99: CSI 1089::99;5u
    assert!(matches_key("\x1b[1089::99;5u", "ctrl+c"));
    assert!(matches_key("\x1b[1074::100;5u", "ctrl+d"));
    assert!(matches_key("\x1b[1103::122;5u", "ctrl+z"));
    assert!(matches_key("\x1b[1079::112;6u", "ctrl+shift+p"));
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_still_matches_direct_codepoint_without_base_layout_key() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[99;5u", "ctrl+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_matches_super_modified_bindings_including_combined_modifiers() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[107;9u", "super+k"));
    assert!(matches_key("\x1b[13;9u", "super+enter"));
    assert!(matches_key("\x1b[107;13u", "ctrl+super+k"));
    assert!(matches_key("\x1b[107;13u", "ctrl+super+k"));
    assert!(matches_key("\x1b[107;14u", "ctrl+shift+super+k"));
    assert!(!matches_key("\x1b[107;13u", "super+k"));
    assert_eq!(parse_key("\x1b[107;9u").as_deref(), Some("super+k"));
    assert_eq!(parse_key("\x1b[13;9u").as_deref(), Some("super+enter"));
    assert_eq!(parse_key("\x1b[107;13u").as_deref(), Some("ctrl+super+k"));
    assert_eq!(
        parse_key("\x1b[107;14u").as_deref(),
        Some("shift+ctrl+super+k")
    );
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_matches_digit_bindings_via_csi_u() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[49u", "1"));
    assert!(matches_key("\x1b[49;5u", "ctrl+1"));
    assert!(!matches_key("\x1b[49;5u", "ctrl+2"));
    assert_eq!(parse_key("\x1b[49u").as_deref(), Some("1"));
    assert_eq!(parse_key("\x1b[49;5u").as_deref(), Some("ctrl+1"));
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_normalizes_keypad_functional_keys() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert!(matches_key("\x1b[57400u", "1"));
    assert!(matches_key("\x1b[57410u", "/"));
    assert!(matches_key("\x1b[57417u", "left"));
    assert!(matches_key("\x1b[57426u", "delete"));
    assert_eq!(parse_key("\x1b[57399u").as_deref(), Some("0"));
    assert_eq!(parse_key("\x1b[57409u").as_deref(), Some("."));
    assert_eq!(parse_key("\x1b[57413u").as_deref(), Some("+"));
    assert_eq!(parse_key("\x1b[57416u").as_deref(), Some(","));
    assert_eq!(parse_key("\x1b[57417u").as_deref(), Some("left"));
    assert_eq!(parse_key("\x1b[57418u").as_deref(), Some("right"));
    assert_eq!(parse_key("\x1b[57419u").as_deref(), Some("up"));
    assert_eq!(parse_key("\x1b[57420u").as_deref(), Some("down"));
    assert_eq!(parse_key("\x1b[57421u").as_deref(), Some("pageUp"));
    assert_eq!(parse_key("\x1b[57422u").as_deref(), Some("pageDown"));
    assert_eq!(parse_key("\x1b[57423u").as_deref(), Some("home"));
    assert_eq!(parse_key("\x1b[57424u").as_deref(), Some("end"));
    assert_eq!(parse_key("\x1b[57425u").as_deref(), Some("insert"));
    assert_eq!(parse_key("\x1b[57426u").as_deref(), Some("delete"));
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_handles_shifted_key_event_type_and_full_format() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    // shift modifier = 1, +1 = 2
    assert!(matches_key("\x1b[99:67:99;2u", "shift+c"));
    // Cyrillic ctrl+c release event (event type 3)
    assert!(matches_key("\x1b[1089::99;5:3u", "ctrl+c"));
    // Cyrillic 'с' = 1089, Cyrillic 'С' = 1057, Latin 'c' = 99; ctrl+shift, repeat
    assert!(matches_key("\x1b[1089:1057:99;6:2u", "ctrl+shift+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn kitty_prefers_codepoint_for_latin_letters_and_symbols_over_base_layout() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    // Dvorak Ctrl+K reports codepoint 'k' (107) and base layout 'v' (118).
    assert!(matches_key("\x1b[107::118;5u", "ctrl+k"));
    assert!(!matches_key("\x1b[107::118;5u", "ctrl+v"));
    // Dvorak Ctrl+/ reports codepoint '/' (47) and base layout '[' (91).
    assert!(matches_key("\x1b[47::91;5u", "ctrl+/"));
    assert!(!matches_key("\x1b[47::91;5u", "ctrl+["));
    // Cyrillic ctrl+с with base 'c' should NOT match ctrl+d or ctrl+shift+c.
    assert!(!matches_key("\x1b[1089::99;5u", "ctrl+d"));
    assert!(!matches_key("\x1b[1089::99;5u", "ctrl+shift+c"));
    set_kitty_protocol_active(false);
}

#[test]
fn modify_other_keys_matching() {
    let _guard = keys_lock();
    set_kitty_protocol_active(false);

    assert!(matches_key("\x1b[27;5;99~", "ctrl+c"));
    assert_eq!(parse_key("\x1b[27;5;99~").as_deref(), Some("ctrl+c"));
    assert!(matches_key("\x1b[27;5;100~", "ctrl+d"));
    assert_eq!(parse_key("\x1b[27;5;100~").as_deref(), Some("ctrl+d"));
    assert!(matches_key("\x1b[27;5;122~", "ctrl+z"));
    assert_eq!(parse_key("\x1b[27;5;122~").as_deref(), Some("ctrl+z"));

    assert!(matches_key("\x1b[27;5;13~", "ctrl+enter"));
    assert!(matches_key("\x1b[27;2;13~", "shift+enter"));
    assert!(matches_key("\x1b[27;3;13~", "alt+enter"));
    assert_eq!(parse_key("\x1b[27;5;13~").as_deref(), Some("ctrl+enter"));
    assert_eq!(parse_key("\x1b[27;2;13~").as_deref(), Some("shift+enter"));
    assert_eq!(parse_key("\x1b[27;3;13~").as_deref(), Some("alt+enter"));

    assert!(matches_key("\x1b[27;2;9~", "shift+tab"));
    assert!(matches_key("\x1b[27;5;9~", "ctrl+tab"));
    assert!(matches_key("\x1b[27;3;9~", "alt+tab"));
    assert_eq!(parse_key("\x1b[27;2;9~").as_deref(), Some("shift+tab"));
    assert_eq!(parse_key("\x1b[27;5;9~").as_deref(), Some("ctrl+tab"));
    assert_eq!(parse_key("\x1b[27;3;9~").as_deref(), Some("alt+tab"));

    assert!(matches_key("\x1b[27;1;127~", "backspace"));
    assert!(matches_key("\x1b[27;5;127~", "ctrl+backspace"));
    assert!(matches_key("\x1b[27;3;127~", "alt+backspace"));
    assert_eq!(parse_key("\x1b[27;1;127~").as_deref(), Some("backspace"));
    assert_eq!(
        parse_key("\x1b[27;5;127~").as_deref(),
        Some("ctrl+backspace")
    );
    assert_eq!(
        parse_key("\x1b[27;3;127~").as_deref(),
        Some("alt+backspace")
    );

    assert!(matches_key("\x1b[27;1;27~", "escape"));
    assert_eq!(parse_key("\x1b[27;1;27~").as_deref(), Some("escape"));

    assert!(matches_key("\x1b[27;1;32~", "space"));
    assert!(matches_key("\x1b[27;5;32~", "ctrl+space"));
    assert_eq!(parse_key("\x1b[27;1;32~").as_deref(), Some("space"));
    assert_eq!(parse_key("\x1b[27;5;32~").as_deref(), Some("ctrl+space"));

    assert!(matches_key("\x1b[27;5;47~", "ctrl+/"));
    assert_eq!(parse_key("\x1b[27;5;47~").as_deref(), Some("ctrl+/"));

    assert!(matches_key("\x1b[27;5;49~", "ctrl+1"));
    assert!(matches_key("\x1b[27;2;49~", "shift+1"));
    assert_eq!(parse_key("\x1b[27;5;49~").as_deref(), Some("ctrl+1"));
    assert_eq!(parse_key("\x1b[27;2;49~").as_deref(), Some("shift+1"));

    assert!(matches_key("\x1b[27;2;69~", "shift+e"));
    assert!(matches_key("\x1b[27;6;69~", "ctrl+shift+e"));
    assert_eq!(parse_key("\x1b[27;2;69~").as_deref(), Some("shift+e"));
    assert_eq!(parse_key("\x1b[27;6;69~").as_deref(), Some("shift+ctrl+e"));

    assert!(matches_key("\x1b[104;7u", "ctrl+alt+h"));
    assert_eq!(parse_key("\x1b[104;7u").as_deref(), Some("ctrl+alt+h"));

    assert!(matches_key("\x1b[27;7;104~", "ctrl+alt+h"));
    assert_eq!(parse_key("\x1b[27;7;104~").as_deref(), Some("ctrl+alt+h"));
}

#[test]
fn legacy_key_matching_basics() {
    let _guard = keys_lock();
    set_kitty_protocol_active(false);

    assert!(matches_key("\x03", "ctrl+c"));
    assert!(matches_key("\x04", "ctrl+d"));
    assert!(matches_key("\x1b", "escape"));

    assert!(matches_key("\n", "enter"));
    assert_eq!(parse_key("\n").as_deref(), Some("enter"));

    assert!(matches_key("\x00", "ctrl+space"));
    assert_eq!(parse_key("\x00").as_deref(), Some("ctrl+space"));

    // Ctrl+\ sends 28, Ctrl+] sends 29, Ctrl+_ (and Ctrl+-) send 31.
    assert!(matches_key("\x1c", "ctrl+\\"));
    assert_eq!(parse_key("\x1c").as_deref(), Some("ctrl+\\"));
    assert!(matches_key("\x1d", "ctrl+]"));
    assert_eq!(parse_key("\x1d").as_deref(), Some("ctrl+]"));
    assert!(matches_key("\x1f", "ctrl+_"));
    assert!(matches_key("\x1f", "ctrl+-"));
    assert_eq!(parse_key("\x1f").as_deref(), Some("ctrl+-"));

    // Ctrl+Alt+symbol sends ESC followed by the control character.
    assert!(matches_key("\x1b\x1b", "ctrl+alt+["));
    assert_eq!(parse_key("\x1b\x1b").as_deref(), Some("ctrl+alt+["));
    assert!(matches_key("\x1b\x1c", "ctrl+alt+\\"));
    assert_eq!(parse_key("\x1b\x1c").as_deref(), Some("ctrl+alt+\\"));
    assert!(matches_key("\x1b\x1d", "ctrl+alt+]"));
    assert_eq!(parse_key("\x1b\x1d").as_deref(), Some("ctrl+alt+]"));
    assert!(matches_key("\x1b\x1f", "ctrl+alt+_"));
    assert!(matches_key("\x1b\x1f", "ctrl+alt+-"));
    assert_eq!(parse_key("\x1b\x1f").as_deref(), Some("ctrl+alt+-"));
}

#[test]
fn kitty_active_treats_linefeed_as_shift_enter() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert!(matches_key("\n", "shift+enter"));
    assert!(!matches_key("\n", "enter"));
    assert_eq!(parse_key("\n").as_deref(), Some("shift+enter"));
    set_kitty_protocol_active(false);
}

/// The three upstream raw-0x08 environment scenarios, serialized in one test
/// because they mutate the process environment.
#[test]
fn raw_0x08_backspace_depends_on_windows_terminal_environment() {
    let _guard = keys_lock();
    set_kitty_protocol_active(false);

    // Outside Windows Terminal (WT_SESSION unset).
    std::env::remove_var("WT_SESSION");
    assert!(matches_key("\x7f", "backspace"));
    assert!(!matches_key("\x7f", "ctrl+backspace"));
    assert_eq!(parse_key("\x7f").as_deref(), Some("backspace"));
    assert!(matches_key("\x08", "backspace"));
    assert!(!matches_key("\x08", "ctrl+backspace"));
    assert_eq!(parse_key("\x08").as_deref(), Some("backspace"));
    assert!(matches_key("\x08", "ctrl+h"));

    // Local Windows Terminal (WT_SESSION set, no SSH variables).
    std::env::set_var("WT_SESSION", "test-session");
    std::env::remove_var("SSH_CONNECTION");
    std::env::remove_var("SSH_CLIENT");
    std::env::remove_var("SSH_TTY");
    assert!(matches_key("\x08", "ctrl+backspace"));
    assert!(!matches_key("\x08", "backspace"));
    assert_eq!(parse_key("\x08").as_deref(), Some("ctrl+backspace"));
    assert!(matches_key("\x08", "ctrl+h"));

    // Windows Terminal over SSH.
    std::env::set_var("WT_SESSION", "test-session");
    std::env::set_var("SSH_CONNECTION", "1 2 3 4");
    std::env::set_var("SSH_CLIENT", "1 2 3");
    std::env::set_var("SSH_TTY", "/dev/pts/1");
    assert!(!matches_key("\x08", "ctrl+backspace"));
    assert!(matches_key("\x08", "backspace"));
    assert_eq!(parse_key("\x08").as_deref(), Some("backspace"));
    assert!(matches_key("\x08", "ctrl+h"));

    std::env::remove_var("WT_SESSION");
    std::env::remove_var("SSH_CONNECTION");
    std::env::remove_var("SSH_CLIENT");
    std::env::remove_var("SSH_TTY");
}

#[test]
fn legacy_alt_prefixed_sequences_depend_on_kitty_state() {
    let _guard = keys_lock();

    set_kitty_protocol_active(false);
    assert!(matches_key("\x1b ", "alt+space"));
    assert_eq!(parse_key("\x1b ").as_deref(), Some("alt+space"));
    assert!(matches_key("\x1b\x08", "alt+backspace"));
    assert_eq!(parse_key("\x1b\x08").as_deref(), Some("alt+backspace"));
    assert!(matches_key("\x1b\x03", "ctrl+alt+c"));
    assert_eq!(parse_key("\x1b\x03").as_deref(), Some("ctrl+alt+c"));
    assert!(matches_key("\x1bB", "alt+left"));
    assert_eq!(parse_key("\x1bB").as_deref(), Some("alt+left"));
    assert!(matches_key("\x1bF", "alt+right"));
    assert_eq!(parse_key("\x1bF").as_deref(), Some("alt+right"));
    for (data, key) in [
        ("\x1ba", "alt+a"),
        ("\x1b1", "alt+1"),
        ("\x1b,", "alt+,"),
        ("\x1b.", "alt+."),
        ("\x1by", "alt+y"),
        ("\x1bz", "alt+z"),
    ] {
        assert!(matches_key(data, key), "{data:?} should match {key}");
        assert_eq!(
            parse_key(data).as_deref(),
            Some(key),
            "{data:?} should parse as {key}"
        );
    }

    set_kitty_protocol_active(true);
    assert!(!matches_key("\x1b ", "alt+space"));
    assert_eq!(parse_key("\x1b "), None);
    assert!(matches_key("\x1b\x08", "alt+backspace"));
    assert_eq!(parse_key("\x1b\x08").as_deref(), Some("alt+backspace"));
    assert!(!matches_key("\x1b\x03", "ctrl+alt+c"));
    assert_eq!(parse_key("\x1b\x03"), None);
    assert!(!matches_key("\x1bB", "alt+left"));
    assert_eq!(parse_key("\x1bB"), None);
    assert!(!matches_key("\x1bF", "alt+right"));
    assert_eq!(parse_key("\x1bF"), None);
    for (data, key) in [
        ("\x1ba", "alt+a"),
        ("\x1b1", "alt+1"),
        ("\x1b,", "alt+,"),
        ("\x1b.", "alt+."),
        ("\x1by", "alt+y"),
        ("\x1bz", "alt+z"),
    ] {
        assert!(
            !matches_key(data, key),
            "{data:?} should not match {key} with kitty active"
        );
        assert_eq!(
            parse_key(data),
            None,
            "{data:?} should not parse with kitty active"
        );
    }
    set_kitty_protocol_active(false);
}

#[test]
fn legacy_arrow_ss3_function_and_modifier_sequences() {
    let _guard = keys_lock();
    set_kitty_protocol_active(false);

    assert!(matches_key("\x1b[A", "up"));
    assert!(matches_key("\x1b[B", "down"));
    assert!(matches_key("\x1b[C", "right"));
    assert!(matches_key("\x1b[D", "left"));

    assert!(matches_key("\x1bOA", "up"));
    assert!(matches_key("\x1bOB", "down"));
    assert!(matches_key("\x1bOC", "right"));
    assert!(matches_key("\x1bOD", "left"));
    assert!(matches_key("\x1bOH", "home"));
    assert!(matches_key("\x1bOF", "end"));

    assert!(matches_key("\x1b[1;5H", "ctrl+home"));
    assert!(matches_key("\x1b[1;5F", "ctrl+end"));
    assert!(matches_key("\x1b[5;5~", "ctrl+pageUp"));
    assert!(matches_key("\x1b[6;5~", "ctrl+pageDown"));
    assert_eq!(parse_key("\x1b[1;5H").as_deref(), Some("ctrl+home"));
    assert_eq!(parse_key("\x1b[1;5F").as_deref(), Some("ctrl+end"));
    assert_eq!(parse_key("\x1b[5;5~").as_deref(), Some("ctrl+pageUp"));
    assert_eq!(parse_key("\x1b[6;5~").as_deref(), Some("ctrl+pageDown"));

    assert!(matches_key("\x1bOP", "f1"));
    assert!(matches_key("\x1b[24~", "f12"));
    assert!(matches_key("\x1b[E", "clear"));

    assert!(matches_key("\x1bp", "alt+up"));
    assert!(!matches_key("\x1bp", "up"));

    // rxvt modifier sequences.
    assert!(matches_key("\x1b[a", "shift+up"));
    assert!(matches_key("\x1bOa", "ctrl+up"));
    assert!(matches_key("\x1b[2$", "shift+insert"));
    assert!(matches_key("\x1b[2^", "ctrl+insert"));
    assert!(matches_key("\x1b[7$", "shift+home"));
}

#[test]
fn decode_kitty_printable_keypad_keys() {
    assert_eq!(decode_kitty_printable("\x1b[57399u").as_deref(), Some("0"));
    assert_eq!(decode_kitty_printable("\x1b[57400u").as_deref(), Some("1"));
    assert_eq!(decode_kitty_printable("\x1b[57409u").as_deref(), Some("."));
    assert_eq!(decode_kitty_printable("\x1b[57410u").as_deref(), Some("/"));
    assert_eq!(decode_kitty_printable("\x1b[57411u").as_deref(), Some("*"));
    assert_eq!(decode_kitty_printable("\x1b[57412u").as_deref(), Some("-"));
    assert_eq!(decode_kitty_printable("\x1b[57413u").as_deref(), Some("+"));
    assert_eq!(decode_kitty_printable("\x1b[57415u").as_deref(), Some("="));
    assert_eq!(decode_kitty_printable("\x1b[57416u").as_deref(), Some(","));
    assert_eq!(decode_kitty_printable("\x1b[57417u"), None);
}

#[test]
fn decode_printable_key_modify_other_keys_sequences() {
    assert_eq!(decode_printable_key("\x1b[27;2;69~").as_deref(), Some("E"));
    assert_eq!(decode_printable_key("\x1b[27;2;196~").as_deref(), Some("Ä"));
    assert_eq!(decode_printable_key("\x1b[27;2;32~").as_deref(), Some(" "));
    assert_eq!(decode_printable_key("\x1b[27;2;13~"), None);
    assert_eq!(decode_printable_key("\x1b[27;6;69~"), None);
}

#[test]
fn parse_key_kitty_alternate_keys_and_modifiers() {
    let _guard = keys_lock();
    set_kitty_protocol_active(true);
    assert_eq!(parse_key("\x1b[1089::99;5u").as_deref(), Some("ctrl+c"));
    assert_eq!(parse_key("\x1b[107::118;5u").as_deref(), Some("ctrl+k"));
    assert_eq!(parse_key("\x1b[47::91;5u").as_deref(), Some("ctrl+/"));
    assert_eq!(parse_key("\x1b[99;5u").as_deref(), Some("ctrl+c"));
    assert!(matches_key("\x1b[69;2u", "shift+e"));
    assert_eq!(parse_key("\x1b[69;2u").as_deref(), Some("shift+e"));
    assert_eq!(parse_key("\x1b[99;17u"), None);
    set_kitty_protocol_active(false);
}

#[test]
fn parse_key_legacy_sequences() {
    let _guard = keys_lock();
    set_kitty_protocol_active(false);

    assert_eq!(parse_key("\x03").as_deref(), Some("ctrl+c"));
    assert_eq!(parse_key("\x04").as_deref(), Some("ctrl+d"));

    assert_eq!(parse_key("\x1b").as_deref(), Some("escape"));
    assert_eq!(parse_key("\t").as_deref(), Some("tab"));
    assert_eq!(parse_key("\r").as_deref(), Some("enter"));
    assert_eq!(parse_key("\n").as_deref(), Some("enter"));
    assert_eq!(parse_key("\x00").as_deref(), Some("ctrl+space"));
    assert_eq!(parse_key(" ").as_deref(), Some("space"));
    assert_eq!(parse_key("1").as_deref(), Some("1"));
    assert!(matches_key("1", "1"));

    assert_eq!(parse_key("\x1b[A").as_deref(), Some("up"));
    assert_eq!(parse_key("\x1b[B").as_deref(), Some("down"));
    assert_eq!(parse_key("\x1b[C").as_deref(), Some("right"));
    assert_eq!(parse_key("\x1b[D").as_deref(), Some("left"));

    assert_eq!(parse_key("\x1bOA").as_deref(), Some("up"));
    assert_eq!(parse_key("\x1bOB").as_deref(), Some("down"));
    assert_eq!(parse_key("\x1bOC").as_deref(), Some("right"));
    assert_eq!(parse_key("\x1bOD").as_deref(), Some("left"));
    assert_eq!(parse_key("\x1bOH").as_deref(), Some("home"));
    assert_eq!(parse_key("\x1bOF").as_deref(), Some("end"));

    assert_eq!(parse_key("\x1bOP").as_deref(), Some("f1"));
    assert_eq!(parse_key("\x1b[24~").as_deref(), Some("f12"));
    assert_eq!(parse_key("\x1b[E").as_deref(), Some("clear"));
    assert_eq!(parse_key("\x1b[2^").as_deref(), Some("ctrl+insert"));
    assert_eq!(parse_key("\x1bp").as_deref(), Some("alt+up"));

    assert_eq!(parse_key("\x1b[[5~").as_deref(), Some("pageUp"));
}
