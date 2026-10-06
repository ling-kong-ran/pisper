//! Port of upstream `packages/tui/src/stdin-buffer.ts`: buffers stdin input
//! and splits it into complete escape sequences, handling chunks that arrive
//! split across multiple events, bracketed paste, and Kitty CSI-u press
//! echo deduplication.
//!
//! Disclosed substitutions for review:
//! - Upstream extends `EventEmitter` and schedules its own `setTimeout` for
//!   flushing incomplete sequences. Rust has no implicit event loop here: the
//!   buffer is a plain state machine — [`StdinBuffer::process`] returns the
//!   emitted events plus an optional `flush_after_ms` hint, and the caller
//!   (the terminal layer) schedules the flush. [`StdinBuffer::flush_emit`]
//!   reproduces the upstream timer path including Kitty dedup; public
//!   [`StdinBuffer::flush`] stays raw like upstream.
//! - Upstream accepts `string | Buffer`. The Buffer path's high-byte rule
//!   (single byte > 127 becomes ESC + byte-128) is [`StdinBuffer::process_bytes`].
//! - JS indexes UTF-16 units; Rust iterates chars. The plain-text path emits
//!   one full char where upstream would split lone surrogates, which cannot
//!   occur in Rust strings.

const ESC: char = '\x1b';
const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// Whether `data` is a complete escape sequence or needs more data.
fn is_complete_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with(ESC) {
        return SequenceStatus::NotEscape;
    }

    let Some(after_esc) = data.get(1..) else {
        // A lone ESC (single char): may still grow into a sequence.
        return SequenceStatus::Incomplete;
    };
    if after_esc.is_empty() {
        // Upstream `data.length === 1` - the lone ESC may still grow.
        return SequenceStatus::Incomplete;
    }

    // CSI sequences: ESC [
    if let Some(rest) = after_esc.strip_prefix('[') {
        if rest.starts_with('M') {
            // Old-style mouse needs ESC[M + 3 bytes = 6 total chars/bytes here
            // (the trailing 3 bytes are raw and therefore ASCII in practice).
            return if data.chars().count() >= 6 {
                SequenceStatus::Complete
            } else {
                SequenceStatus::Incomplete
            };
        }
        return is_complete_csi_sequence(data);
    }

    // OSC sequences: ESC ]
    if after_esc.starts_with(']') {
        return is_complete_osc_sequence(data);
    }

    // DCS sequences: ESC P ... ESC \ (includes XTVersion responses)
    if after_esc.starts_with('P') {
        return is_complete_dcs_sequence(data);
    }

    // APC sequences: ESC _ ... ESC \ (includes Kitty graphics responses)
    if after_esc.starts_with('_') {
        return is_complete_apc_sequence(data);
    }

    // SS3 sequences: ESC O followed by a single character.
    if after_esc.starts_with('O') {
        return if after_esc.chars().count() >= 2 {
            SequenceStatus::Complete
        } else {
            SequenceStatus::Incomplete
        };
    }

    // Meta key sequences: ESC followed by a single character.
    if after_esc.chars().count() == 1 {
        return SequenceStatus::Complete;
    }

    // Unknown escape sequence - treat as complete.
    SequenceStatus::Complete
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SequenceStatus {
    Complete,
    Incomplete,
    NotEscape,
}

/// Check if CSI sequence is complete: ESC [ ... final byte (0x40-0x7E).
fn is_complete_csi_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b[") {
        return SequenceStatus::Complete;
    }

    // Need at least ESC [ and one more character.
    if data.chars().count() < 3 {
        return SequenceStatus::Incomplete;
    }

    let payload = &data[2..];
    let Some(last_char) = payload.chars().last() else {
        return SequenceStatus::Incomplete;
    };
    let last_char_code = last_char as u32;

    if (0x40..=0x7e).contains(&last_char_code) {
        // Special handling for SGR mouse sequences:
        // ESC[<B;X;Ym or ESC[<B;X;YM.
        if payload.starts_with('<') {
            if is_sgr_mouse_payload(payload) {
                return SequenceStatus::Complete;
            }
            if last_char == 'M' || last_char == 'm' {
                let parts: Vec<&str> = payload[1..payload.len() - 1].split(';').collect();
                if parts.len() == 3
                    && parts
                        .iter()
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                {
                    return SequenceStatus::Complete;
                }
            }
            return SequenceStatus::Incomplete;
        }
        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

fn is_sgr_mouse_payload(payload: &str) -> bool {
    // ^<\d+;\d+;\d+[Mm]$
    let body = match payload.strip_prefix('<') {
        Some(rest) => match rest.char_indices().last() {
            Some((index, last)) if last == 'M' || last == 'm' => &rest[..index],
            _ => return false,
        },
        None => return false,
    };
    let parts: Vec<&str> = body.split(';').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// OSC sequences end with ST (ESC \\) or BEL.
fn is_complete_osc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b]") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") || data.ends_with('\x07') {
        SequenceStatus::Complete
    } else {
        SequenceStatus::Incomplete
    }
}

/// DCS sequences end with ST (ESC \\).
fn is_complete_dcs_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1bP") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        SequenceStatus::Complete
    } else {
        SequenceStatus::Incomplete
    }
}

/// APC sequences end with ST (ESC \\).
fn is_complete_apc_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with("\x1b_") {
        return SequenceStatus::Complete;
    }
    if data.ends_with("\x1b\\") {
        SequenceStatus::Complete
    } else {
        SequenceStatus::Incomplete
    }
}

/// Upstream `parseUnmodifiedKittyPrintableCodepoint`.
fn parse_unmodified_kitty_printable_codepoint(sequence: &str) -> Option<u32> {
    // ^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$
    let body = sequence.strip_prefix("\x1b[")?.strip_suffix('u')?;
    let mut parts = body.split(':');
    let codepoint_part = parts.next()?;
    if codepoint_part.is_empty() || !codepoint_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match parts.next() {
        None => {}
        Some(shifted) => {
            // (?::\d*)? then (?::\d+)? — one optional (possibly empty), then
            // one optional non-empty.
            match parts.next() {
                None => {
                    if !shifted.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                }
                Some(base) => {
                    if shifted.is_empty() || !shifted.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    if base.is_empty() || !base.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                }
            }
        }
    }
    let codepoint: u32 = codepoint_part.parse().ok()?;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

/// Split accumulated buffer into complete sequences
/// (upstream `extractCompleteSequences`).
fn extract_complete_sequences(buffer: &str) -> (Vec<String>, String) {
    let mut sequences: Vec<String> = Vec::new();
    let mut pos = 0usize;

    while pos < buffer.len() {
        let remaining = &buffer[pos..];

        if remaining.starts_with(ESC) {
            // Grow the candidate one char at a time until complete; if the
            // buffer is exhausted first, the remainder stays buffered.
            let total_chars = remaining.chars().count();
            let mut seq_end_chars = 1usize;
            let mut end_byte = ESC.len_utf8();
            loop {
                if seq_end_chars > total_chars {
                    return (sequences, remaining.to_string());
                }
                let candidate = &remaining[..end_byte];
                match is_complete_sequence(candidate) {
                    SequenceStatus::Complete => {
                        // WezTerm sends the Escape key press as a raw ESC byte
                        // and the release as a full Kitty CSI-u; when the char
                        // after ESC ESC would begin a new escape sequence, emit
                        // only the first ESC and restart from the second.
                        if candidate == "\x1b\x1b" {
                            let next_char = remaining[end_byte..].chars().next();
                            if matches!(
                                next_char,
                                Some('[') | Some(']') | Some('O') | Some('P') | Some('_')
                            ) {
                                sequences.push(ESC.to_string());
                                pos += 1;
                                break;
                            }
                        }
                        sequences.push(candidate.to_string());
                        pos += end_byte;
                        break;
                    }
                    SequenceStatus::Incomplete => match remaining[end_byte..].char_indices().next()
                    {
                        Some((_, c)) => {
                            end_byte += c.len_utf8();
                            seq_end_chars += 1;
                        }
                        // Candidate covers the whole buffer and is still incomplete.
                        None => return (sequences, remaining.to_string()),
                    },
                    SequenceStatus::NotEscape => {
                        // Should not happen when starting with ESC.
                        sequences.push(candidate.to_string());
                        pos += end_byte;
                        break;
                    }
                }
            }
        } else {
            // Not an escape sequence - take a single character.
            let ch = remaining.chars().next().expect("non-empty");
            sequences.push(ch.to_string());
            pos += ch.len_utf8();
        }
    }

    (sequences, String::new())
}

/// Events emitted while processing input (upstream `data` / `paste` events).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StdinEvent {
    Data(String),
    Paste(String),
}

/// Result of one [`StdinBuffer::process`] call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub events: Vec<StdinEvent>,
    /// When the buffer holds an incomplete sequence, the caller schedules a
    /// flush after this many milliseconds (upstream sets a `setTimeout`).
    pub flush_after_ms: Option<u64>,
}

/// Options mirroring upstream `StdinBufferOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdinBufferOptions {
    /// Maximum time to wait for an incomplete sequence such as CSI or mouse.
    pub timeout_ms: Option<u64>,
    /// Maximum time to wait after a lone ESC before treating it as Escape.
    pub escape_timeout_ms: Option<u64>,
}

/// Buffers stdin input and splits it into complete sequences.
#[derive(Debug)]
pub struct StdinBuffer {
    buffer: String,
    timeout_ms: u64,
    escape_timeout_ms: u64,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_printable_codepoint: Option<u32>,
}

impl Default for StdinBuffer {
    fn default() -> Self {
        Self::new(StdinBufferOptions::default())
    }
}

impl StdinBuffer {
    pub fn new(options: StdinBufferOptions) -> Self {
        Self {
            buffer: String::new(),
            timeout_ms: options.timeout_ms.unwrap_or(DEFAULT_SEQUENCE_TIMEOUT_MS),
            escape_timeout_ms: options
                .escape_timeout_ms
                .unwrap_or(DEFAULT_ESCAPE_TIMEOUT_MS),
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_printable_codepoint: None,
        }
    }

    /// Upstream `process(data: string | Buffer)`. Bytes follow the high-byte
    /// rule: a single byte > 127 becomes ESC + (byte - 128).
    pub fn process_bytes(&mut self, data: &[u8]) -> ProcessOutcome {
        if data.len() == 1 && data[0] > 127 {
            let byte = data[0] - 128;
            let ch = char::from_u32(u32::from(byte)).unwrap_or('\0');
            let str = format!("{ESC}{ch}");
            self.process(&str)
        } else {
            match std::str::from_utf8(data) {
                Ok(text) => self.process(text),
                Err(error) => self
                    .process(std::str::from_utf8(&data[..error.valid_up_to()]).unwrap_or_default()),
            }
        }
    }

    pub fn process(&mut self, data: &str) -> ProcessOutcome {
        if data.is_empty() && self.buffer.is_empty() {
            let mut events = Vec::new();
            self.emit_data_sequence(String::new(), &mut events);
            return ProcessOutcome {
                events,
                flush_after_ms: None,
            };
        }

        self.buffer.push_str(data);

        if self.paste_mode {
            self.paste_buffer.push_str(&self.buffer);
            self.buffer.clear();

            if let Some(end_index) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_string();
                let remaining =
                    self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                let mut events = vec![StdinEvent::Paste(pasted_content)];
                if !remaining.is_empty() {
                    events.extend(self.process(&remaining).events);
                }
                return ProcessOutcome {
                    events,
                    flush_after_ms: None,
                };
            }
            return ProcessOutcome {
                events: Vec::new(),
                flush_after_ms: None,
            };
        }

        if let Some(start_index) = self.buffer.find(BRACKETED_PASTE_START) {
            let mut events = Vec::new();
            if start_index > 0 {
                let before_paste = self.buffer[..start_index].to_string();
                let (sequences, _) = extract_complete_sequences(&before_paste);
                for sequence in sequences {
                    self.emit_data_sequence(sequence, &mut events);
                }
            }

            self.pending_kitty_printable_codepoint = None;
            self.buffer = self.buffer[start_index + BRACKETED_PASTE_START.len()..].to_string();
            self.paste_mode = true;
            self.paste_buffer = std::mem::take(&mut self.buffer);

            if let Some(end_index) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_string();
                let remaining =
                    self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                events.push(StdinEvent::Paste(pasted_content));
                if !remaining.is_empty() {
                    events.extend(self.process(&remaining).events);
                }
            }
            return ProcessOutcome {
                events,
                flush_after_ms: None,
            };
        }

        let (sequences, remainder) = extract_complete_sequences(&self.buffer);
        self.buffer = remainder;

        let mut events = Vec::new();
        for sequence in sequences {
            self.emit_data_sequence(sequence, &mut events);
        }

        let flush_after_ms = if self.buffer.is_empty() {
            None
        } else if self.buffer == ESC.to_string() {
            Some(self.escape_timeout_ms)
        } else {
            Some(self.timeout_ms)
        };

        ProcessOutcome {
            events,
            flush_after_ms,
        }
    }

    fn emit_data_sequence(&mut self, sequence: String, events: &mut Vec<StdinEvent>) {
        let raw_codepoint = match sequence.chars().count() {
            1 => sequence.chars().next().map(|c| c as u32),
            _ => None,
        };
        if raw_codepoint.is_some() && raw_codepoint == self.pending_kitty_printable_codepoint {
            self.pending_kitty_printable_codepoint = None;
            return;
        }

        self.pending_kitty_printable_codepoint =
            parse_unmodified_kitty_printable_codepoint(&sequence);
        events.push(StdinEvent::Data(sequence));
    }

    /// Upstream public `flush`: the raw buffered remainder without dedup.
    pub fn flush(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let sequences = vec![std::mem::take(&mut self.buffer)];
        self.pending_kitty_printable_codepoint = None;
        sequences
    }

    /// The upstream timer path: flush the remainder and route it through
    /// `emitDataSequence` (Kitty printable dedup applies).
    pub fn flush_emit(&mut self) -> Vec<StdinEvent> {
        let mut events = Vec::new();
        for sequence in self.flush() {
            self.emit_data_sequence(sequence, &mut events);
        }
        events
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.paste_mode = false;
        self.paste_buffer.clear();
        self.pending_kitty_printable_codepoint = None;
    }

    pub fn get_buffer(&self) -> &str {
        &self.buffer
    }

    pub fn destroy(&mut self) {
        self.clear();
    }
}
