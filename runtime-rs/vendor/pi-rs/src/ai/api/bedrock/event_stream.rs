//! Minimal AWS event-stream frame decoder for the Bedrock ConverseStream
//! response, plus the base64 codec the redacted-reasoning replay needs.
//!
//! Wire framing findings (what the JS SDK hides behind
//! `@smithy/eventstream-codec`): every ConverseStream HTTP 200 response is a
//! sequence of binary frames, `content-type:
//! application/vnd.amazon.eventstream`. Each frame is
//!
//! ```text
//! u32 BE  total byte-length of the message (prelude + headers + payload + CRC)
//! u32 BE  headers byte-length
//! u32 BE  CRC-32 (IEEE) of the 8 prelude bytes above
//! [headers byte-length]  sequence of typed headers
//! [payload]              the JSON event document
//! u32 BE  CRC-32 (IEEE) of every byte before it
//! ```
//!
//! Headers are `u8 name-length, name bytes, u8 value-type, value`, where the
//! value types are `0 true, 1 false, 2 byte, 3 short, 4 integer, 5 long,
//! 6 byte-array, 7 string, 8 timestamp, 9 uuid`. ConverseStream frames carry
//! the string headers `:message-type` (`event` | `exception` | `error`),
//! `:event-type` (the union member, e.g. `contentBlockDelta`),
//! `:exception-type` (for exceptions, e.g. `throttlingException`), and for
//! `:message-type: error` frames `:error-code` / `:error-message`. The event
//! payload is the union member's own JSON (the member key comes from the
//! `:event-type` header); the JS SDK's unmarshaller re-keys it as
//! `{ [<event-type>]: <payload> }` — which is exactly the shape upstream
//! `bedrock-converse-stream.ts` iterates. Response frames are not signed
//! (no chunk-signature headers), so only the CRCs are verified here.

/// Guard against garbage headers announcing absurd frame sizes (the decoder
/// would otherwise try to buffer `total_length` up front). Real Bedrock
/// event frames are far below this.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

const MIN_FRAME_BYTES: usize = 16; // 12 prelude + 4 message CRC

/// One decoded event-stream frame: the routing headers plus the raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// `:message-type`: `"event"`, `"exception"`, or `"error"`.
    pub message_type: String,
    /// `:event-type`: the ConverseStreamOutput union member for event frames.
    pub event_type: Option<String>,
    /// `:exception-type`: the modeled exception name for exception frames.
    pub exception_type: Option<String>,
    /// `:error-code`: present on `:message-type: error` frames.
    pub error_code: Option<String>,
    /// `:error-message`: present on `:message-type: error` frames.
    pub error_message: Option<String>,
    /// Raw payload bytes (JSON for ConverseStream events and exceptions).
    pub payload: Vec<u8>,
}

/// Incremental frame decoder: feed network chunks, get whole frames.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes `data`, decoding every complete frame it completes. A
    /// malformed or CRC-failing frame fails the whole stream (the JS SDK's
    /// event unmarshaller fails the iterator the same way), so `Err` is
    /// terminal.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>, String> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if self.buffer.len() < 12 {
                break;
            }
            let total_length = u32::from_be_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            let headers_length = u32::from_be_bytes([
                self.buffer[4],
                self.buffer[5],
                self.buffer[6],
                self.buffer[7],
            ]) as usize;
            if !(MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&total_length) {
                return Err(format!("Invalid event stream frame length: {total_length}"));
            }
            if self.buffer.len() < total_length {
                break;
            }
            let prelude_crc = u32::from_be_bytes([
                self.buffer[8],
                self.buffer[9],
                self.buffer[10],
                self.buffer[11],
            ]);
            if crc32(&self.buffer[..8]) != prelude_crc {
                return Err("Event stream frame prelude CRC mismatch".to_string());
            }
            if headers_length > total_length.saturating_sub(MIN_FRAME_BYTES) {
                return Err(format!(
                    "Invalid event stream header length: {headers_length}"
                ));
            }
            let message_crc = u32::from_be_bytes([
                self.buffer[total_length - 4],
                self.buffer[total_length - 3],
                self.buffer[total_length - 2],
                self.buffer[total_length - 1],
            ]);
            if crc32(&self.buffer[..total_length - 4]) != message_crc {
                return Err("Event stream frame message CRC mismatch".to_string());
            }
            let header_bytes = &self.buffer[12..12 + headers_length];
            let payload = self.buffer[12 + headers_length..total_length - 4].to_vec();
            let headers = parse_headers(header_bytes)?;
            let take = |name: &str| {
                headers
                    .iter()
                    .find_map(|(key, value)| (key == name).then(|| value.clone()))
            };
            frames.push(Frame {
                message_type: take(":message-type").unwrap_or_default(),
                event_type: take(":event-type"),
                exception_type: take(":exception-type"),
                error_code: take(":error-code"),
                error_message: take(":error-message"),
                payload,
            });
            self.buffer.drain(..total_length);
        }
        Ok(frames)
    }

    /// Called at end of stream: leftover bytes are a truncated frame.
    pub fn finish(&mut self) -> Result<(), String> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err("Event stream ended mid-frame".to_string())
        }
    }
}

/// Parses one frame's header block into `(name, string value)` pairs. Only
/// string headers matter for ConverseStream routing; other value types are
/// skipped, preserving their bytes as opaque.
fn parse_headers(mut bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
        if bytes.len() < length {
            return Err("Truncated header value".to_string());
        }
        let (value, rest) = bytes.split_at(length);
        *bytes = rest;
        Ok(value)
    }

    let mut headers = Vec::new();
    while !bytes.is_empty() {
        let name_length = u16::from(take(&mut bytes, 1)?[0]) as usize;
        let name = String::from_utf8_lossy(take(&mut bytes, name_length)?).into_owned();
        let value_type = take(&mut bytes, 1)?[0];
        match value_type {
            0 | 1 => {} // true / false: no value bytes
            2 => {
                take(&mut bytes, 1)?;
            }
            3 => {
                take(&mut bytes, 2)?;
            }
            4 => {
                take(&mut bytes, 4)?;
            }
            5 | 8 => {
                // 5 = long, 8 = timestamp (i64 epoch millis) — both 8 bytes
                // per the AWS event-stream spec.
                take(&mut bytes, 8)?;
            }
            6 | 7 => {
                let length = u16::from_be_bytes(take(&mut bytes, 2)?.try_into().unwrap()) as usize;
                let value = take(&mut bytes, length)?;
                if value_type == 7 {
                    headers.push((name.clone(), String::from_utf8_lossy(value).into_owned()));
                }
            }
            9 => {
                take(&mut bytes, 16)?;
            }
            other => return Err(format!("Unknown header value type: {other}")),
        }
    }
    Ok(headers)
}

/// CRC-32 (IEEE, reflected, poly 0xEDB88320) — the checksum the AWS
/// event-stream prelude and message trailer use. Computed without a lookup
/// table: 8 inversions per byte, negligible next to the JSON work.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Base64 (standard alphabet, padded) decoder — the port's `atob` for stored
/// redacted-reasoning payloads. Strict: rejects characters outside the
/// alphabet, misplaced padding, and impossible lengths.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    fn value_of(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let trimmed = input.trim();
    let unpadded = trimmed.trim_end_matches('=');
    let padding = trimmed.len().saturating_sub(unpadded.len());
    if padding > 2 || !trimmed.len().is_multiple_of(4) {
        return Err("Invalid base64 length".to_string());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for (index, &byte) in trimmed.as_bytes().iter().enumerate() {
        if byte == b'=' {
            // Padding is only legal in the final two positions.
            if index + padding != trimmed.len() {
                return Err("Invalid base64 padding".to_string());
            }
            break;
        }
        let value = value_of(byte).ok_or("Invalid base64 character")?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xFF) as u8);
        }
    }
    // Remaining bits must be zero-padding, not payload.
    if bits > 0 && accumulator & ((1 << bits) - 1) != 0 {
        return Err("Invalid base64 trailing bits".to_string());
    }
    Ok(out)
}

/// Base64 (standard alphabet, padded) encoder — the port's `btoa` for
/// flushing redacted-reasoning chunks into `thinkingSignature`.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&b| u32::from(b));
        let b2 = chunk.get(2).map_or(0, |&b| u32::from(b));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(triple >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one wire frame: prelude + headers + payload + CRCs.
    pub(super) fn encode_frame(headers: &[(&str, HeaderValue<'_>)], payload: &[u8]) -> Vec<u8> {
        let mut header_bytes = Vec::new();
        for (name, value) in headers {
            header_bytes.push(name.len() as u8);
            header_bytes.extend_from_slice(name.as_bytes());
            header_bytes.push(value.type_id());
            header_bytes.extend_from_slice(&value.encoded());
        }
        let total = 12 + header_bytes.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total);
        frame.extend_from_slice(&(total as u32).to_be_bytes());
        frame.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame.extend_from_slice(&header_bytes);
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame
    }

    /// Header value builder for the test encoder.
    #[derive(Clone, Copy)]
    pub(super) enum HeaderValue<'a> {
        String(&'a str),
        True,
        Integer(i32),
        Timestamp(i64),
    }

    impl HeaderValue<'_> {
        fn type_id(&self) -> u8 {
            match self {
                HeaderValue::String(_) => 7,
                HeaderValue::True => 0,
                HeaderValue::Integer(_) => 4,
                HeaderValue::Timestamp(_) => 8,
            }
        }

        fn encoded(&self) -> Vec<u8> {
            match self {
                HeaderValue::String(text) => {
                    let mut out = (text.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(text.as_bytes());
                    out
                }
                HeaderValue::True => Vec::new(),
                HeaderValue::Integer(value) => value.to_be_bytes().to_vec(),
                HeaderValue::Timestamp(value) => value.to_be_bytes().to_vec(),
            }
        }
    }

    #[test]
    fn crc32_matches_known_vectors() {
        // Standard CRC-32/ISO-HDLC check values.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn decodes_event_frames_split_across_chunks() {
        let payload = br#"{"contentBlockIndex":0,"delta":{"text":"Hi"}}"#;
        let frame = encode_frame(
            &[
                (":message-type", HeaderValue::String("event")),
                (":event-type", HeaderValue::String("contentBlockDelta")),
                (":content-type", HeaderValue::String("application/json")),
            ],
            payload,
        );
        let mut decoder = FrameDecoder::new();
        // Split at awkward boundaries: mid-prelude, mid-header, mid-payload.
        assert!(decoder.decode(&frame[..3]).unwrap().is_empty());
        let batch = decoder.decode(&frame[3..20]).unwrap();
        assert!(batch.is_empty());
        let batch = decoder.decode(&frame[20..]).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].message_type, "event");
        assert_eq!(batch[0].event_type.as_deref(), Some("contentBlockDelta"));
        assert_eq!(batch[0].exception_type, None);
        assert_eq!(batch[0].payload, payload.to_vec());
        decoder.finish().unwrap();
    }

    #[test]
    fn decodes_exception_and_error_frames() {
        let exception = encode_frame(
            &[
                (":message-type", HeaderValue::String("exception")),
                (
                    ":exception-type",
                    HeaderValue::String("throttlingException"),
                ),
                (":content-type", HeaderValue::String("application/json")),
            ],
            br#"{"__type":"throttlingException","message":"Rate limited"}"#,
        );
        let error = encode_frame(
            &[
                (":message-type", HeaderValue::String("error")),
                (
                    ":error-code",
                    HeaderValue::String("ModelStreamErrorException"),
                ),
                (
                    ":error-message",
                    HeaderValue::String("Model stream terminated unexpectedly."),
                ),
            ],
            b"",
        );
        let mut decoder = FrameDecoder::new();
        let mut frames = decoder.decode(&exception).unwrap();
        frames.extend(decoder.decode(&error).unwrap());
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].message_type, "exception");
        assert_eq!(
            frames[0].exception_type.as_deref(),
            Some("throttlingException")
        );
        assert_eq!(frames[1].message_type, "error");
        assert_eq!(
            frames[1].error_code.as_deref(),
            Some("ModelStreamErrorException")
        );
        assert_eq!(
            frames[1].error_message.as_deref(),
            Some("Model stream terminated unexpectedly.")
        );
    }

    #[test]
    fn skips_non_string_header_value_types() {
        let frame = encode_frame(
            &[
                (":message-type", HeaderValue::String("event")),
                (":event-type", HeaderValue::String("metadata")),
                (":checksum-crc32", HeaderValue::Integer(123456)),
                (":final", HeaderValue::True),
            ],
            b"{}",
        );
        let mut decoder = FrameDecoder::new();
        let frames = decoder.decode(&frame).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event_type.as_deref(), Some("metadata"));
    }

    #[test]
    fn timestamp_header_consumes_eight_bytes_per_spec() {
        // Spec vector (AWS event-stream encoding + @smithy/eventstream-codec):
        // header value type 8 (TIMESTAMP) is 8 bytes (i64 epoch millis). A
        // 4-byte read would desynchronize header parsing and fail the stream.
        let frame = encode_frame(
            &[
                (":message-type", HeaderValue::String("event")),
                (":event-type", HeaderValue::String("metadata")),
                (":x-ts", HeaderValue::Timestamp(1_758_240_000_000)),
                // Headers after the timestamp must still parse.
                (":content-type", HeaderValue::String("application/json")),
                (":final", HeaderValue::True),
                (":x-long", HeaderValue::Timestamp(i64::MAX)),
            ],
            b"{\"usage\":{}}",
        );
        let mut decoder = FrameDecoder::new();
        let frames = decoder.decode(&frame).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, "event");
        assert_eq!(frames[0].event_type.as_deref(), Some("metadata"));
        // The 8 timestamp bytes were skipped exactly: the headers after it
        // (`:content-type`, `:final`, `:x-long`) are intact and the payload is
        // untouched. A 4-byte read would desynchronize the block and drop or
        // corrupt every header after `:x-ts`.
        assert_eq!(frames[0].payload, b"{\"usage\":{}}".to_vec());
        // And the decoder advances past the whole frame: the next frame in the
        // same byte stream still parses.
        let next = encode_frame(&[(":message-type", HeaderValue::String("event"))], b"{}");
        let mut stream = frame.clone();
        stream.extend_from_slice(&next);
        let mut decoder = FrameDecoder::new();
        let frames = decoder.decode(&stream).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].message_type, "event");
        decoder.finish().unwrap();
    }

    #[test]
    fn corrupt_frames_fail_the_stream() {
        let payload = b"{}";
        let mut frame = encode_frame(&[(":message-type", HeaderValue::String("event"))], payload);
        // Flip a prelude CRC byte.
        let mut corrupt = frame.clone();
        corrupt[10] ^= 0xFF;
        let mut decoder = FrameDecoder::new();
        assert!(decoder.decode(&corrupt).is_err());

        // Flip a payload byte (breaks the message CRC).
        let last = frame.len() - 2;
        frame[last] ^= 0x01;
        let mut decoder = FrameDecoder::new();
        assert!(decoder.decode(&frame).is_err());

        // Announced length beyond the guard.
        let mut decoder = FrameDecoder::new();
        assert!(decoder
            .decode(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0])
            .is_err());

        // Trailing partial frame fails at finish().
        let mut decoder = FrameDecoder::new();
        decoder
            .decode(&encode_frame(
                &[(":message-type", HeaderValue::String("event"))],
                payload,
            ))
            .unwrap();
        decoder.decode(&[0, 0, 0]).unwrap();
        assert!(decoder.finish().is_err());
    }

    #[test]
    fn base64_codec_round_trips_and_validates() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            (0u8..=255).collect(),
        ];
        for case in cases {
            let encoded = base64_encode(&case);
            assert_eq!(base64_decode(&encoded).unwrap(), case, "{encoded}");
        }
        // The oracle fixture decodes to the bytes Bedrock streamed.
        assert_eq!(
            base64_decode("cnNuXzVaVnJpZjRKMGJYSXFtV2RsZWRqN1FJRmVOaWtSUWJF").unwrap(),
            b"rsn_5ZVrif4J0bXIqmWdledj7QIFeNikRQbE".to_vec()
        );
        // Padding and character strictness (atob would throw the same way).
        assert!(base64_decode("A").is_err());
        assert!(base64_decode("AAAA=A").is_err());
        assert!(base64_decode("!!!").is_err());
        assert!(base64_decode("####").is_err());
    }
}
