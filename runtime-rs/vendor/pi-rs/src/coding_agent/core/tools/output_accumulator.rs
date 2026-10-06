//! Bounded streaming output and lossless spill files from upstream
//! `core/tools/output-accumulator.ts`. Decoding uses WHATWG TextDecoder
//! semantics (including a single leading BOM), not Node StringDecoder.
//! Temp IO is synchronous and fallible; callers must propagate write errors.
use super::truncate::{
    self, truncate_tail, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use crate::agent_core::harness::types::TruncatedBy;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default)]
pub struct OutputAccumulatorOptions {
    pub max_lines: Option<u64>,
    pub max_bytes: Option<u64>,
    pub temp_file_prefix: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub content: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<PathBuf>,
}
impl OutputSnapshot {
    pub fn as_value(&self) -> Value {
        let mut value =
            json!({"content":self.content,"truncation":truncate::as_value(&self.truncation)});
        if let Some(path) = &self.full_output_path {
            value["fullOutputPath"] = json!(path);
        }
        value
    }
}

#[derive(Default)]
pub(crate) struct TextDecoder {
    pending: Vec<u8>,
    started: bool,
}
impl TextDecoder {
    pub(crate) fn decode(&mut self, data: &[u8], finish: bool) -> String {
        // At most three bytes are retained between calls. Invalid prefixes are
        // replaced immediately, unlike Node's StringDecoder in rpc/jsonl.rs.
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(data);
        let mut text = String::new();
        let mut remaining = bytes.as_slice();
        loop {
            match std::str::from_utf8(remaining) {
                Ok(valid) => {
                    text.push_str(valid);
                    break;
                }
                Err(error) => {
                    let (valid, suffix) = remaining.split_at(error.valid_up_to());
                    text.push_str(std::str::from_utf8(valid).expect("validated UTF-8 prefix"));
                    if let Some(length) = error.error_len() {
                        text.push('\u{fffd}');
                        remaining = &suffix[length..];
                    } else {
                        if finish {
                            text.push('\u{fffd}');
                        } else {
                            self.pending.extend_from_slice(suffix);
                        }
                        break;
                    }
                }
            }
        }
        if !self.started && !text.is_empty() {
            self.started = true;
            if text.starts_with('\u{feff}') {
                text.drain(..'\u{feff}'.len_utf8());
            }
        }
        text
    }
}

pub struct OutputAccumulator {
    max_lines: u64,
    max_bytes: u64,
    max_rolling_bytes: u64,
    temp_file_prefix: String,
    temp_directory: PathBuf,
    decoder: TextDecoder,
    raw_chunks: Vec<Vec<u8>>,
    tail_text: String,
    tail_bytes: u64,
    tail_starts_at_line_boundary: bool,
    total_raw_bytes: u64,
    total_decoded_bytes: u64,
    completed_lines: u64,
    total_lines: u64,
    current_line_bytes: u64,
    has_open_line: bool,
    finished: bool,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<File>,
}
impl Default for OutputAccumulator {
    fn default() -> Self {
        Self::new(Default::default())
    }
}
impl OutputAccumulator {
    pub fn new(options: OutputAccumulatorOptions) -> Self {
        Self::with_temp_directory(options, std::env::temp_dir())
    }
    /// Explicit temp root for embedding hosts and isolated offline tests.
    /// Spill files survive this accumulator; callers own their retention.
    pub fn with_temp_directory(
        options: OutputAccumulatorOptions,
        directory: impl AsRef<Path>,
    ) -> Self {
        let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        Self {
            max_lines: options.max_lines.unwrap_or(DEFAULT_MAX_LINES),
            max_bytes,
            max_rolling_bytes: max_bytes.saturating_mul(2).max(1),
            temp_file_prefix: options
                .temp_file_prefix
                .unwrap_or_else(|| "pi-output".into()),
            temp_directory: directory.as_ref().to_owned(),
            decoder: TextDecoder::default(),
            raw_chunks: vec![],
            tail_text: String::new(),
            tail_bytes: 0,
            tail_starts_at_line_boundary: true,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            completed_lines: 0,
            total_lines: 0,
            current_line_bytes: 0,
            has_open_line: false,
            finished: false,
            temp_file_path: None,
            temp_file: None,
        }
    }
    pub fn append(&mut self, data: &[u8]) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::other(
                "Cannot append to a finished output accumulator",
            ));
        }
        self.total_raw_bytes += data.len() as u64;
        let decoded = self.decoder.decode(data, false);
        self.append_decoded_text(&decoded);
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file()?;
            if let Some(file) = &mut self.temp_file {
                file.write_all(data)?;
            }
        } else if !data.is_empty() {
            self.raw_chunks.push(data.to_vec());
        }
        Ok(())
    }
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let decoded = self.decoder.decode(&[], true);
        self.append_decoded_text(&decoded);
        if self.should_use_temp_file() {
            self.ensure_temp_file()?;
        }
        Ok(())
    }
    pub fn snapshot(&mut self, persist_if_truncated: bool) -> io::Result<OutputSnapshot> {
        let text = if self.tail_starts_at_line_boundary {
            self.tail_text.as_str()
        } else {
            self.tail_text
                .find('\n')
                .map(|index| &self.tail_text[index + 1..])
                .unwrap_or(&self.tail_text)
        };
        let mut truncation = truncate_tail(
            text,
            TruncationOptions {
                max_lines: Some(self.max_lines),
                max_bytes: Some(self.max_bytes),
            },
        );
        let truncated =
            self.total_lines > self.max_lines || self.total_decoded_bytes > self.max_bytes;
        truncation.truncated = truncated;
        truncation.truncated_by = if truncated {
            truncation
                .truncated_by
                .or(Some(if self.total_decoded_bytes > self.max_bytes {
                    TruncatedBy::Bytes
                } else {
                    TruncatedBy::Lines
                }))
        } else {
            None
        };
        truncation.total_lines = self.total_lines;
        truncation.total_bytes = self.total_decoded_bytes;
        if persist_if_truncated && truncated {
            self.ensure_temp_file()?;
        }
        Ok(OutputSnapshot {
            content: truncation.content.clone(),
            truncation,
            full_output_path: self.temp_file_path.clone(),
        })
    }
    pub fn close_temp_file(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.temp_file.take() {
            file.flush()?;
        }
        Ok(())
    }
    pub fn get_last_line_bytes(&self) -> u64 {
        self.current_line_bytes
    }

    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.len() as u64;
        self.total_decoded_bytes += bytes;
        self.tail_text.push_str(text);
        self.tail_bytes += bytes;
        if self.tail_bytes > self.max_rolling_bytes.saturating_mul(2) {
            self.trim_tail();
        }
        if let Some(last) = text.rfind('\n') {
            self.completed_lines += text.bytes().filter(|&c| c == b'\n').count() as u64;
            let tail = &text[last + 1..];
            self.current_line_bytes = tail.len() as u64;
            self.has_open_line = !tail.is_empty();
        } else {
            self.current_line_bytes += bytes;
            self.has_open_line = true;
        }
        self.total_lines = self.completed_lines + u64::from(self.has_open_line);
    }
    fn trim_tail(&mut self) {
        if self.tail_text.len() as u64 <= self.max_rolling_bytes {
            return;
        }
        let mut start = self.tail_text.len() - self.max_rolling_bytes as usize;
        while !self.tail_text.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            self.tail_starts_at_line_boundary = self.tail_text.as_bytes()[start - 1] == b'\n';
        }
        // Replace rather than drain to release the large backing allocation.
        self.tail_text = self.tail_text[start..].to_owned();
        self.tail_bytes = self.tail_text.len() as u64;
    }
    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines > self.max_lines
    }
    fn ensure_temp_file(&mut self) -> io::Result<()> {
        if self.temp_file_path.is_some() {
            return Ok(());
        }
        let random: [u8; 8] = rand::random();
        let id = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = self
            .temp_directory
            .join(format!("{}-{id}.log", self.temp_file_prefix));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        for chunk in &self.raw_chunks {
            file.write_all(chunk)?;
        }
        self.raw_chunks.clear();
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
        Ok(())
    }
}

#[cfg(test)]
#[path = "output_accumulator_tests.rs"]
mod tests;
