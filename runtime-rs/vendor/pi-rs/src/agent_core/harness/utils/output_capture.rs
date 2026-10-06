//! Port of `packages/agent/src/harness/utils/output-capture.ts` (238 lines):
//! maintains and publishes one bounded shell-output view.
//!
//! Ported here (M3b Task 6): the execution environment's `exec` owns
//! source-side capture, adaptive publication, and spilling, so this module
//! lands with the env. `applyShellOutputUpdate` and `sanitizeShellOutput`
//! are the exported helpers the oracle tests consume.
//!
//! Disclosed substitutions:
//! - **Decoding.** Upstream feeds every chunk through a streaming
//!   `TextDecoder`; the port buffers raw bytes and decodes lossily at
//!   snapshot time (`String::from_utf8_lossy`), which replaces invalid
//!   sequences with U+FFFD exactly like the upstream non-fatal decoder. The
//!   only observable difference is where a replacement character lands for
//!   invalid bytes straddling chunk boundaries; valid UTF-8 output is
//!   byte-identical.
//! - **String units.** `updateFrom`/`applyShellOutputUpdate` compute
//!   `drop`/`slice` positions in UTF-16 code units like upstream (see the
//!   `ShellOutputUpdate::Slide` doc in harness types); the port encodes to
//!   `u16` slices for those two operations and decodes lossily on the way
//!   back. Truncation itself is line- and byte-based (see
//!   [`super::truncate`] module docs).
//! - **Callback failures.** Upstream wraps synchronous callback throws into
//!   `onError`; the port catches panics on the publish path
//!   ([`AdaptivePublisher`]) and feeds them to the same handler.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::adaptive_publisher::{AdaptivePublisher, AdaptivePublisherOptions};
use super::truncate::{
    truncate_head, truncate_tail, TruncationOptions, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::types::{
    ShellOutputCaptureOptions, ShellOutputLimits, ShellOutputMetadata, ShellOutputRetention,
    ShellOutputUpdate, ShellOutputView, ShellUpdateCallback, TruncatedBy,
};

/// Upstream `OUTPUT_MIN_EMIT_INTERVAL_MS` (`output-capture.ts:6`).
pub const OUTPUT_MIN_EMIT_INTERVAL_MS: u64 = 100;

/// Upstream `OUTPUT_TARGET_BYTES_PER_SECOND` (`output-capture.ts:7`).
pub const OUTPUT_TARGET_BYTES_PER_SECOND: u64 = 100 * 1024;

/// Handler bundle from upstream `OutputCaptureHandlers`
/// (`output-capture.ts:13-16`): `onUpdate` plus the required `onError`.
pub struct OutputCaptureHandlers {
    /// Called with bounded output changes (upstream sync callback).
    pub on_update: Option<Arc<ShellUpdateCallback>>,
    /// Receives callback failures (upstream thrown errors).
    pub on_error: Arc<dyn Fn(String) + Send + Sync>,
}

struct CaptureState {
    buffer: Vec<u8>,
    total_bytes: u64,
    newlines: u64,
    ends_with_newline: bool,
    current_line_bytes: u64,
    spill_path: Option<String>,
    disposed: bool,
}

/// Upstream `OutputCapture` (`output-capture.ts:26-151`): the bounded view
/// and its adaptive publisher. Shared by the exec reader tasks, so every
/// method takes `&self`.
pub struct OutputCapture {
    max_bytes: u64,
    max_lines: u64,
    retain_head: bool,
    state: Arc<Mutex<CaptureState>>,
    publisher: Arc<AdaptivePublisher<ShellOutputView, ShellOutputUpdate>>,
}

// Silence the unused-import lint for the alias import below.

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl std::fmt::Debug for OutputCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The publisher's closures are opaque; limits are the observable part.
        f.debug_struct("OutputCapture")
            .field("max_bytes", &self.max_bytes)
            .field("max_lines", &self.max_lines)
            .field("retain_head", &self.retain_head)
            .finish_non_exhaustive()
    }
}

impl OutputCapture {
    /// Upstream constructor (`output-capture.ts:44-65`). `options.limits`
    /// default to 50KB/2000 lines/`"tail"`; invalid limits return the
    /// upstream `TypeError` message (exec maps it to an `unknown`
    /// execution error).
    pub fn new(
        options: Option<&ShellOutputCaptureOptions>,
        context: Context,
        handlers: OutputCaptureHandlers,
    ) -> Result<Self, String> {
        let default_limits = ShellOutputLimits {
            max_bytes: DEFAULT_MAX_BYTES,
            max_lines: DEFAULT_MAX_LINES,
            retain: None,
        };
        let limits = options
            .map(|options| options.limits)
            .unwrap_or(default_limits);
        let max_bytes = limits.max_bytes;
        let max_lines = limits.max_lines;
        let retain_head = limits.retain == Some(ShellOutputRetention::Head);
        if max_bytes == 0 {
            return Err("Output maxBytes must be a positive finite number".to_string());
        }
        if max_lines == 0 {
            return Err("Output maxLines must be a positive integer".to_string());
        }
        let state = Arc::new(Mutex::new(CaptureState {
            buffer: Vec::new(),
            total_bytes: 0,
            newlines: 0,
            ends_with_newline: true,
            current_line_bytes: 0,
            spill_path: None,
            disposed: false,
        }));

        let snapshot_state = Arc::clone(&state);
        let update_context = context;
        let publish_context = update_context.clone();
        let publish_on_update = handlers.on_update.clone();
        let on_error = handlers.on_error;
        let publisher = Arc::new(AdaptivePublisher::new(AdaptivePublisherOptions {
            snapshot: Box::new(move || {
                let state = lock(&snapshot_state);
                snapshot_of(&state, max_bytes, max_lines, retain_head)
            }),
            update: Box::new(update_from),
            measure: Box::new(|update: &ShellOutputUpdate| {
                serde_json::to_string(update).unwrap_or_default().len()
            }),
            publish: Box::new(move |update: ShellOutputUpdate| {
                if let Some(on_update) = &publish_on_update {
                    on_update(update, &publish_context);
                }
            }),
            on_error: Box::new(move |message: String| on_error(message)),
            min_interval_ms: Some(OUTPUT_MIN_EMIT_INTERVAL_MS),
            target_bytes_per_second: Some(OUTPUT_TARGET_BYTES_PER_SECOND),
        }));

        Ok(OutputCapture {
            max_bytes,
            max_lines,
            retain_head,
            state,
            publisher,
        })
    }

    /// Upstream `truncated` (`output-capture.ts:67-69`).
    pub fn truncated(&self) -> bool {
        let state = lock(&self.state);
        state.total_bytes > self.max_bytes || total_lines_of(&state) > self.max_lines
    }

    /// Upstream `push` (`output-capture.ts:71-79`).
    pub fn push(&self, chunk: &[u8]) {
        self.append(chunk);
    }

    /// Upstream `finish` (`output-capture.ts:81-84`): flush the decoder. The
    /// port decodes at snapshot time, so this only re-marks the view dirty.
    pub fn finish(&self) {
        self.append(&[]);
    }

    /// Upstream `setSpillPath` (`output-capture.ts:86-91`).
    pub fn set_spill_path(&self, path: &str) {
        {
            let mut state = lock(&self.state);
            if state.disposed || state.spill_path.as_deref() == Some(path) {
                return;
            }
            state.spill_path = Some(path.to_string());
        }
        self.publisher.mark_dirty();
        self.flush();
    }

    /// Upstream `snapshot` (`output-capture.ts:93-113`).
    pub fn snapshot(&self) -> ShellOutputView {
        let state = lock(&self.state);
        snapshot_of(&state, self.max_bytes, self.max_lines, self.retain_head)
    }

    /// Upstream `flush` (`output-capture.ts:115-118`): forced publication.
    pub fn flush(&self) {
        self.publisher.flush(true);
    }

    /// Upstream `dispose` (`output-capture.ts:120-123`).
    pub fn dispose(&self) {
        lock(&self.state).disposed = true;
        self.publisher.dispose();
    }

    fn append(&self, chunk: &[u8]) {
        let text_bytes = chunk.len() as u64;
        {
            let mut state = lock(&self.state);
            if state.disposed {
                return;
            }
            if chunk.is_empty() {
                return;
            }
            state.total_bytes += text_bytes;
            state.newlines += chunk.iter().filter(|byte| **byte == b'\n').count() as u64;
            state.ends_with_newline = chunk.last() == Some(&b'\n');
            let last_newline = chunk.iter().rposition(|byte| *byte == b'\n');
            state.current_line_bytes = match last_newline {
                Some(position) => (chunk.len() - position - 1) as u64,
                None => state.current_line_bytes + text_bytes,
            };
            state.buffer.extend_from_slice(chunk);

            // Buffer guard (output-capture.ts:137-144): cap at twice the byte
            // limit (the upstream `guard * 2` check), keeping the head or tail
            // half depending on retain.
            let guard = self.max_bytes * 2;
            if state.buffer.len() as u64 > guard * 2 {
                let trimmed = if self.retain_head {
                    trim_to_first_utf8_bytes(&state.buffer, guard as usize)
                } else {
                    trim_to_last_utf8_bytes(&state.buffer, guard as usize)
                };
                state.buffer = trimmed;
            }
        }
        self.publisher.mark_dirty();
    }
}

fn total_lines_of(state: &CaptureState) -> u64 {
    state.newlines + u64::from(!state.ends_with_newline && state.total_bytes != 0)
}

fn snapshot_of(
    state: &CaptureState,
    max_bytes: u64,
    max_lines: u64,
    retain_head: bool,
) -> ShellOutputView {
    let buffer = String::from_utf8_lossy(&state.buffer).into_owned();
    let options = TruncationOptions {
        max_lines: Some(max_lines),
        max_bytes: Some(max_bytes),
    };
    let retained = if retain_head {
        truncate_head(&buffer, options)
    } else {
        truncate_tail(&buffer, options)
    };
    let total_lines = total_lines_of(state);
    let truncated = state.total_bytes > max_bytes || total_lines > max_lines;
    let mut metadata = ShellOutputMetadata {
        truncation: retained.truncation_metadata(),
        spill_path: state.spill_path.clone(),
        last_line_bytes: None,
    };
    // Snapshot totals override the buffer-relative counts the truncation
    // helpers computed (upstream spreads `totalBytes`/`totalLines`/`truncated`
    // /`truncatedBy` over the retained result, output-capture.ts:103-109).
    metadata.truncation.total_bytes = state.total_bytes;
    metadata.truncation.total_lines = total_lines;
    metadata.truncation.truncated = truncated;
    metadata.truncation.truncated_by = if truncated {
        Some(if total_lines > max_lines {
            TruncatedBy::Lines
        } else {
            TruncatedBy::Bytes
        })
    } else {
        None
    };
    if retained.last_line_partial {
        metadata.last_line_bytes = Some(state.current_line_bytes);
    }
    ShellOutputView {
        metadata,
        text: sanitize_shell_output(&retained.content),
    }
}

/// Upstream `applyShellOutputUpdate` (`output-capture.ts:153-167`): fold one
/// incremental update into the current view.
pub fn apply_shell_output_update(
    current: Option<ShellOutputView>,
    update: ShellOutputUpdate,
) -> ShellOutputView {
    match update {
        ShellOutputUpdate::Replace { output } => output,
        ShellOutputUpdate::Append { text, metadata } => {
            let previous = current
                .as_ref()
                .map(|view| view.text.as_str())
                .unwrap_or("");
            ShellOutputView {
                metadata,
                text: format!("{previous}{text}"),
            }
        }
        ShellOutputUpdate::Slide {
            drop,
            text,
            metadata,
        } => {
            let previous = current
                .as_ref()
                .map(|view| view.text.as_str())
                .unwrap_or("");
            // Upstream `text.slice(update.drop)` operates on UTF-16 units.
            let units: Vec<u16> = previous.encode_utf16().collect();
            let start = (drop as usize).min(units.len());
            let kept = String::from_utf16_lossy(&units[start..]);
            ShellOutputView {
                metadata,
                text: format!("{kept}{text}"),
            }
        }
        ShellOutputUpdate::Metadata { metadata } => {
            let text = current.map(|view| view.text).unwrap_or_default();
            ShellOutputView { metadata, text }
        }
    }
}

/// The `updateFrom` diff (`output-capture.ts:169-194`): classify the current
/// view against the previously published one.
fn update_from(
    previous: Option<&ShellOutputView>,
    current: &ShellOutputView,
) -> Option<ShellOutputUpdate> {
    let Some(previous) = previous else {
        return Some(ShellOutputUpdate::Replace {
            output: current.clone(),
        });
    };
    let metadata = ShellOutputMetadata {
        truncation: current.metadata.truncation,
        spill_path: current.metadata.spill_path.clone(),
        last_line_bytes: current.metadata.last_line_bytes,
    };
    if current.text == previous.text {
        return Some(ShellOutputUpdate::Metadata { metadata });
    }
    // Prefix extension check in UTF-16 units (output-capture.ts:177-179).
    let previous_units: Vec<u16> = previous.text.encode_utf16().collect();
    let current_units: Vec<u16> = current.text.encode_utf16().collect();
    if current_units.len() > previous_units.len()
        && current_units[..previous_units.len()] == previous_units[..]
    {
        return Some(ShellOutputUpdate::Append {
            text: String::from_utf16_lossy(&current_units[previous_units.len()..]),
            metadata,
        });
    }
    let scan = previous_units
        .len()
        .min(current_units.len())
        .min(current.metadata.truncation.max_bytes.saturating_mul(2) as usize);
    let shared = suffix_prefix_overlap(&previous_units, &current_units, scan);
    if shared > 0 {
        return Some(ShellOutputUpdate::Slide {
            drop: (previous_units.len() - shared) as u64,
            text: String::from_utf16_lossy(&current_units[shared..]),
            metadata,
        });
    }
    Some(ShellOutputUpdate::Replace {
        output: current.clone(),
    })
}

/// Upstream `suffixPrefixOverlap` (`output-capture.ts:196-212`): the longest
/// suffix of `before` that is a prefix of `after`, scanned over at most
/// `scan` trailing units with the two-probe search.
fn suffix_prefix_overlap(before: &[u16], after: &[u16], scan: usize) -> usize {
    if before.is_empty() || after.is_empty() || scan == 0 {
        return 0;
    }
    let tail: &[u16] = if before.len() > scan {
        &before[before.len() - scan..]
    } else {
        before
    };
    for probe_length in [after.len().min(64), 1] {
        let probe = &after[..probe_length];
        let mut candidates = 0;
        let mut index = 0;
        while let Some(found) = find_subslice(tail, probe, index) {
            index = found + 1;
            candidates += 1;
            if candidates > 8 {
                break;
            }
            let overlap_length = tail.len() - found;
            if overlap_length <= after.len() && tail[found..] == after[..overlap_length] {
                return overlap_length;
            }
        }
        if probe_length == 1 {
            break;
        }
    }
    0
}

/// `indexOf` over `u16` slices starting at `from`.
fn find_subslice(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || from > haystack.len() - needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len())
        .find(|index| &haystack[*index..index + needle.len()] == needle)
}

/// Upstream `sanitizeShellOutput` (`output-capture.ts:214-216`): strip
/// control characters except tab and newline, plus the U+FFF9-FFFB inline
/// formatting range. `\r` (0x0D) is inside the stripped range upstream.
pub fn sanitize_shell_output(text: &str) -> String {
    text.chars()
        .filter(|character| {
            !matches!(
                character,
                '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{fff9}'..='\u{fffb}'
            )
        })
        .collect()
}

/// Upstream `trimToLastUtf8Bytes` (`output-capture.ts:224-230`).
fn trim_to_last_utf8_bytes(buffer: &[u8], max_bytes: usize) -> Vec<u8> {
    if buffer.len() <= max_bytes {
        return buffer.to_vec();
    }
    let mut start = buffer.len() - max_bytes;
    while start < buffer.len() && (buffer[start] & 0xc0) == 0x80 {
        start += 1;
    }
    buffer[start..].to_vec()
}

/// Upstream `trimToFirstUtf8Bytes` (`output-capture.ts:232-238`).
fn trim_to_first_utf8_bytes(buffer: &[u8], max_bytes: usize) -> Vec<u8> {
    if buffer.len() <= max_bytes {
        return buffer.to_vec();
    }
    let mut end = max_bytes;
    while end > 0 && (buffer[end] & 0xc0) == 0x80 {
        end -= 1;
    }
    buffer[..end].to_vec()
}

#[cfg(test)]
mod tests;
