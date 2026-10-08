//! Port of upstream `packages/tui/src/terminal.ts`: the terminal interface,
//! keyboard-protocol negotiation, and the transport-independent parts of the
//! OS terminal.
//!
//! Disclosed substitutions for review:
//! - [`Terminal`] mirrors the upstream interface; `drainInput` becomes a
//!   provided no-op method (memory terminals) that the OS implementation
//!   overrides.
//! - The upstream `ProcessTerminal` binds Node's `process.stdin/stdout`
//!   (raw mode, resize events, Windows VT input via the native helper). Rust
//!   has no Node runtime here and unsafe FFI is out of scope, so the
//!   transport-independent state machine lives in [`TerminalCore`]: callers
//!   feed stdin data and perform the returned stdout writes and timer hints.
//!   The raw-mode/resize/native-helper shell joins with the M5 CLI
//!   integration; everything covered here behaves byte-identically.
//! - `refreshTerminalDimensions` (POSIX SIGWINCH re-raise) needs unsafe signal
//!   FFI and is not ported; `isNativeModifierPressed` becomes the
//!   caller-supplied [`TerminalCore::set_native_shift_pressed`] flag.

use std::sync::OnceLock;

use regex::Regex;

use crate::tui::keys::set_kitty_protocol_active;
use crate::tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

pub const TERMINAL_PROGRESS_KEEPALIVE_MS: u64 = 1000;
pub const TERMINAL_PROGRESS_ACTIVE_SEQUENCE: &str = "\x1b]9;4;3\x07";
pub const TERMINAL_PROGRESS_CLEAR_SEQUENCE: &str = "\x1b]9;4;0\x07";
pub const NATIVE_SHIFT_ENTER_SEQUENCE: &str = "\x1b[13;2u";
pub const DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS: u64 = 7;
pub const KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS: u64 = 150;
pub const KITTY_KEYBOARD_PROTOCOL_QUERY: &str = "\x1b[>7u\x1b[?u\x1b[c";
pub const BRACKETED_PASTE_ENABLE: &str = "\x1b[?2004h";
pub const BRACKETED_PASTE_DISABLE: &str = "\x1b[?2004l";
pub const KITTY_PROTOCOL_DISABLE_SEQUENCE: &str = "\x1b[<u";
pub const MODIFY_OTHER_KEYS_ENABLE: &str = "\x1b[>4;2m";
pub const MODIFY_OTHER_KEYS_DISABLE: &str = "\x1b[>4;0m";

/// Upstream `KeyboardProtocolNegotiationSequence`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegotiationSequence {
    KittyFlags { flags: u64 },
    DeviceAttributes,
}

fn kitty_flags_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[\?(\d+)u$").unwrap())
}

fn device_attributes_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[\?[\d;]*c$").unwrap())
}

fn negotiation_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\x1b\[\?[\d;]*$").unwrap())
}

/// Upstream `parseKeyboardProtocolNegotiationSequence`.
pub fn parse_keyboard_protocol_negotiation_sequence(sequence: &str) -> Option<NegotiationSequence> {
    if let Some(captures) = kitty_flags_regex().captures(sequence) {
        let flags = captures
            .get(1)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0);
        return Some(NegotiationSequence::KittyFlags { flags });
    }
    if device_attributes_regex().is_match(sequence) {
        return Some(NegotiationSequence::DeviceAttributes);
    }
    None
}

/// Upstream `isKeyboardProtocolNegotiationSequencePrefix`.
pub fn is_keyboard_protocol_negotiation_sequence_prefix(sequence: &str) -> bool {
    sequence == "\x1b[" || negotiation_prefix_regex().is_match(sequence)
}

/// Upstream `isAppleTerminalSession`.
pub fn is_apple_terminal_session() -> bool {
    cfg!(target_os = "macos")
        && std::env::var("TERM_PROGRAM").is_ok_and(|value| value == "Apple_Terminal")
}

const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const DEFAULT_SSH_ESCAPE_TIMEOUT_MS: u64 = 100;

/// Upstream `resolveEscapeTimeoutMs`: how long to wait for the rest of an
/// escape sequence before a lone ESC dispatches as the Escape key.
pub fn resolve_escape_timeout_ms(get_env: impl Fn(&str) -> Option<String>) -> u64 {
    if let Some(configured) = get_env("PI_TUI_ESC_TIMEOUT") {
        // Upstream `Number(value)`: "" parses as 0 and invalid values as NaN;
        // both fail the `> 0` check.
        let parsed = configured.trim().parse::<f64>().unwrap_or(f64::NAN);
        if parsed.is_finite() && parsed > 0.0 {
            return parsed as u64;
        }
    }
    if get_env("SSH_CONNECTION").is_some() || get_env("SSH_TTY").is_some() {
        return DEFAULT_SSH_ESCAPE_TIMEOUT_MS;
    }
    DEFAULT_ESCAPE_TIMEOUT_MS
}

/// Upstream `normalizeNativeShiftEnterInput`.
pub fn normalize_native_shift_enter_input(
    data: &str,
    should_detect_native_shift_enter: bool,
    is_shift_pressed: bool,
) -> String {
    if should_detect_native_shift_enter && data == "\r" && is_shift_pressed {
        return NATIVE_SHIFT_ENTER_SEQUENCE.to_string();
    }
    data.to_string()
}

/// Upstream `normalizeAppleTerminalInput`.
pub fn normalize_apple_terminal_input(
    data: &str,
    is_apple_terminal: bool,
    is_shift_pressed: bool,
) -> String {
    normalize_native_shift_enter_input(data, is_apple_terminal, is_shift_pressed)
}

/// The upstream `Terminal` interface.
pub trait Terminal {
    /// Register the input/resize handlers and enter the terminal session.
    fn start(
        &mut self,
        on_input: Box<dyn FnMut(String) + Send>,
        on_resize: Box<dyn FnMut() + Send>,
    );

    /// Restore state.
    fn stop(&mut self);

    /// Write output to the terminal.
    fn write(&mut self, data: &str);

    fn columns(&self) -> usize;
    fn rows(&self) -> usize;

    fn kitty_protocol_active(&self) -> bool;

    /// Move cursor up (negative) or down (positive) by N lines.
    fn move_by(&mut self, lines: i64);

    fn hide_cursor(&mut self);
    fn show_cursor(&mut self);
    fn clear_line(&mut self);
    fn clear_from_cursor(&mut self);
    fn clear_screen(&mut self);
    fn set_title(&mut self, title: &str);
    fn set_progress(&mut self, active: bool);

    /// Drain stdin before exiting (upstream `drainInput`); memory terminals
    /// have nothing to drain.
    fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
}

/// Memory-backed [`Terminal`] mirroring the upstream test terminal: writes are
/// recorded and input is injected explicitly.
#[derive(Default)]
pub struct MemoryTerminal {
    writes: Vec<String>,
    input_handler: Option<Box<dyn FnMut(String) + Send>>,
    resize_handler: Option<Box<dyn FnMut() + Send>>,
    columns: usize,
    rows: usize,
    started: bool,
}

impl MemoryTerminal {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            writes: Vec::new(),
            input_handler: None,
            resize_handler: None,
            columns,
            rows,
            started: false,
        }
    }

    pub fn writes(&self) -> &[String] {
        &self.writes
    }

    /// Inject input as if it arrived from the terminal.
    pub fn send_input(&mut self, data: &str) {
        if let Some(handler) = &mut self.input_handler {
            handler(data.to_string());
        }
    }

    pub fn send_resize(&mut self) {
        if let Some(handler) = &mut self.resize_handler {
            handler();
        }
    }
}

impl Terminal for MemoryTerminal {
    fn start(
        &mut self,
        on_input: Box<dyn FnMut(String) + Send>,
        on_resize: Box<dyn FnMut() + Send>,
    ) {
        self.input_handler = Some(on_input);
        self.resize_handler = Some(on_resize);
        self.started = true;
    }

    fn stop(&mut self) {
        self.input_handler = None;
        self.resize_handler = None;
        self.started = false;
    }

    fn write(&mut self, data: &str) {
        self.writes.push(data.to_string());
    }

    fn columns(&self) -> usize {
        self.columns
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn kitty_protocol_active(&self) -> bool {
        false
    }

    fn move_by(&mut self, _lines: i64) {}

    fn hide_cursor(&mut self) {}
    fn show_cursor(&mut self) {}
    fn clear_line(&mut self) {}
    fn clear_from_cursor(&mut self) {}
    fn clear_screen(&mut self) {}
    fn set_title(&mut self, _title: &str) {}
    fn set_progress(&mut self, _active: bool) {}
}

/// One `process_stdin` (or timer flush) result: everything the OS shell would
/// do on the caller's behalf.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreOutcome {
    /// stdout writes (protocol queries, fallback toggles).
    pub writes: Vec<String>,
    /// Sequences forwarded to the input handler.
    pub forwarded_input: Vec<String>,
    /// StdinBuffer flush hint (incomplete sequence buffered).
    pub stdin_flush_after_ms: Option<u64>,
    /// Negotiation-buffer hint (a possible split Kitty response).
    pub negotiation_flush_after_ms: Option<u64>,
}

enum NegotiationRead {
    Match {
        parsed: NegotiationSequence,
        /// The full (possibly reassembled) sequence, forwarded as input when
        /// the parsed reply goes unconsumed.
        sequence: String,
    },
    Pending,
    /// Not a negotiation sequence; a previously buffered prefix (if any) is
    /// replayed as input before the current sequence.
    NotNegotiation {
        flushed_input: Option<String>,
    },
}

/// The transport-independent ProcessTerminal state machine: Kitty/DA
/// negotiation, modifyOtherKeys fallback, StdinBuffer splitting, and input
/// forwarding. The OS shell feeds stdin data and performs the returned writes
/// and timer hints.
pub struct TerminalCore {
    stdin_buffer: StdinBuffer,
    kitty_protocol_active: bool,
    modify_other_keys_active: bool,
    keyboard_protocol_pushed: bool,
    /// DA1 replies owed to keyboard protocol queries. Later DA1 replies answer
    /// other queries and are forwarded.
    pending_keyboard_protocol_device_attributes: i64,
    negotiation_buffer: String,
    input_handler: Option<Box<dyn FnMut(String) + Send>>,
    native_shift_pressed: bool,
}

impl Default for TerminalCore {
    fn default() -> Self {
        Self::new(resolve_escape_timeout_ms(|name| std::env::var(name).ok()))
    }
}

impl TerminalCore {
    pub fn new(escape_timeout_ms: u64) -> Self {
        Self {
            stdin_buffer: StdinBuffer::new(StdinBufferOptions {
                escape_timeout_ms: Some(escape_timeout_ms),
                ..Default::default()
            }),
            kitty_protocol_active: false,
            modify_other_keys_active: false,
            keyboard_protocol_pushed: false,
            pending_keyboard_protocol_device_attributes: 0,
            negotiation_buffer: String::new(),
            input_handler: None,
            native_shift_pressed: false,
        }
    }

    /// Upstream `queryAndEnableKittyProtocol`: push the desired flags and
    /// query them; the trailing DA query is the no-Kitty sentinel.
    pub fn query_and_enable_kitty_protocol(&mut self, writes: &mut Vec<String>) {
        self.keyboard_protocol_pushed = true;
        self.pending_keyboard_protocol_device_attributes += 1;
        self.negotiation_buffer.clear();
        writes.push(KITTY_KEYBOARD_PROTOCOL_QUERY.to_string());
    }

    pub fn set_input_handler(&mut self, handler: Option<Box<dyn FnMut(String) + Send>>) {
        self.input_handler = handler;
    }

    /// The native-modifier hook (`isNativeModifierPressed("shift")`); the OS
    /// shell updates this from platform input state.
    pub fn set_native_shift_pressed(&mut self, pressed: bool) {
        self.native_shift_pressed = pressed;
    }

    pub fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol_active
    }

    pub fn modify_other_keys_active(&self) -> bool {
        self.modify_other_keys_active
    }

    pub fn keyboard_protocol_pushed(&self) -> bool {
        self.keyboard_protocol_pushed
    }

    /// Feed stdin data through the StdinBuffer and the negotiation machine.
    pub fn process_stdin(&mut self, data: &str) -> CoreOutcome {
        let stdin_outcome = self.stdin_buffer.process(data);
        let mut outcome = CoreOutcome {
            stdin_flush_after_ms: stdin_outcome.flush_after_ms,
            ..Default::default()
        };
        self.dispatch_events(stdin_outcome.events, &mut outcome);
        outcome
    }

    /// The StdinBuffer timer fired: flush its remainder through negotiation
    /// (upstream `flush()` inside the setTimeout).
    pub fn stdin_flush(&mut self) -> CoreOutcome {
        let mut outcome = CoreOutcome::default();
        let events = self.stdin_buffer.flush_emit();
        self.dispatch_events(events, &mut outcome);
        outcome
    }

    /// The negotiation-fragment timer fired: replay the buffered prefix as
    /// input (upstream `flushKeyboardProtocolNegotiationBufferAsInput`).
    pub fn negotiation_flush(&mut self) -> CoreOutcome {
        let mut outcome = CoreOutcome::default();
        if !self.negotiation_buffer.is_empty() {
            let sequence = std::mem::take(&mut self.negotiation_buffer);
            self.forward_input_sequence(&sequence, &mut outcome.forwarded_input);
        }
        outcome
    }

    /// Upstream `drainInput` protocol teardown writes.
    pub fn drain_input_writes(&mut self) -> Vec<String> {
        let mut writes = Vec::new();
        if self.keyboard_protocol_pushed || self.kitty_protocol_active {
            writes.push(KITTY_PROTOCOL_DISABLE_SEQUENCE.to_string());
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.negotiation_buffer.clear();
        self.disable_modify_other_keys(&mut writes);
        writes
    }

    /// Upstream `stop` teardown writes (progress clear + paste disable).
    pub fn stop_writes(&mut self) -> Vec<String> {
        let mut writes = vec![
            TERMINAL_PROGRESS_CLEAR_SEQUENCE.to_string(),
            BRACKETED_PASTE_DISABLE.to_string(),
        ];
        if self.keyboard_protocol_pushed || self.kitty_protocol_active {
            writes.push(KITTY_PROTOCOL_DISABLE_SEQUENCE.to_string());
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.negotiation_buffer.clear();
        self.disable_modify_other_keys(&mut writes);
        writes
    }

    fn dispatch_events(&mut self, events: Vec<StdinEvent>, outcome: &mut CoreOutcome) {
        for event in events {
            match event {
                StdinEvent::Data(sequence) => self.dispatch_sequence(&sequence, outcome),
                StdinEvent::Paste(content) => {
                    // Re-wrap paste content with bracketed paste markers for
                    // existing editor handling.
                    if let Some(handler) = &mut self.input_handler {
                        let wrapped = format!("\x1b[200~{content}\x1b[201~");
                        handler(wrapped.clone());
                        outcome.forwarded_input.push(wrapped);
                    }
                }
            }
        }
    }

    fn dispatch_sequence(&mut self, sequence: &str, outcome: &mut CoreOutcome) {
        match self.read_negotiation_sequence(sequence) {
            NegotiationRead::Match { parsed, sequence } => {
                outcome.writes.clear();
                if self.handle_negotiation_sequence(parsed, &mut outcome.writes) {
                    return;
                }
                // An unconsumed DA1 answer belongs to another query; the full
                // (possibly reassembled) sequence is forwarded as input.
                self.forward_input_sequence(&sequence, &mut outcome.forwarded_input);
            }
            NegotiationRead::Pending => {
                outcome.negotiation_flush_after_ms =
                    Some(KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS);
            }
            NegotiationRead::NotNegotiation { flushed_input } => {
                if let Some(flushed) = flushed_input {
                    self.forward_input_sequence(&flushed, &mut outcome.forwarded_input);
                }
                self.forward_input_sequence(sequence, &mut outcome.forwarded_input);
            }
        }
    }

    fn handle_negotiation_sequence(
        &mut self,
        sequence: NegotiationSequence,
        writes: &mut Vec<String>,
    ) -> bool {
        self.negotiation_buffer.clear();
        match sequence {
            NegotiationSequence::KittyFlags { flags } => {
                if flags != 0 {
                    self.disable_modify_other_keys(writes);
                    if !self.kitty_protocol_active {
                        self.kitty_protocol_active = true;
                        set_kitty_protocol_active(true);
                    }
                } else {
                    self.enable_modify_other_keys(writes);
                }
                true
            }
            NegotiationSequence::DeviceAttributes => {
                if self.pending_keyboard_protocol_device_attributes == 0 {
                    return false;
                }
                self.pending_keyboard_protocol_device_attributes -= 1;
                if !self.kitty_protocol_active {
                    self.enable_modify_other_keys(writes);
                }
                true
            }
        }
    }

    fn read_negotiation_sequence(&mut self, sequence: &str) -> NegotiationRead {
        if !self.negotiation_buffer.is_empty() {
            let buffered_sequence = format!("{}{}", self.negotiation_buffer, sequence);
            if let Some(negotiation) =
                parse_keyboard_protocol_negotiation_sequence(&buffered_sequence)
            {
                self.negotiation_buffer.clear();
                return NegotiationRead::Match {
                    parsed: negotiation,
                    sequence: buffered_sequence,
                };
            }
            if is_keyboard_protocol_negotiation_sequence_prefix(&buffered_sequence) {
                self.negotiation_buffer = buffered_sequence;
                return NegotiationRead::Pending;
            }
            let flushed = std::mem::take(&mut self.negotiation_buffer);
            return match self.read_negotiation_sequence(sequence) {
                NegotiationRead::NotNegotiation { flushed_input: _ } => {
                    NegotiationRead::NotNegotiation {
                        flushed_input: Some(flushed),
                    }
                }
                other => other,
            };
        }

        if let Some(negotiation) = parse_keyboard_protocol_negotiation_sequence(sequence) {
            return NegotiationRead::Match {
                parsed: negotiation,
                sequence: sequence.to_string(),
            };
        }
        if is_keyboard_protocol_negotiation_sequence_prefix(sequence) {
            self.negotiation_buffer = sequence.to_string();
            return NegotiationRead::Pending;
        }
        NegotiationRead::NotNegotiation {
            flushed_input: None,
        }
    }

    fn enable_modify_other_keys(&mut self, writes: &mut Vec<String>) {
        if self.kitty_protocol_active || self.modify_other_keys_active {
            return;
        }
        writes.push(MODIFY_OTHER_KEYS_ENABLE.to_string());
        self.modify_other_keys_active = true;
    }

    fn disable_modify_other_keys(&mut self, writes: &mut Vec<String>) {
        if !self.modify_other_keys_active {
            return;
        }
        writes.push(MODIFY_OTHER_KEYS_DISABLE.to_string());
        self.modify_other_keys_active = false;
    }

    fn forward_input_sequence(&mut self, sequence: &str, forwarded: &mut Vec<String>) {
        let Some(handler) = &mut self.input_handler else {
            return;
        };
        let should_detect_native_shift_enter =
            sequence == "\r" && (is_apple_terminal_session() || cfg!(windows));
        let is_shift_pressed = should_detect_native_shift_enter && self.native_shift_pressed;
        let input = normalize_native_shift_enter_input(
            sequence,
            should_detect_native_shift_enter,
            is_shift_pressed,
        );
        handler(input.clone());
        forwarded.push(input);
    }
}
