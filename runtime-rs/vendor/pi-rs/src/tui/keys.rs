//! Port of upstream `packages/tui/src/keys.ts`: keyboard input handling for
//! terminal applications, covering legacy sequences, xterm modifyOtherKeys,
//! and the Kitty keyboard protocol.
//!
//! Disclosed substitutions for review:
//! - Upstream `KeyId` is a TypeScript template-literal union type: typing-only,
//!   with no runtime behavior beyond `parseKeyId` splitting on `+`. Rust
//!   callers pass `&str` key identifiers with the same grammar.
//! - The `Key` helper object is autocomplete sugar over the same strings; use
//!   the constants in [`key_names`] or plain strings instead.
//! - Upstream writes `_lastEventType` in `parseKittySequence` but never reads
//!   it; the write-only state is not ported. [`is_key_release`] and
//!   [`is_key_repeat`] scan the raw data, like upstream.
//! - The anchored JS regexes become `regex` crate patterns with identical
//!   anchors and groups.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use regex::Regex;

// =============================================================================
// Global Kitty Protocol State
// =============================================================================

static KITTY_PROTOCOL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Upstream `setKittyProtocolActive`.
pub fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL_ACTIVE.store(active, Ordering::SeqCst);
}

/// Upstream `isKittyProtocolActive`.
pub fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL_ACTIVE.load(Ordering::SeqCst)
}

/// Frequently used key-name strings (the upstream `Key` helper constants).
pub mod key_names {
    pub const ESCAPE: &str = "escape";
    pub const ESC: &str = "esc";
    pub const ENTER: &str = "enter";
    pub const RETURN: &str = "return";
    pub const TAB: &str = "tab";
    pub const SPACE: &str = "space";
    pub const BACKSPACE: &str = "backspace";
    pub const DELETE: &str = "delete";
    pub const INSERT: &str = "insert";
    pub const CLEAR: &str = "clear";
    pub const HOME: &str = "home";
    pub const END: &str = "end";
    pub const PAGE_UP: &str = "pageUp";
    pub const PAGE_DOWN: &str = "pageDown";
    pub const UP: &str = "up";
    pub const DOWN: &str = "down";
    pub const LEFT: &str = "left";
    pub const RIGHT: &str = "right";
}

// =============================================================================
// Constants
// =============================================================================

const SYMBOL_KEYS: &[char] = &[
    '`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '!', '@', '#', '$', '%', '^', '&',
    '*', '(', ')', '_', '+', '|', '~', '{', '}', ':', '<', '>', '?',
];

pub(crate) const MOD_SHIFT: i64 = 1;
pub(crate) const MOD_ALT: i64 = 2;
pub(crate) const MOD_CTRL: i64 = 4;
pub(crate) const MOD_SUPER: i64 = 8;

const LOCK_MASK: i64 = 64 + 128; // Caps Lock + Num Lock

const CODEPOINT_ESCAPE: i64 = 27;
const CODEPOINT_TAB: i64 = 9;
const CODEPOINT_ENTER: i64 = 13;
const CODEPOINT_SPACE: i64 = 32;
const CODEPOINT_BACKSPACE: i64 = 127;
const CODEPOINT_KP_ENTER: i64 = 57414; // Numpad Enter (Kitty protocol)

const ARROW_UP: i64 = -1;
const ARROW_DOWN: i64 = -2;
const ARROW_RIGHT: i64 = -3;
const ARROW_LEFT: i64 = -4;

const FUNCTIONAL_DELETE: i64 = -10;
const FUNCTIONAL_INSERT: i64 = -11;
const FUNCTIONAL_PAGE_UP: i64 = -12;
const FUNCTIONAL_PAGE_DOWN: i64 = -13;
const FUNCTIONAL_HOME: i64 = -14;
const FUNCTIONAL_END: i64 = -15;

/// Upstream `KITTY_FUNCTIONAL_KEY_EQUIVALENTS` (kitty keypad keys to their
/// logical equivalents).
fn kitty_functional_equivalent(codepoint: i64) -> Option<i64> {
    Some(match codepoint {
        57399 => 48, // KP_0 -> 0
        57400 => 49, // KP_1 -> 1
        57401 => 50, // KP_2 -> 2
        57402 => 51, // KP_3 -> 3
        57403 => 52, // KP_4 -> 4
        57404 => 53, // KP_5 -> 5
        57405 => 54, // KP_6 -> 6
        57406 => 55, // KP_7 -> 7
        57407 => 56, // KP_8 -> 8
        57408 => 57, // KP_9 -> 9
        57409 => 46, // KP_DECIMAL -> .
        57410 => 47, // KP_DIVIDE -> /
        57411 => 42, // KP_MULTIPLY -> *
        57412 => 45, // KP_SUBTRACT -> -
        57413 => 43, // KP_ADD -> +
        57415 => 61, // KP_EQUAL -> =
        57416 => 44, // KP_SEPARATOR -> ,
        57417 => ARROW_LEFT,
        57418 => ARROW_RIGHT,
        57419 => ARROW_UP,
        57420 => ARROW_DOWN,
        57421 => FUNCTIONAL_PAGE_UP,
        57422 => FUNCTIONAL_PAGE_DOWN,
        57423 => FUNCTIONAL_HOME,
        57424 => FUNCTIONAL_END,
        57425 => FUNCTIONAL_INSERT,
        57426 => FUNCTIONAL_DELETE,
        _ => return None,
    })
}

fn normalize_kitty_functional_codepoint(codepoint: i64) -> i64 {
    kitty_functional_equivalent(codepoint).unwrap_or(codepoint)
}

fn normalize_shifted_letter_identity_codepoint(codepoint: i64, modifier: i64) -> i64 {
    let effective_modifier = modifier & !LOCK_MASK;
    if (effective_modifier & MOD_SHIFT) != 0 && (65..=90).contains(&codepoint) {
        return codepoint + 32;
    }
    codepoint
}

const LEGACY_KEY_SEQUENCES: &[(&str, &[&str])] = &[
    ("up", &["\x1b[A", "\x1bOA"]),
    ("down", &["\x1b[B", "\x1bOB"]),
    ("right", &["\x1b[C", "\x1bOC"]),
    ("left", &["\x1b[D", "\x1bOD"]),
    ("home", &["\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~"]),
    ("end", &["\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~"]),
    ("insert", &["\x1b[2~"]),
    ("delete", &["\x1b[3~"]),
    ("pageUp", &["\x1b[5~", "\x1b[[5~"]),
    ("pageDown", &["\x1b[6~", "\x1b[[6~"]),
    ("clear", &["\x1b[E", "\x1bOE"]),
    ("f1", &["\x1bOP", "\x1b[11~", "\x1b[[A"]),
    ("f2", &["\x1bOQ", "\x1b[12~", "\x1b[[B"]),
    ("f3", &["\x1bOR", "\x1b[13~", "\x1b[[C"]),
    ("f4", &["\x1bOS", "\x1b[14~", "\x1b[[D"]),
    ("f5", &["\x1b[15~", "\x1b[[E"]),
    ("f6", &["\x1b[17~"]),
    ("f7", &["\x1b[18~"]),
    ("f8", &["\x1b[19~"]),
    ("f9", &["\x1b[20~"]),
    ("f10", &["\x1b[21~"]),
    ("f11", &["\x1b[23~"]),
    ("f12", &["\x1b[24~"]),
];

const LEGACY_SHIFT_SEQUENCES: &[(&str, &str)] = &[
    ("up", "\x1b[a"),
    ("down", "\x1b[b"),
    ("right", "\x1b[c"),
    ("left", "\x1b[d"),
    ("clear", "\x1b[e"),
    ("insert", "\x1b[2$"),
    ("delete", "\x1b[3$"),
    ("pageUp", "\x1b[5$"),
    ("pageDown", "\x1b[6$"),
    ("home", "\x1b[7$"),
    ("end", "\x1b[8$"),
];

const LEGACY_CTRL_SEQUENCES: &[(&str, &str)] = &[
    ("up", "\x1bOa"),
    ("down", "\x1bOb"),
    ("right", "\x1bOc"),
    ("left", "\x1bOd"),
    ("clear", "\x1bOe"),
    ("insert", "\x1b[2^"),
    ("delete", "\x1b[3^"),
    ("pageUp", "\x1b[5^"),
    ("pageDown", "\x1b[6^"),
    ("home", "\x1b[7^"),
    ("end", "\x1b[8^"),
];

const LEGACY_SEQUENCE_KEY_IDS: &[(&str, &str)] = &[
    ("\x1bOA", "up"),
    ("\x1bOB", "down"),
    ("\x1bOC", "right"),
    ("\x1bOD", "left"),
    ("\x1bOH", "home"),
    ("\x1bOF", "end"),
    ("\x1b[E", "clear"),
    ("\x1bOE", "clear"),
    ("\x1bOe", "ctrl+clear"),
    ("\x1b[e", "shift+clear"),
    ("\x1b[2~", "insert"),
    ("\x1b[2$", "shift+insert"),
    ("\x1b[2^", "ctrl+insert"),
    ("\x1b[3$", "shift+delete"),
    ("\x1b[3^", "ctrl+delete"),
    ("\x1b[[5~", "pageUp"),
    ("\x1b[[6~", "pageDown"),
    ("\x1b[a", "shift+up"),
    ("\x1b[b", "shift+down"),
    ("\x1b[c", "shift+right"),
    ("\x1b[d", "shift+left"),
    ("\x1bOa", "ctrl+up"),
    ("\x1bOb", "ctrl+down"),
    ("\x1bOc", "ctrl+right"),
    ("\x1bOd", "ctrl+left"),
    ("\x1b[5$", "shift+pageUp"),
    ("\x1b[6$", "shift+pageDown"),
    ("\x1b[7$", "shift+home"),
    ("\x1b[8$", "shift+end"),
    ("\x1b[5^", "ctrl+pageUp"),
    ("\x1b[6^", "ctrl+pageDown"),
    ("\x1b[7^", "ctrl+home"),
    ("\x1b[8^", "ctrl+end"),
    ("\x1bOP", "f1"),
    ("\x1bOQ", "f2"),
    ("\x1bOR", "f3"),
    ("\x1bOS", "f4"),
    ("\x1b[11~", "f1"),
    ("\x1b[12~", "f2"),
    ("\x1b[13~", "f3"),
    ("\x1b[14~", "f4"),
    ("\x1b[[A", "f1"),
    ("\x1b[[B", "f2"),
    ("\x1b[[C", "f3"),
    ("\x1b[[D", "f4"),
    ("\x1b[[E", "f5"),
    ("\x1b[15~", "f5"),
    ("\x1b[17~", "f6"),
    ("\x1b[18~", "f7"),
    ("\x1b[19~", "f8"),
    ("\x1b[20~", "f9"),
    ("\x1b[21~", "f10"),
    ("\x1b[23~", "f11"),
    ("\x1b[24~", "f12"),
    ("\x1bb", "alt+left"),
    ("\x1bf", "alt+right"),
    ("\x1bp", "alt+up"),
    ("\x1bn", "alt+down"),
];

fn legacy_key_sequences(key: &str) -> &'static [&'static str] {
    LEGACY_KEY_SEQUENCES
        .iter()
        .find(|(name, _)| *name == key)
        .map_or(&[], |(_, sequences)| sequences)
}

fn matches_legacy_sequence(data: &str, sequences: &[&str]) -> bool {
    sequences.contains(&data)
}

fn matches_legacy_modifier_sequence(data: &str, key: &str, modifier: i64) -> bool {
    if modifier == MOD_SHIFT {
        let sequence = LEGACY_SHIFT_SEQUENCES
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, seq)| *seq);
        return sequence == Some(data);
    }
    if modifier == MOD_CTRL {
        let sequence = LEGACY_CTRL_SEQUENCES
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, seq)| *seq);
        return sequence == Some(data);
    }
    false
}

// =============================================================================
// Kitty Protocol Parsing
// =============================================================================

/// Event types from Kitty keyboard protocol (flag 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug)]
struct ParsedKittySequence {
    codepoint: i64,
    // Upstream carries the shifted key and event type on the parsed sequence;
    // current Rust use sites read those groups directly from the CSI-u match,
    // so the fields are populated for parity but not yet consumed.
    #[allow(dead_code)]
    shifted_key: Option<i64>,
    base_layout_key: Option<i64>,
    modifier: i64,
    #[allow(dead_code)]
    event_type: KeyEventType,
}

#[derive(Clone, Copy, Debug)]
struct ParsedModifyOtherKeysSequence {
    codepoint: i64,
    modifier: i64,
}

fn csi_u_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$").unwrap()
    })
}

fn parse_event_type(event_type_str: Option<&str>) -> KeyEventType {
    match event_type_str.and_then(|s| s.parse::<i64>().ok()) {
        Some(2) => KeyEventType::Repeat,
        Some(3) => KeyEventType::Release,
        _ => KeyEventType::Press,
    }
}

fn parse_group_i64(group: Option<regex::Match<'_>>) -> Option<i64> {
    group.and_then(|m| m.as_str().parse::<i64>().ok())
}

fn parse_kitty_sequence(data: &str) -> Option<ParsedKittySequence> {
    if let Some(parsed) = parse_kitty_sequence_csi_u(data) {
        return Some(parsed);
    }
    parse_kitty_sequence_late(data)
}

fn parse_kitty_sequence_csi_u(data: &str) -> Option<ParsedKittySequence> {
    // CSI u format with alternate keys (flag 4) and event types (flag 2).
    let captures = csi_u_regex().captures(data)?;
    let codepoint = parse_group_i64(captures.get(1))?;
    let shifted_key = match captures.get(2) {
        Some(m) if !m.as_str().is_empty() => Some(m.as_str().parse::<i64>().ok()?),
        _ => None,
    };
    let base_layout_key = parse_group_i64(captures.get(3));
    let modifier = match captures.get(4) {
        Some(m) => m.as_str().parse::<i64>().unwrap_or(1),
        None => 1,
    } - 1;
    let event_type = parse_event_type(captures.get(5).map(|m| m.as_str()));
    Some(ParsedKittySequence {
        codepoint,
        shifted_key,
        base_layout_key,
        modifier,
        event_type,
    })
}

fn arrow_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[1;(\d+)(?::(\d+))?([ABCD])$").unwrap())
}

fn functional_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[(\d+)(?:;(\d+))?(?::(\d+))?~$").unwrap())
}

fn home_end_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[1;(\d+)(?::(\d+))?([HF])$").unwrap())
}

/// Upstream `parseKittySequence` continuation: arrow keys with modifiers,
/// functional `~` keys, and Home/End with modifiers.
fn parse_kitty_sequence_late(data: &str) -> Option<ParsedKittySequence> {
    if let Some(captures) = arrow_regex().captures(data) {
        let modifier = parse_group_i64(captures.get(1))?;
        let event_type = parse_event_type(captures.get(2).map(|m| m.as_str()));
        let codepoint = match captures.get(3).map(|m| m.as_str()) {
            Some("A") => ARROW_UP,
            Some("B") => ARROW_DOWN,
            Some("C") => ARROW_RIGHT,
            Some("D") => ARROW_LEFT,
            _ => return None,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: modifier - 1,
            event_type,
        });
    }

    if let Some(captures) = functional_regex().captures(data) {
        let key_num = parse_group_i64(captures.get(1))?;
        let modifier = match captures.get(2) {
            Some(m) => m.as_str().parse::<i64>().ok()?,
            None => 1,
        };
        let event_type = parse_event_type(captures.get(3).map(|m| m.as_str()));
        let codepoint = match key_num {
            2 => FUNCTIONAL_INSERT,
            3 => FUNCTIONAL_DELETE,
            5 => FUNCTIONAL_PAGE_UP,
            6 => FUNCTIONAL_PAGE_DOWN,
            7 => FUNCTIONAL_HOME,
            8 => FUNCTIONAL_END,
            _ => return None,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: modifier - 1,
            event_type,
        });
    }

    if let Some(captures) = home_end_regex().captures(data) {
        let modifier = parse_group_i64(captures.get(1))?;
        let event_type = parse_event_type(captures.get(2).map(|m| m.as_str()));
        let codepoint = match captures.get(3).map(|m| m.as_str()) {
            Some("H") => FUNCTIONAL_HOME,
            Some("F") => FUNCTIONAL_END,
            _ => return None,
        };
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: modifier - 1,
            event_type,
        });
    }

    None
}

fn matches_kitty_sequence(data: &str, expected_codepoint: i64, expected_modifier: i64) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;

    if actual_mod != expected_mod {
        return false;
    }

    let normalized_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(expected_codepoint),
        expected_modifier,
    );

    // Primary match: codepoint matches directly after normalizing functional keys.
    if normalized_codepoint == normalized_expected_codepoint {
        return true;
    }

    // Alternate match: base layout key for non-Latin keyboard layouts. Only
    // used when the codepoint is NOT already a recognized Latin letter or
    // symbol, so remapped layouts cannot cause false matches.
    if parsed.base_layout_key == Some(expected_codepoint) {
        let cp = normalized_codepoint;
        let is_latin_letter = (97..=122).contains(&cp); // a-z
        let is_known_symbol = char::from_u32(cp as u32).is_some_and(|c| SYMBOL_KEYS.contains(&c));
        if !is_latin_letter && !is_known_symbol {
            return true;
        }
    }

    false
}

fn modify_other_keys_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[27;(\d+);(\d+)~$").unwrap())
}

fn parse_modify_other_keys_sequence(data: &str) -> Option<ParsedModifyOtherKeysSequence> {
    let captures = modify_other_keys_regex().captures(data)?;
    let modifier = parse_group_i64(captures.get(1))?;
    let codepoint = parse_group_i64(captures.get(2))?;
    Some(ParsedModifyOtherKeysSequence {
        codepoint,
        modifier: modifier - 1,
    })
}

/// Match xterm modifyOtherKeys format: CSI 27 ; modifiers ; keycode ~
fn matches_modify_other_keys(data: &str, expected_keycode: i64, expected_modifier: i64) -> bool {
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    parsed.codepoint == expected_keycode && parsed.modifier == expected_modifier
}

fn is_windows_terminal_session() -> bool {
    std::env::var_os("WT_SESSION").is_some_and(|v| !v.is_empty())
        && std::env::var_os("SSH_CONNECTION").is_none()
        && std::env::var_os("SSH_CLIENT").is_none()
        && std::env::var_os("SSH_TTY").is_none()
}

/// Raw 0x08 (BS) is ambiguous in legacy terminals; see keys.ts:722.
fn matches_raw_backspace(data: &str, expected_modifier: i64) -> bool {
    if data == "\x7f" {
        return expected_modifier == 0;
    }
    if data != "\x08" {
        return false;
    }
    if is_windows_terminal_session() {
        expected_modifier == MOD_CTRL
    } else {
        expected_modifier == 0
    }
}

/// Universal control-character formula: code & 0x1f.
fn raw_ctrl_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let lowered = ch.to_ascii_lowercase();
    let code = lowered as u32;
    if (97..=122).contains(&code)
        || lowered == '['
        || lowered == '\\'
        || lowered == ']'
        || lowered == '_'
    {
        return char::from_u32(code & 0x1f);
    }
    // Handle - as _ (same physical key on US keyboards).
    if lowered == '-' {
        return char::from_u32(31); // Same as Ctrl+_
    }
    None
}

fn is_digit_key(key: &str) -> bool {
    matches!(key.as_bytes(), [b] if b.is_ascii_digit())
}

fn matches_printable_modify_other_keys(
    data: &str,
    expected_keycode: i64,
    expected_modifier: i64,
) -> bool {
    if expected_modifier == 0 {
        return false;
    }
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    if parsed.modifier != expected_modifier {
        return false;
    }
    normalize_shifted_letter_identity_codepoint(parsed.codepoint, parsed.modifier)
        == normalize_shifted_letter_identity_codepoint(expected_keycode, expected_modifier)
}

fn format_key_name_with_modifiers(key_name: &str, modifier: i64) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    let effective_mod = modifier & !LOCK_MASK;
    let supported_modifier_mask = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_SUPER;
    if (effective_mod & !supported_modifier_mask) != 0 {
        return None;
    }
    if (effective_mod & MOD_SHIFT) != 0 {
        mods.push("shift");
    }
    if (effective_mod & MOD_CTRL) != 0 {
        mods.push("ctrl");
    }
    if (effective_mod & MOD_ALT) != 0 {
        mods.push("alt");
    }
    if (effective_mod & MOD_SUPER) != 0 {
        mods.push("super");
    }
    Some(if mods.is_empty() {
        key_name.to_string()
    } else {
        format!("{}+{}", mods.join("+"), key_name)
    })
}

#[derive(Clone, Debug, Default)]
struct ParsedKeyId {
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_key: bool,
}

fn parse_key_id(key_id: &str) -> Option<ParsedKeyId> {
    let lowered = key_id.to_lowercase();
    let key = lowered.rsplit('+').next()?.to_string();
    if key.is_empty() {
        return None;
    }
    Some(ParsedKeyId {
        key,
        ctrl: lowered.split('+').any(|part| part == "ctrl"),
        shift: lowered.split('+').any(|part| part == "shift"),
        alt: lowered.split('+').any(|part| part == "alt"),
        super_key: lowered.split('+').any(|part| part == "super"),
    })
}

/// Upstream `matchesKey`: match input data against a key identifier string.
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let Some(parsed) = parse_key_id(key_id) else {
        return false;
    };

    let key = parsed.key.as_str();
    let mut modifier: i64 = 0;
    if parsed.shift {
        modifier |= MOD_SHIFT;
    }
    if parsed.alt {
        modifier |= MOD_ALT;
    }
    if parsed.ctrl {
        modifier |= MOD_CTRL;
    }
    if parsed.super_key {
        modifier |= MOD_SUPER;
    }

    match key {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            data == "\x1b"
                || matches_kitty_sequence(data, CODEPOINT_ESCAPE, 0)
                || matches_modify_other_keys(data, CODEPOINT_ESCAPE, 0)
        }

        "space" => {
            if !is_kitty_protocol_active() {
                if modifier == MOD_CTRL && data == "\x00" {
                    return true;
                }
                if modifier == MOD_ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                return data == " "
                    || matches_kitty_sequence(data, CODEPOINT_SPACE, 0)
                    || matches_modify_other_keys(data, CODEPOINT_SPACE, 0);
            }
            matches_kitty_sequence(data, CODEPOINT_SPACE, modifier)
                || matches_modify_other_keys(data, CODEPOINT_SPACE, modifier)
        }

        "tab" => {
            if modifier == MOD_SHIFT {
                return data == "\x1b[Z"
                    || matches_kitty_sequence(data, CODEPOINT_TAB, MOD_SHIFT)
                    || matches_modify_other_keys(data, CODEPOINT_TAB, MOD_SHIFT);
            }
            if modifier == 0 {
                return data == "\t" || matches_kitty_sequence(data, CODEPOINT_TAB, 0);
            }
            matches_kitty_sequence(data, CODEPOINT_TAB, modifier)
                || matches_modify_other_keys(data, CODEPOINT_TAB, modifier)
        }

        "enter" | "return" => {
            if modifier == MOD_SHIFT {
                if matches_kitty_sequence(data, CODEPOINT_ENTER, MOD_SHIFT)
                    || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, MOD_SHIFT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, CODEPOINT_ENTER, MOD_SHIFT) {
                    return true;
                }
                if is_kitty_protocol_active() {
                    return data == "\x1b\r" || data == "\n";
                }
                return false;
            }
            if modifier == MOD_ALT {
                if matches_kitty_sequence(data, CODEPOINT_ENTER, MOD_ALT)
                    || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, MOD_ALT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, CODEPOINT_ENTER, MOD_ALT) {
                    return true;
                }
                if !is_kitty_protocol_active() {
                    return data == "\x1b\r";
                }
                return false;
            }
            if modifier == 0 {
                return data == "\r"
                    || (!is_kitty_protocol_active() && data == "\n")
                    || data == "\x1bOM"
                    || matches_kitty_sequence(data, CODEPOINT_ENTER, 0)
                    || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, 0);
            }
            matches_kitty_sequence(data, CODEPOINT_ENTER, modifier)
                || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, modifier)
                || matches_modify_other_keys(data, CODEPOINT_ENTER, modifier)
        }

        "backspace" => {
            if modifier == MOD_ALT {
                if data == "\x1b\x7f" || data == "\x1b\x08" {
                    return true;
                }
                return matches_kitty_sequence(data, CODEPOINT_BACKSPACE, MOD_ALT)
                    || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                if matches_raw_backspace(data, MOD_CTRL) {
                    return true;
                }
                return matches_kitty_sequence(data, CODEPOINT_BACKSPACE, MOD_CTRL)
                    || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_raw_backspace(data, 0)
                    || matches_kitty_sequence(data, CODEPOINT_BACKSPACE, 0)
                    || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, 0);
            }
            matches_kitty_sequence(data, CODEPOINT_BACKSPACE, modifier)
                || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, modifier)
        }

        "insert" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("insert"))
                    || matches_kitty_sequence(data, FUNCTIONAL_INSERT, 0);
            }
            if matches_legacy_modifier_sequence(data, "insert", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_INSERT, modifier)
        }

        "delete" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("delete"))
                    || matches_kitty_sequence(data, FUNCTIONAL_DELETE, 0);
            }
            if matches_legacy_modifier_sequence(data, "delete", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_DELETE, modifier)
        }

        "clear" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("clear"));
            }
            matches_legacy_modifier_sequence(data, "clear", modifier)
        }

        "home" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("home"))
                    || matches_kitty_sequence(data, FUNCTIONAL_HOME, 0);
            }
            if matches_legacy_modifier_sequence(data, "home", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_HOME, modifier)
        }

        "end" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("end"))
                    || matches_kitty_sequence(data, FUNCTIONAL_END, 0);
            }
            if matches_legacy_modifier_sequence(data, "end", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_END, modifier)
        }

        "pageup" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("pageUp"))
                    || matches_kitty_sequence(data, FUNCTIONAL_PAGE_UP, 0);
            }
            if matches_legacy_modifier_sequence(data, "pageUp", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_PAGE_UP, modifier)
        }

        "pagedown" => {
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("pageDown"))
                    || matches_kitty_sequence(data, FUNCTIONAL_PAGE_DOWN, 0);
            }
            if matches_legacy_modifier_sequence(data, "pageDown", modifier) {
                return true;
            }
            matches_kitty_sequence(data, FUNCTIONAL_PAGE_DOWN, modifier)
        }

        "up" => {
            if modifier == MOD_ALT {
                return data == "\x1bp" || matches_kitty_sequence(data, ARROW_UP, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("up"))
                    || matches_kitty_sequence(data, ARROW_UP, 0);
            }
            if matches_legacy_modifier_sequence(data, "up", modifier) {
                return true;
            }
            matches_kitty_sequence(data, ARROW_UP, modifier)
        }

        "down" => {
            if modifier == MOD_ALT {
                return data == "\x1bn" || matches_kitty_sequence(data, ARROW_DOWN, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("down"))
                    || matches_kitty_sequence(data, ARROW_DOWN, 0);
            }
            if matches_legacy_modifier_sequence(data, "down", modifier) {
                return true;
            }
            matches_kitty_sequence(data, ARROW_DOWN, modifier)
        }

        "left" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3D"
                    || (!is_kitty_protocol_active() && data == "\x1bB")
                    || data == "\x1bb"
                    || matches_kitty_sequence(data, ARROW_LEFT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5D"
                    || matches_legacy_modifier_sequence(data, "left", MOD_CTRL)
                    || matches_kitty_sequence(data, ARROW_LEFT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("left"))
                    || matches_kitty_sequence(data, ARROW_LEFT, 0);
            }
            if matches_legacy_modifier_sequence(data, "left", modifier) {
                return true;
            }
            matches_kitty_sequence(data, ARROW_LEFT, modifier)
        }

        "right" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3C"
                    || (!is_kitty_protocol_active() && data == "\x1bF")
                    || data == "\x1bf"
                    || matches_kitty_sequence(data, ARROW_RIGHT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5C"
                    || matches_legacy_modifier_sequence(data, "right", MOD_CTRL)
                    || matches_kitty_sequence(data, ARROW_RIGHT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy_sequence(data, legacy_key_sequences("right"))
                    || matches_kitty_sequence(data, ARROW_RIGHT, 0);
            }
            if matches_legacy_modifier_sequence(data, "right", modifier) {
                return true;
            }
            matches_kitty_sequence(data, ARROW_RIGHT, modifier)
        }

        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            if modifier != 0 {
                return false;
            }
            matches_legacy_sequence(data, legacy_key_sequences(key))
        }

        _ => {
            // Single letter/digit keys and symbols.
            let bytes = key.as_bytes();
            let is_single_char = bytes.len() == 1;
            let is_letter = matches!(bytes, [b] if b.is_ascii_lowercase());
            let is_symbol = matches!(bytes, [b] if SYMBOL_KEYS.contains(&(*b as char)));

            if is_single_char && (is_letter || is_digit_key(key) || is_symbol) {
                let codepoint = bytes[0] as i64;
                let raw_ctrl = raw_ctrl_char(key);

                if modifier == MOD_CTRL + MOD_ALT && !is_kitty_protocol_active() {
                    if let Some(raw_ctrl) = raw_ctrl {
                        // Legacy: ctrl+alt+key is ESC followed by the control character.
                        let legacy = format!("\x1b{raw_ctrl}");
                        if data == legacy {
                            return true;
                        }
                    }
                }

                if modifier == MOD_ALT
                    && !is_kitty_protocol_active()
                    && (is_letter || is_digit_key(key) || is_symbol)
                {
                    // Legacy: alt+printable key is ESC followed by the key.
                    let legacy = format!("\x1b{key}");
                    if data == legacy {
                        return true;
                    }
                }

                if modifier == MOD_CTRL {
                    if let Some(raw_ctrl) = raw_ctrl {
                        if data == raw_ctrl.to_string() {
                            return true;
                        }
                    }
                    return matches_kitty_sequence(data, codepoint, MOD_CTRL)
                        || matches_printable_modify_other_keys(data, codepoint, MOD_CTRL);
                }

                if modifier == MOD_SHIFT + MOD_CTRL {
                    return matches_kitty_sequence(data, codepoint, MOD_SHIFT + MOD_CTRL)
                        || matches_printable_modify_other_keys(
                            data,
                            codepoint,
                            MOD_SHIFT + MOD_CTRL,
                        );
                }

                if modifier == MOD_SHIFT {
                    // Legacy: shift+letter produces uppercase.
                    if is_letter && data == key.to_ascii_uppercase() {
                        return true;
                    }
                    return matches_kitty_sequence(data, codepoint, MOD_SHIFT)
                        || matches_printable_modify_other_keys(data, codepoint, MOD_SHIFT);
                }

                if modifier != 0 {
                    return matches_kitty_sequence(data, codepoint, modifier)
                        || matches_printable_modify_other_keys(data, codepoint, modifier);
                }

                // Check both raw char and Kitty sequence (needed for release events).
                return data == key || matches_kitty_sequence(data, codepoint, 0);
            }

            false
        }
    }
}

/// Upstream `isKeyRelease`: only meaningful with Kitty protocol flag 2.
pub fn is_key_release(data: &str) -> bool {
    // Don't treat bracketed paste content as key release.
    if data.contains("\x1b[200~") {
        return false;
    }
    [":3u", ":3~", ":3A", ":3B", ":3C", ":3D", ":3H", ":3F"]
        .iter()
        .any(|pattern| data.contains(pattern))
}

/// Upstream `isKeyRepeat`: only meaningful with Kitty protocol flag 2.
pub fn is_key_repeat(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }
    [":2u", ":2~", ":2A", ":2B", ":2C", ":2D", ":2H", ":2F"]
        .iter()
        .any(|pattern| data.contains(pattern))
}

fn char_from_codepoint(codepoint: i64) -> Option<char> {
    char::from_u32(u32::try_from(codepoint).ok()?)
}

fn format_parsed_key(
    codepoint: i64,
    modifier: i64,
    base_layout_key: Option<i64>,
) -> Option<String> {
    let normalized_codepoint = normalize_kitty_functional_codepoint(codepoint);
    let identity_codepoint =
        normalize_shifted_letter_identity_codepoint(normalized_codepoint, modifier);

    // The codepoint is authoritative for recognized Latin letters, digits and
    // symbols; the base layout key applies to other layouts only.
    let is_latin_letter = (97..=122).contains(&identity_codepoint);
    let is_digit = (48..=57).contains(&identity_codepoint);
    let is_known_symbol =
        char_from_codepoint(identity_codepoint).is_some_and(|c| SYMBOL_KEYS.contains(&c));
    let effective_codepoint = if is_latin_letter || is_digit || is_known_symbol {
        identity_codepoint
    } else {
        base_layout_key.unwrap_or(identity_codepoint)
    };

    let key_name = match effective_codepoint {
        cp if cp == CODEPOINT_ESCAPE => "escape",
        cp if cp == CODEPOINT_TAB => "tab",
        cp if cp == CODEPOINT_ENTER || cp == CODEPOINT_KP_ENTER => "enter",
        cp if cp == CODEPOINT_SPACE => "space",
        cp if cp == CODEPOINT_BACKSPACE => "backspace",
        cp if cp == FUNCTIONAL_DELETE => "delete",
        cp if cp == FUNCTIONAL_INSERT => "insert",
        cp if cp == FUNCTIONAL_HOME => "home",
        cp if cp == FUNCTIONAL_END => "end",
        cp if cp == FUNCTIONAL_PAGE_UP => "pageUp",
        cp if cp == FUNCTIONAL_PAGE_DOWN => "pageDown",
        cp if cp == ARROW_UP => "up",
        cp if cp == ARROW_DOWN => "down",
        cp if cp == ARROW_LEFT => "left",
        cp if cp == ARROW_RIGHT => "right",
        cp if (48..=57).contains(&cp) || (97..=122).contains(&cp) => {
            return format_key_name_with_modifiers(&char_from_codepoint(cp)?.to_string(), modifier);
        }
        cp if char_from_codepoint(cp).is_some_and(|c| SYMBOL_KEYS.contains(&c)) => {
            return format_key_name_with_modifiers(&char_from_codepoint(cp)?.to_string(), modifier);
        }
        _ => return None,
    };

    format_key_name_with_modifiers(key_name, modifier)
}

/// Upstream `parseKey`: parse input data and return the key identifier.
pub fn parse_key(data: &str) -> Option<String> {
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_parsed_key(kitty.codepoint, kitty.modifier, kitty.base_layout_key);
    }

    if let Some(modify_other_keys) = parse_modify_other_keys_sequence(data) {
        return format_parsed_key(
            modify_other_keys.codepoint,
            modify_other_keys.modifier,
            None,
        );
    }

    // When Kitty protocol is active, ambiguous sequences are custom terminal
    // mappings: \x1b\r = shift+enter (Kitty), \n = shift+enter (Ghostty).
    if is_kitty_protocol_active() && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".to_string());
    }

    if let Some((_, key_id)) = LEGACY_SEQUENCE_KEY_IDS
        .iter()
        .find(|(sequence, _)| *sequence == data)
    {
        return Some(key_id.to_string());
    }

    if data == "\x1b" {
        return Some("escape".to_string());
    }
    if data == "\x1c" {
        return Some("ctrl+\\".to_string());
    }
    if data == "\x1d" {
        return Some("ctrl+]".to_string());
    }
    if data == "\x1f" {
        return Some("ctrl+-".to_string());
    }
    if data == "\x1b\x1b" {
        return Some("ctrl+alt+[".to_string());
    }
    if data == "\x1b\x1c" {
        return Some("ctrl+alt+\\".to_string());
    }
    if data == "\x1b\x1d" {
        return Some("ctrl+alt+]".to_string());
    }
    if data == "\x1b\x1f" {
        return Some("ctrl+alt+-".to_string());
    }
    if data == "\t" {
        return Some("tab".to_string());
    }
    if data == "\r" || (!is_kitty_protocol_active() && data == "\n") || data == "\x1bOM" {
        return Some("enter".to_string());
    }
    if data == "\x00" {
        return Some("ctrl+space".to_string());
    }
    if data == " " {
        return Some("space".to_string());
    }
    if data == "\x7f" {
        return Some("backspace".to_string());
    }
    if data == "\x08" {
        return Some(if is_windows_terminal_session() {
            "ctrl+backspace".to_string()
        } else {
            "backspace".to_string()
        });
    }
    if data == "\x1b[Z" {
        return Some("shift+tab".to_string());
    }
    if !is_kitty_protocol_active() && data == "\x1b\r" {
        return Some("alt+enter".to_string());
    }
    if !is_kitty_protocol_active() && data == "\x1b " {
        return Some("alt+space".to_string());
    }
    if data == "\x1b\x7f" || data == "\x1b\x08" {
        return Some("alt+backspace".to_string());
    }
    if !is_kitty_protocol_active() && data == "\x1bB" {
        return Some("alt+left".to_string());
    }
    if !is_kitty_protocol_active() && data == "\x1bF" {
        return Some("alt+right".to_string());
    }
    if !is_kitty_protocol_active() && data.len() == 2 && data.as_bytes()[0] == 0x1b {
        let code = data.as_bytes()[1];
        if (1..=26).contains(&code) {
            return Some(format!(
                "ctrl+alt+{}",
                char::from_u32(u32::from(code) + 96).unwrap()
            ));
        }
        // Legacy alt+letter/digit/symbol (ESC followed by the key).
        let key = code as char;
        if key.is_ascii_lowercase() || key.is_ascii_digit() || SYMBOL_KEYS.contains(&key) {
            return Some(format!("alt+{key}"));
        }
    }
    if data == "\x1b[A" {
        return Some("up".to_string());
    }
    if data == "\x1b[B" {
        return Some("down".to_string());
    }
    if data == "\x1b[C" {
        return Some("right".to_string());
    }
    if data == "\x1b[D" {
        return Some("left".to_string());
    }
    if data == "\x1b[H" || data == "\x1bOH" {
        return Some("home".to_string());
    }
    if data == "\x1b[F" || data == "\x1bOF" {
        return Some("end".to_string());
    }
    if data == "\x1b[3~" {
        return Some("delete".to_string());
    }
    if data == "\x1b[5~" {
        return Some("pageUp".to_string());
    }
    if data == "\x1b[6~" {
        return Some("pageDown".to_string());
    }

    // Raw Ctrl+letter.
    if data.len() == 1 {
        let code = data.as_bytes()[0];
        if (1..=26).contains(&code) {
            return Some(format!(
                "ctrl+{}",
                char::from_u32(u32::from(code) + 96).unwrap()
            ));
        }
        if (32..=126).contains(&code) {
            return Some(data.to_string());
        }
    }

    None
}

// =============================================================================
// Kitty CSI-u Printable Decoding
// =============================================================================

const KITTY_PRINTABLE_ALLOWED_MODIFIERS: i64 = MOD_SHIFT | LOCK_MASK;

/// Upstream `decodeKittyPrintable`: extract the printable character from a
/// Kitty CSI-u sequence (plain or Shift-modified only).
pub fn decode_kitty_printable(data: &str) -> Option<String> {
    let captures = csi_u_regex().captures(data)?;

    let codepoint = parse_group_i64(captures.get(1))?;
    let shifted_key = match captures.get(2) {
        Some(m) if !m.as_str().is_empty() => Some(m.as_str().parse::<i64>().ok()?),
        _ => None,
    };
    let modifier = match captures.get(4) {
        Some(m) => m.as_str().parse::<i64>().unwrap_or(1),
        None => 1,
    } - 1;

    // Reject unsupported modifier bits (e.g. Super/Meta) and Ctrl/Alt.
    if (modifier & !KITTY_PRINTABLE_ALLOWED_MODIFIERS) != 0 {
        return None;
    }
    if (modifier & (MOD_ALT | MOD_CTRL)) != 0 {
        return None;
    }

    // Prefer the shifted keycode when Shift is held.
    let mut effective_codepoint = codepoint;
    if (modifier & MOD_SHIFT) != 0 {
        if let Some(shifted) = shifted_key {
            effective_codepoint = shifted;
        }
    }
    effective_codepoint = normalize_kitty_functional_codepoint(effective_codepoint);
    // Drop control characters or invalid codepoints.
    if effective_codepoint < 32 {
        return None;
    }

    char_from_codepoint(effective_codepoint).map(|c| c.to_string())
}

fn decode_modify_other_keys_printable(data: &str) -> Option<String> {
    let parsed = parse_modify_other_keys_sequence(data)?;
    let modifier = parsed.modifier & !LOCK_MASK;
    if (modifier & !MOD_SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    char_from_codepoint(parsed.codepoint).map(|c| c.to_string())
}

/// Upstream `decodePrintableKey`.
pub fn decode_printable_key(data: &str) -> Option<String> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}
