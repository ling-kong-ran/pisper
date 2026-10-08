//! Length-prefixed frame codec. Port of `packages/protocol/src/framing.ts`.
//!
//! Divergence D7 (parent module docs): payload assembly uses one growable
//! buffer instead of upstream's 64 KiB block list; emitted frames are
//! byte-identical.

const FRAME_HEADER_LENGTH: usize = 4;

/// Default upper bound for one framed CBOR payload.
pub const DEFAULT_MAX_FRAME_LENGTH: u64 = 16 * 1024 * 1024;

/// Upstream `FrameDecoderOptions`; `maxFrameLength` is `u64`, so the
/// non-integer `RangeError` cases are type-level impossible (divergence D4)
/// while range overflows still fail in [`FrameDecoder::new`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameDecoderOptions {
    pub max_frame_length: Option<u64>,
}

impl FrameDecoderOptions {
    pub fn with_max_frame_length(mut self, value: u64) -> FrameDecoderOptions {
        self.max_frame_length = Some(value);
        self
    }
}

/// Port of upstream `FrameError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameError {
    message: String,
}

impl FrameError {
    pub fn new(message: impl Into<String>) -> FrameError {
        FrameError {
            message: message.into(),
        }
    }

    /// The exact upstream `error.message` text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FrameError {}

fn resolve_max_frame_length(options: FrameDecoderOptions) -> Result<u64, RangeError> {
    let value = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
    if value > u64::from(u32::MAX) {
        return Err(RangeError::new(format!(
            "maxFrameLength must be an integer between 0 and {}",
            u32::MAX
        )));
    }
    Ok(value)
}

/// Re-exported so callers can match the construction error without reaching
/// into the CBOR module.
pub use super::cbor::options::RangeError;

/// Prefixes a payload with its unsigned 32-bit big-endian byte length.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, RangeError> {
    if payload.len() > u32::MAX as usize {
        return Err(RangeError::new(
            "Frame payload exceeds the unsigned 32-bit length limit",
        ));
    }
    let length = payload.len() as u32;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecoderState {
    Open,
    Ended,
    Failed,
}

/// Incrementally splits arbitrary byte chunks into length-prefixed payloads.
#[derive(Debug)]
pub struct FrameDecoder {
    header: [u8; FRAME_HEADER_LENGTH],
    header_length: usize,
    max_frame_length: u64,
    payload: Vec<u8>,
    expected_payload_length: Option<u64>,
    payload_length: u64,
    state: DecoderState,
}

impl FrameDecoder {
    /// Returns `Err` when `maxFrameLength` exceeds the unsigned 32-bit range
    /// (upstream `RangeError`).
    pub fn new(options: FrameDecoderOptions) -> Result<FrameDecoder, RangeError> {
        Ok(FrameDecoder {
            header: [0; FRAME_HEADER_LENGTH],
            header_length: 0,
            max_frame_length: resolve_max_frame_length(options)?,
            payload: Vec::new(),
            expected_payload_length: None,
            payload_length: 0,
            state: DecoderState::Open,
        })
    }

    /// Pushes one arbitrary chunk; returns every payload completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        match self.state {
            DecoderState::Ended => {
                return Err(FrameError::new("Frame decoder has ended"));
            }
            DecoderState::Failed => {
                return Err(FrameError::new("Frame decoder has failed"));
            }
            DecoderState::Open => {}
        }

        let mut frames = Vec::new();
        let mut chunk_offset = 0;
        while chunk_offset < chunk.len() {
            if self.expected_payload_length.is_none() {
                let header_bytes =
                    (FRAME_HEADER_LENGTH - self.header_length).min(chunk.len() - chunk_offset);
                self.header[self.header_length..self.header_length + header_bytes]
                    .copy_from_slice(&chunk[chunk_offset..chunk_offset + header_bytes]);
                self.header_length += header_bytes;
                chunk_offset += header_bytes;
                if self.header_length < FRAME_HEADER_LENGTH {
                    continue;
                }

                let frame_length = u64::from(u32::from_be_bytes(self.header));
                self.header_length = 0;
                if frame_length > self.max_frame_length {
                    return Err(self.fail(format!(
                        "Frame length {} exceeds configured limit of {}",
                        frame_length, self.max_frame_length
                    )));
                }
                if frame_length == 0 {
                    frames.push(Vec::new());
                    continue;
                }
                self.expected_payload_length = Some(frame_length);
                self.payload.clear();
                self.payload.reserve(frame_length.min(64 * 1024) as usize);
                self.payload_length = 0;
            }

            let expected_payload_length = self
                .expected_payload_length
                .expect("payload length set above");
            while chunk_offset < chunk.len() && self.payload_length < expected_payload_length {
                let payload_bytes = (expected_payload_length - self.payload_length)
                    .min((chunk.len() - chunk_offset) as u64);
                let payload_bytes = payload_bytes as usize;
                self.payload
                    .extend_from_slice(&chunk[chunk_offset..chunk_offset + payload_bytes]);
                self.payload_length += payload_bytes as u64;
                chunk_offset += payload_bytes;
            }
            if self.payload_length == expected_payload_length {
                frames.push(std::mem::take(&mut self.payload));
                self.expected_payload_length = None;
                self.payload_length = 0;
            }
        }
        Ok(frames)
    }

    /// Ends the stream; a truncated frame at the boundary is a failure.
    pub fn end(&mut self) -> Result<(), FrameError> {
        match self.state {
            DecoderState::Ended => {
                return Err(FrameError::new("Frame decoder has ended"));
            }
            DecoderState::Failed => {
                return Err(FrameError::new("Frame decoder has failed"));
            }
            DecoderState::Open => {}
        }
        if self.header_length != 0 || self.expected_payload_length.is_some() {
            return Err(self.fail("Truncated frame at end of stream".to_string()));
        }
        self.state = DecoderState::Ended;
        Ok(())
    }

    fn fail(&mut self, message: String) -> FrameError {
        self.state = DecoderState::Failed;
        self.header_length = 0;
        self.payload.clear();
        self.expected_payload_length = None;
        self.payload_length = 0;
        FrameError::new(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concatenate(chunks: &[&[u8]]) -> Vec<u8> {
        let mut result = Vec::new();
        for chunk in chunks {
            result.extend_from_slice(chunk);
        }
        result
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn new_decoder() -> FrameDecoder {
        FrameDecoder::new(FrameDecoderOptions::default()).unwrap()
    }

    #[test]
    fn prefixes_payloads_with_four_byte_big_endian_length() {
        assert_eq!(
            hex(&encode_frame(&[0xaa, 0xbb, 0xcc]).unwrap()),
            "00000003aabbcc"
        );
        assert_eq!(hex(&encode_frame(&[]).unwrap()), "00000000");
    }

    #[test]
    fn decodes_fragmented_coalesced_and_empty_frames_in_order() {
        let wire = concatenate(&[
            &encode_frame(&[1, 2, 3]).unwrap(),
            &encode_frame(&[]).unwrap(),
            &encode_frame(&[4]).unwrap(),
        ]);
        let mut decoder = new_decoder();
        let mut frames = Vec::new();
        for byte in &wire {
            frames.extend(decoder.push(&[*byte]).unwrap());
        }
        decoder.end().unwrap();
        assert_eq!(frames, [vec![1, 2, 3], Vec::new(), vec![4]]);

        let mut coalesced = new_decoder();
        assert_eq!(coalesced.push(&wire).unwrap(), frames);
        coalesced.end().unwrap();
    }

    #[test]
    fn assembles_payloads_spanning_multiple_internal_blocks() {
        // Upstream 64 KiB block boundary (70_000 bytes, D7).
        let payload: Vec<u8> = (0..70_000).map(|index| (index % 251) as u8).collect();
        let wire = encode_frame(&payload).unwrap();
        let mut decoder = new_decoder();
        let mut frames = decoder.push(&wire[0..101]).unwrap();
        frames.extend(decoder.push(&wire[101..65_541]).unwrap());
        frames.extend(decoder.push(&wire[65_541..]).unwrap());
        decoder.end().unwrap();
        assert_eq!(frames, [payload]);
    }

    #[test]
    fn handles_every_split_point_across_a_frame() {
        let wire = encode_frame(&[10, 20, 30, 40]).unwrap();
        for split in 0..=wire.len() {
            let mut decoder = new_decoder();
            let mut frames = decoder.push(&wire[..split]).unwrap();
            frames.extend(decoder.push(&wire[split..]).unwrap());
            decoder.end().unwrap();
            assert_eq!(frames, [vec![10, 20, 30, 40]], "split {split}");
        }
    }

    #[test]
    fn copies_payload_bytes_instead_of_aliasing_input_chunks() {
        let mut chunk = encode_frame(&[1, 2, 3]).unwrap();
        let mut decoder = new_decoder();
        let frames = decoder.push(&chunk).unwrap();
        chunk.fill(9);
        assert_eq!(frames, [vec![1, 2, 3]]);
    }

    #[test]
    fn accepts_empty_chunks_and_a_clean_empty_stream() {
        let mut decoder = new_decoder();
        assert_eq!(decoder.push(&[]).unwrap(), Vec::<Vec<u8>>::new());
        assert!(decoder.end().is_ok());
    }

    #[test]
    fn rejects_truncated_streams_at_end() {
        let mut decoder = new_decoder();
        assert_eq!(decoder.push(&[0, 0, 0]).unwrap(), Vec::<Vec<u8>>::new());
        assert_eq!(
            decoder.end().unwrap_err().message(),
            "Truncated frame at end of stream"
        );

        let mut decoder = new_decoder();
        assert_eq!(
            decoder.push(&[0, 0, 0, 2, 1]).unwrap(),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            decoder.end().unwrap_err().message(),
            "Truncated frame at end of stream"
        );
    }

    #[test]
    fn rejects_oversized_declared_length_as_soon_as_header_is_complete() {
        let mut decoder =
            FrameDecoder::new(FrameDecoderOptions::default().with_max_frame_length(3)).unwrap();
        assert_eq!(
            decoder.push(&[0, 0, 0, 4]).unwrap_err().message(),
            "Frame length 4 exceeds configured limit of 3"
        );
        assert_eq!(
            decoder.push(&[1]).unwrap_err().message(),
            "Frame decoder has failed"
        );
    }

    #[test]
    fn accepts_a_frame_exactly_at_the_configured_maximum() {
        let mut decoder =
            FrameDecoder::new(FrameDecoderOptions::default().with_max_frame_length(3)).unwrap();
        assert_eq!(
            decoder.push(&encode_frame(&[1, 2, 3]).unwrap()).unwrap(),
            [vec![1, 2, 3]]
        );
        decoder.end().unwrap();
    }

    #[test]
    fn cannot_be_pushed_after_end() {
        let mut decoder = new_decoder();
        decoder.end().unwrap();
        assert_eq!(
            decoder.push(&[]).unwrap_err().message(),
            "Frame decoder has ended"
        );
        assert_eq!(
            decoder.end().unwrap_err().message(),
            "Frame decoder has ended"
        );
    }

    #[test]
    fn rejects_invalid_maximum_frame_length() {
        // D4: -1, 1.5 and NaN are unrepresentable as u64; the representable
        // overflow keeps the exact upstream message (oracle: 16 MiB * 1000).
        let error = FrameDecoder::new(
            FrameDecoderOptions::default().with_max_frame_length(DEFAULT_MAX_FRAME_LENGTH * 1000),
        )
        .unwrap_err();
        assert_eq!(
            error.message(),
            "maxFrameLength must be an integer between 0 and 4294967295"
        );
    }

    #[test]
    fn oracle_block_boundary_output() {
        // Node oracle: frame-70000-blocks, first 40 and last 10 payload bytes.
        let payload: Vec<u8> = (0..70_000).map(|index| (index % 251) as u8).collect();
        let wire = encode_frame(&payload).unwrap();
        let mut decoder = new_decoder();
        let frames = decoder.push(&wire).unwrap();
        decoder.end().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            hex(&frames[0][..40]),
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021222324252627"
        );
        assert_eq!(hex(&frames[0][69_990..]), "d4d5d6d7d8d9dadbdcdd");
    }
}
