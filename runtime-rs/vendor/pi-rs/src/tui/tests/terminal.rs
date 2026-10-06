//! Ports of upstream `packages/tui/test/terminal.test.ts`: the pure helpers
//! and the ProcessTerminal Kitty-keyboard-protocol negotiation state machine,
//! driven through the transport-independent [`TerminalCore`] with a recorded
//! write sink.

use crate::tui::keys::{matches_key, set_kitty_protocol_active};
use crate::tui::stdin_buffer::StdinBufferOptions;
use crate::tui::terminal::{
    normalize_apple_terminal_input, normalize_native_shift_enter_input,
    parse_keyboard_protocol_negotiation_sequence, resolve_escape_timeout_ms, NegotiationSequence,
    Terminal, TerminalCore,
};

fn env_of(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name: &str| {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn escape_timeout_uses_pi_tui_esc_timeout_when_configured() {
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("PI_TUI_ESC_TIMEOUT", "25")])),
        25
    );
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("PI_TUI_ESC_TIMEOUT", "0")])),
        10
    );
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("PI_TUI_ESC_TIMEOUT", "-1")])),
        10
    );
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("PI_TUI_ESC_TIMEOUT", "abc")])),
        10
    );
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("PI_TUI_ESC_TIMEOUT", "")])),
        10
    );
}

#[test]
fn escape_timeout_defaults_to_100ms_over_ssh() {
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("SSH_CONNECTION", "1 2 3 4")])),
        100
    );
    assert_eq!(
        resolve_escape_timeout_ms(env_of(&[("SSH_TTY", "/dev/pts/1")])),
        100
    );
    assert_eq!(resolve_escape_timeout_ms(env_of(&[])), 10);
}

#[test]
fn normalize_native_shift_enter_rewrites_return_only_when_detecting_with_shift() {
    assert_eq!(
        normalize_native_shift_enter_input("\r", true, true),
        "\x1b[13;2u"
    );
    assert_eq!(normalize_native_shift_enter_input("\r", false, true), "\r");
    assert_eq!(normalize_native_shift_enter_input("\r", true, false), "\r");
    assert_eq!(normalize_native_shift_enter_input("x", true, true), "x");
}

#[test]
fn normalize_apple_terminal_input_mirrors_native_detection() {
    assert_eq!(
        normalize_apple_terminal_input("\r", true, true),
        "\x1b[13;2u"
    );
    assert_eq!(normalize_apple_terminal_input("\r", true, false), "\r");
    assert_eq!(normalize_apple_terminal_input("\r", false, true), "\r");
    assert_eq!(normalize_apple_terminal_input("x", false, true), "x");
}

#[test]
fn parses_negotiation_sequences() {
    assert_eq!(
        parse_keyboard_protocol_negotiation_sequence("\x1b[?7u"),
        Some(NegotiationSequence::KittyFlags { flags: 7 })
    );
    assert_eq!(
        parse_keyboard_protocol_negotiation_sequence("\x1b[?0u"),
        Some(NegotiationSequence::KittyFlags { flags: 0 })
    );
    assert_eq!(
        parse_keyboard_protocol_negotiation_sequence("\x1b[?62;4;52c"),
        Some(NegotiationSequence::DeviceAttributes)
    );
    assert_eq!(parse_keyboard_protocol_negotiation_sequence("\x1b[?"), None);
}

/// The upstream `setupNegotiation` harness: a TerminalCore with recorded
/// writes, a captured input handler, and a query bootstrap.
struct Harness {
    core: TerminalCore,
    writes: Vec<String>,
    input: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Harness {
    fn new() -> Self {
        let mut core = TerminalCore::new(resolve_escape_timeout_ms(|_| None));
        let mut writes = Vec::new();
        core.query_and_enable_kitty_protocol(&mut writes);
        let input: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
        let input_sink = std::sync::Arc::clone(&input);
        core.set_input_handler(Some(Box::new(move |data| {
            input_sink.lock().unwrap().push(data)
        })));
        Self {
            core,
            writes,
            input,
        }
    }

    fn input(&self) -> Vec<String> {
        self.input.lock().unwrap().clone()
    }

    fn send(&mut self, data: &str) {
        let outcome = self.core.process_stdin(data);
        self.writes.extend(outcome.writes);
        for forwarded in outcome.forwarded_input {
            self.input.lock().unwrap().push(forwarded);
        }
    }

    fn last_input(&self) -> Option<String> {
        self.input().last().cloned()
    }
}

#[test]
fn negotiation_queries_kitty_mode_before_enabling_modify_other_keys_fallback() {
    let harness = Harness::new();
    assert_eq!(harness.writes[0], "\x1b[>7u\x1b[?u\x1b[c");
    assert!(!harness.writes.contains(&"\x1b[>4;2m".to_string()));
    assert!(!harness.core.kitty_protocol_active());
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_activates_kitty_mode_for_non_zero_flags() {
    let mut harness = Harness::new();
    harness.send("\x1b[?7u");

    assert!(harness.input().is_empty());
    assert!(harness.core.kitty_protocol_active());
    assert!(!harness.writes.contains(&"\x1b[>4;2m".to_string()));
    assert!(!harness.writes.contains(&"\x1b[>4;0m".to_string()));

    let teardown = harness.core.stop_writes();
    harness.writes.extend(teardown);
    assert_eq!(harness.writes.iter().filter(|w| *w == "\x1b[<u").count(), 1);
    assert!(!harness.writes.contains(&"\x1b[>4;0m".to_string()));
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_falls_back_to_modify_other_keys_for_zero_kitty_flags() {
    let mut harness = Harness::new();
    harness.send("\x1b[?0u");

    assert!(harness.input().is_empty());
    assert!(!harness.core.kitty_protocol_active());
    assert_eq!(
        harness.writes.iter().filter(|w| *w == "\x1b[>4;2m").count(),
        1
    );

    let teardown = harness.core.stop_writes();
    harness.writes.extend(teardown);
    assert_eq!(
        harness.writes.iter().filter(|w| *w == "\x1b[>4;0m").count(),
        1
    );
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_falls_back_to_modify_other_keys_for_device_attributes() {
    let mut harness = Harness::new();
    harness.send("\x1b[?62;4;52c");

    assert!(harness.input().is_empty());
    assert!(!harness.core.kitty_protocol_active());
    assert_eq!(
        harness.writes.iter().filter(|w| *w == "\x1b[>4;2m").count(),
        1
    );
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_forwards_normal_input_while_waiting_for_kitty_response() {
    let mut harness = Harness::new();
    harness.send("a");

    assert_eq!(harness.last_input().as_deref(), Some("a"));
    assert!(!harness.core.kitty_protocol_active());
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_tracks_split_kitty_confirmation() {
    let mut harness = Harness::new();
    // An incomplete CSI-u response: StdinBuffer holds it with a 50ms hint.
    let outcome = harness.core.process_stdin("\x1b[?7");
    assert!(outcome.forwarded_input.is_empty());
    assert_eq!(outcome.stdin_flush_after_ms, Some(50));

    // The StdinBuffer sequence timer fires first, flushing the fragment to
    // the negotiator, which buffers it as a possible Kitty response prefix.
    let outcome = harness.core.stdin_flush();
    assert!(outcome.forwarded_input.is_empty());
    assert_eq!(outcome.negotiation_flush_after_ms, Some(150));

    // The rest arrives before the fragment timer: Kitty mode activates.
    let outcome = harness.core.process_stdin("u");
    assert!(outcome.forwarded_input.is_empty());
    assert!(harness.core.kitty_protocol_active());
    assert!(!harness.writes.contains(&"\x1b[>4;2m".to_string()));
    set_kitty_protocol_active(false);
}

#[test]
fn negotiation_replays_buffered_csi_prefix_when_not_a_kitty_response() {
    let mut harness = Harness::new();
    // "\x1b[" goes through the StdinBuffer (sequence timeout, not lone-ESC).
    let outcome = harness.core.process_stdin("\x1b[");
    assert!(outcome.forwarded_input.is_empty());
    assert!(outcome.stdin_flush_after_ms.is_some());

    // StdinBuffer timer fires first and emits "\x1b[" to the negotiator.
    let outcome = harness.core.stdin_flush();
    assert!(outcome.forwarded_input.is_empty());
    assert_eq!(outcome.negotiation_flush_after_ms, Some(150));

    // The negotiation fragment timer fires with nothing following: replay.
    let outcome = harness.core.negotiation_flush();
    assert_eq!(outcome.forwarded_input, vec!["\x1b["]);
}

#[test]
fn negotiation_forwards_paste_content_rewrapped() {
    let mut harness = Harness::new();
    let outcome = harness.core.process_stdin("\x1b[200~hello world\x1b[201~");
    assert_eq!(
        outcome.forwarded_input,
        vec!["\x1b[200~hello world\x1b[201~"]
    );
}

#[test]
fn drain_input_disables_kitty_protocol_once() {
    let mut harness = Harness::new();
    harness.send("\x1b[?7u");
    assert!(harness.core.kitty_protocol_active());

    let teardown = harness.core.drain_input_writes();
    assert_eq!(teardown, vec!["\x1b[<u"]);
    assert!(!harness.core.kitty_protocol_active());
    assert!(!harness.core.keyboard_protocol_pushed());

    // A second drain does not repeat the disable sequence.
    let teardown = harness.core.drain_input_writes();
    assert!(teardown.is_empty());
    set_kitty_protocol_active(false);
}

#[test]
fn stop_writes_disable_paste_and_progress() {
    let mut core = TerminalCore::new(10);
    let teardown = core.stop_writes();
    assert_eq!(
        teardown,
        vec!["\x1b]9;4;0\x07".to_string(), "\x1b[?2004l".to_string(),]
    );
    set_kitty_protocol_active(false);
}

#[test]
fn memory_terminal_records_writes_and_dispatches_input() {
    use std::sync::{Arc, Mutex};

    let mut terminal = crate::tui::terminal::MemoryTerminal::new(80, 24);
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    terminal.start(
        Box::new(move |data| sink.lock().unwrap().push(data)),
        Box::new(|| {}),
    );
    terminal.write("\x1b[?2004h");
    terminal.send_input("hello");
    assert_eq!(terminal.writes(), ["\x1b[?2004h"]);
    assert_eq!(*received.lock().unwrap(), ["hello".to_string()]);
    assert_eq!((terminal.columns(), terminal.rows()), (80, 24));
    terminal.stop();
}

#[test]
fn legacy_escape_matches_through_stdin_buffer_flush() {
    // End-to-end: a lone ESC flushed by its escape timeout still matches the
    // Escape key binding, and ESC CR within the window matches alt+enter.
    let mut buffer = crate::tui::stdin_buffer::StdinBuffer::new(StdinBufferOptions {
        escape_timeout_ms: Some(100),
        ..Default::default()
    });
    let outcome = buffer.process("\x1b");
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.flush_after_ms, Some(100));
    let outcome = buffer.process("\r");
    let events = outcome.events;
    assert_eq!(events.len(), 1);
    if let crate::tui::stdin_buffer::StdinEvent::Data(sequence) = &events[0] {
        assert!(matches_key(sequence, "alt+enter"));
    }
}
