//! Strict LF-only framing from upstream `modes/rpc/jsonl.ts`.
//!
//! Byte chunks retain Node StringDecoder's incomplete UTF-8 suffix, including
//! malformed leading sequences; text chunks bypass (do not flush) that decoder.
//! UTF-16 text and returned lines are lossless even for lone surrogate units.
//! The owning stream adapter feeds this reader and stops on detach. This module
//! does not own, close, or spawn a task for the caller's stdin/stream.

use crate::serde_support::{order_json_object_keys, to_json_string_with_js_numbers};
use crate::tui::utf16::Utf16Text;
use serde::Serialize;

/// Compact JSON plus exactly one LF. JSON number spelling and indexed property
/// ordering follow JavaScript. The input is a serde-representable JSON value,
/// not arbitrary JS objects with getters/toJSON/undefined/cycles/BigInt.
pub fn serialize_json_line<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(value)?;
    order_json_object_keys(&mut value);
    let mut line = to_json_string_with_js_numbers(&value)?;
    line.push('\n');
    Ok(line)
}

#[derive(Default)]
struct Utf8StreamDecoder {
    pending: Vec<u8>,
    total: usize,
}

fn sequence_len(byte: u8) -> usize {
    match byte {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 0,
    }
}
fn continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}
impl Utf8StreamDecoder {
    fn write(&mut self, mut bytes: &[u8]) -> String {
        let mut output = String::new();
        if !self.pending.is_empty() {
            let count = bytes
                .iter()
                .take(self.total - self.pending.len())
                .take_while(|&&byte| continuation(byte))
                .count();
            self.pending.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.pending.len() < self.total && bytes.is_empty() {
                return output;
            }
            // A new non-continuation flushes the incomplete prefix, but is
            // itself decoded below. Lossy conversion applies maximal-subpart
            // replacement just like Buffer.toString; validation must not run
            // before the pending sequence reaches its expected byte length.
            output.push_str(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
        let mut cut = bytes.len();
        for distance in 1..=bytes.len().min(3) {
            let index = bytes.len() - distance;
            let byte = bytes[index];
            let total = sequence_len(byte);
            if total > distance {
                cut = index;
                self.total = total;
                self.pending.extend_from_slice(&bytes[index..]);
                break;
            }
            if !continuation(byte) {
                break;
            }
        }
        output.push_str(&String::from_utf8_lossy(&bytes[..cut]));
        output
    }
    fn end(&mut self) -> String {
        let output = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        output
    }
}

/// The data/end/detach state of upstream `attachJsonlLineReader`.
/// Empty lines are emitted; EOF emits only a nonempty tail; exactly one trailing
/// CR is removed. U+0085/U+2028/U+2029 and standalone CR never split records.
/// No length cap is imposed: upstream's framing helper has none.
pub struct JsonlLineReader {
    decoder: Utf8StreamDecoder,
    buffer: Vec<u16>,
    attached: bool,
}
impl Default for JsonlLineReader {
    fn default() -> Self {
        Self::new()
    }
}
impl JsonlLineReader {
    pub fn new() -> Self {
        Self {
            decoder: Utf8StreamDecoder::default(),
            buffer: Vec::new(),
            attached: true,
        }
    }
    pub fn is_attached(&self) -> bool {
        self.attached
    }
    pub fn detach(&mut self) {
        self.attached = false;
    }
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> Vec<Utf16Text> {
        if !self.attached {
            return Vec::new();
        }
        let text = self.decoder.write(bytes);
        self.feed_text(&text)
    }
    pub fn feed_text(&mut self, text: &str) -> Vec<Utf16Text> {
        self.feed_utf16(&Utf16Text::from(text))
    }
    pub fn feed_utf16(&mut self, text: &Utf16Text) -> Vec<Utf16Text> {
        if !self.attached {
            return Vec::new();
        }
        self.buffer.extend_from_slice(text.as_units());
        let mut start = 0;
        let mut lines = Vec::new();
        for (index, &unit) in self.buffer.iter().enumerate() {
            if unit == u16::from(b'\n') {
                lines.push(emit_line(&self.buffer[start..index]));
                start = index + 1;
            }
        }
        self.buffer.drain(..start);
        lines
    }
    pub fn finish(&mut self) -> Vec<Utf16Text> {
        if !self.attached {
            return Vec::new();
        }
        self.buffer.extend(self.decoder.end().encode_utf16());
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            let line = emit_line(&self.buffer);
            self.buffer.clear();
            vec![line]
        }
    }
}
fn emit_line(units: &[u16]) -> Utf16Text {
    Utf16Text::from_units(
        units
            .strip_suffix(&[u16::from(b'\r')])
            .unwrap_or(units)
            .to_vec(),
    )
}

#[cfg(test)]
#[path = "jsonl_tests.rs"]
mod tests;
