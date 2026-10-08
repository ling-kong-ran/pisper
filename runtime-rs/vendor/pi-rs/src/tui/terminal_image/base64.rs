//! Node `Buffer` base64 semantics used by upstream `terminal-image.ts`
//! (`Buffer.from(data, "base64")` for header probing,
//! `Buffer.byteLength(data, "base64")` for the iTerm2 `size=` field and
//! `Buffer.from(name).toString("base64")` for the iTerm2 `name=` field).
//!
//! Every rule here was captured empirically from the runtime that runs the
//! upstream module and is locked by `tests::oracle_base64` against
//! `tests/fixtures/terminal_image_oracle/oracle_output.json` (29 probe inputs):
//!
//! - decoding skips characters outside the standard + base64url alphabets
//!   (whitespace and garbage included),
//! - decoding stops at the first `=` padding character anywhere in the input,
//! - leftover bits of a final partial group are dropped,
//! - `byte_length` strips at most two trailing `=` and returns
//!   `(len - stripped) * 3 / 4` on the RAW character count (invalid and
//!   whitespace characters included), matching node's `string_bytes.cc`.
//!
//! Divergence (disclosed): JS string length counts UTF-16 code units; here
//! `char`s/bytes are used. Inputs in the image pipeline are ASCII base64, so
//! the counts coincide for every reachable input.

/// Decode `input` with node's forgiving base64 rules (see module docs).
pub fn decode(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        if ch == '=' {
            break;
        }
        let value = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => continue,
        };
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    out
}

/// Node `Buffer.byteLength(input, "base64")` (see module docs).
pub fn byte_length(input: &str) -> usize {
    let bytes = input.as_bytes();
    let mut size = bytes.len();
    if size > 1 && bytes[size - 1] == b'=' {
        size -= 1;
    }
    if size > 1 && bytes[size - 1] == b'=' {
        size -= 1;
    }
    size * 3 / 4
}

const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode with the standard alphabet and `=` padding — the inverse of what
/// `Buffer#toString("base64")` produces (used for the iTerm2 `name=` field).
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let group = (b0 << 16) | (b1 << 8) | b2;
        out.push(STD_ALPHABET[(group >> 18) as usize & 0x3f] as char);
        out.push(STD_ALPHABET[(group >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(STD_ALPHABET[(group >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(STD_ALPHABET[group as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}
