//! Port of `src/harness/output.ts`: tool-output sanitizing, bounding, the
//! chunked [`OutputBuffer`], and the adaptive [`Progress`] commit scheduler.
//!
//! Divergences (structural, disclosed): the upstream `Progress` throttles
//! async commits through promise resolvers and `setTimeout`; the port's
//! harness runtimes schedule the same minimum intervals
//! ([`MIN_PROGRESS_INTERVAL_MS`], [`PROGRESS_BYTES_PER_SECOND`]) through
//! [`tokio::time`] and a tokio task, so timing behavior is unchanged and the
//! resolvers become waiters. `PromiseWithResolvers` maps to a oneshot pair.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::super::truncate::utf8_byte_length;

/// Retention limits of one tool's output (`output.ts` `OutputLimits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub retain: super::types::OutputRetain,
}

/// Retained output and what the limits dropped (`output.ts` `BoundedOutput`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    pub dropped_bytes: usize,
    pub dropped_lines: usize,
}

/// An exact slice of the input within the limits, and what it left out
/// (`output.ts` `OutputSlice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSlice {
    pub text: String,
    pub bytes: usize,
    pub dropped_bytes: usize,
    pub dropped_lines: usize,
}

const NEWLINE: u8 = 0x0a;

/// Remove control characters that break display and transcripts (`output.ts`
/// `sanitizeOutput`); tabs and newlines stay. The upstream class covers
/// C0 controls other than tab/LF plus the U+FFF9–U+FFFB inline-notation
/// range.
pub fn sanitize_output(text: &str) -> String {
    text.chars()
        .filter(
            |character| !matches!(*character as u32, 0x00..=0x08 | 0x0b..=0x1f | 0xfff9..=0xfffb),
        )
        .collect()
}

/// Bound `text` to whole lines within the limits (`output.ts` `boundOutput`):
/// the first lines for `head`, the last lines for `tail`. The result is an
/// exact slice, trailing newline included. A single line longer than
/// `maxBytes` is cut at the byte limit on a character boundary.
pub fn bound_output(text: &str, limits: &OutputLimits) -> OutputSlice {
    let bytes = text.as_bytes();
    let (from, to) = match limits.retain {
        super::types::OutputRetain::Head => head_range(bytes, limits),
        super::types::OutputRetain::Tail => tail_range(bytes, limits),
    };
    let kept = &bytes[from..to];
    let text = if kept.len() == bytes.len() {
        text.to_string()
    } else {
        String::from_utf8_lossy(kept).into_owned()
    };
    OutputSlice {
        bytes: kept.len(),
        dropped_bytes: bytes.len() - kept.len(),
        dropped_lines: line_count(bytes) - line_count(kept),
        text,
    }
    .into_ordered()
}

impl OutputSlice {
    /// Reorder into the declared field order (text, bytes, droppedBytes,
    /// droppedLines) — construction-order fidelity, not semantics.
    fn into_ordered(self) -> OutputSlice {
        self
    }
}

fn head_range(bytes: &[u8], limits: &OutputLimits) -> (usize, usize) {
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (0, 0);
    }
    let mut end = bytes.len();
    let mut lines = 0;
    let mut index = match bytes.iter().position(|byte| *byte == NEWLINE) {
        Some(index) => index,
        None => usize::MAX,
    };
    while index != usize::MAX {
        lines += 1;
        if lines == limits.max_lines {
            end = index + 1;
            break;
        }
        index = match bytes[index + 1..].iter().position(|byte| *byte == NEWLINE) {
            Some(offset) => index + 1 + offset,
            None => usize::MAX,
        };
    }
    if end > limits.max_bytes {
        let newline = bytes[..limits.max_bytes.min(bytes.len())]
            .iter()
            .rposition(|byte| *byte == NEWLINE);
        end = match newline {
            Some(newline) => newline + 1,
            None => character_end(bytes, limits.max_bytes),
        };
    }
    (0, end)
}

fn tail_range(bytes: &[u8], limits: &OutputLimits) -> (usize, usize) {
    if limits.max_lines == 0 || limits.max_bytes == 0 {
        return (bytes.len(), bytes.len());
    }
    // A trailing newline ends the last line rather than starting another.
    let last = if bytes.last() == Some(&NEWLINE) {
        bytes.len() as isize - 2
    } else {
        bytes.len() as isize - 1
    };
    let mut start = 0usize;
    let mut lines = 1usize;
    let mut index: isize = if last < 0 {
        -1
    } else {
        bytes[..=last as usize]
            .iter()
            .rposition(|byte| *byte == NEWLINE)
            .map(|position| position as isize)
            .unwrap_or(-1)
    };
    while index != -1 {
        if lines == limits.max_lines {
            start = index as usize + 1;
            break;
        }
        lines += 1;
        index = if index == 0 {
            -1
        } else {
            bytes[..index as usize]
                .iter()
                .rposition(|byte| *byte == NEWLINE)
                .map(|position| position as isize)
                .unwrap_or(-1)
        };
    }
    if bytes.len() - start > limits.max_bytes {
        let from = bytes.len() - limits.max_bytes;
        let newline = if from >= 1 {
            bytes[from - 1..]
                .iter()
                .position(|byte| *byte == NEWLINE)
                .map(|position| position + from - 1)
        } else {
            bytes.iter().position(|byte| *byte == NEWLINE)
        };
        // The first line starting inside the byte window, or a cut of the
        // last line when it alone is too long.
        start = match newline {
            Some(newline) if newline + 1 < bytes.len() => newline + 1,
            _ => character_start(bytes, from),
        };
    }
    (start, bytes.len())
}

/// The last character boundary at or before `index` (`output.ts`
/// `characterEnd`).
pub fn character_end(bytes: &[u8], index: usize) -> usize {
    let mut end = index;
    while end > 0 && (bytes[end] & 0xc0) == 0x80 {
        end -= 1;
    }
    end
}

/// The first character boundary at or after `index` (`output.ts`
/// `characterStart`).
fn character_start(bytes: &[u8], index: usize) -> usize {
    let mut start = index;
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    start
}

fn line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|byte| **byte == NEWLINE).count();
    newlines + usize::from(bytes[bytes.len() - 1] != NEWLINE)
}

/// One stored chunk of an [`OutputBuffer`].
#[derive(Debug, Clone)]
struct Chunk {
    text: String,
    bytes: usize,
    newlines: usize,
}

/// Bounded running output of one tool call (`output.ts` `OutputBuffer`).
/// Accepting a chunk costs time proportional to the chunk: head retention
/// stops storing once the window is full, and tail retention drops stored
/// text the window no longer needs when it snapshots. Counts of the whole
/// stream are kept so the dropped totals stay exact.
pub struct OutputBuffer {
    limits: OutputLimits,
    decoder: Mutex<StreamDecoder>,
    /// Stored chunks: for head the start of the stream, for tail a suffix
    /// that still contains the next window.
    chunks: Mutex<VecDeque<Chunk>>,
    state: Mutex<BufferState>,
}

#[derive(Default)]
struct BufferState {
    stored_bytes: usize,
    stored_newlines: usize,
    full: bool,
    total_bytes: usize,
    total_newlines: usize,
    ends_with_newline: bool,
}

/// Incremental UTF-8 decoder: chunks may split characters; the incomplete
/// trailing sequence stays buffered until the flush.
#[derive(Default)]
struct StreamDecoder {
    pending: Vec<u8>,
}

impl StreamDecoder {
    fn decode(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let text = match std::str::from_utf8(&self.pending) {
            Ok(_) => {
                let text = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                text
            }
            Err(error) => {
                let valid = error.valid_up_to();
                let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                self.pending.drain(..valid);
                text
            }
        };
        text
    }

    /// Final flush (`decoder.decode()`): the incomplete tail becomes U+FFFD.
    fn finish(&mut self) -> String {
        let tail = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        tail
    }
}

impl OutputBuffer {
    pub fn new(limits: OutputLimits) -> Self {
        OutputBuffer {
            limits,
            decoder: Mutex::new(StreamDecoder::default()),
            chunks: Mutex::new(VecDeque::new()),
            state: Mutex::new(BufferState::default()),
        }
    }

    /// Bytes currently held; bounded by the limits plus one chunk
    /// (`storedBytes`).
    pub fn stored_bytes(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stored_bytes
    }

    /// Accept a chunk (`push`); returns whether anything was accepted.
    pub fn push(&self, chunk: &[u8]) -> bool {
        let text = self
            .decoder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decode(chunk);
        self.accept(&text)
    }

    /// Accept text directly (`push` of a string chunk).
    pub fn push_text(&self, text: &str) -> bool {
        // Bytes of an incomplete character from an earlier byte chunk come
        // first (`this.#decoder.decode() + chunk`).
        let flush = self
            .decoder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish();
        self.accept(&format!("{flush}{text}"))
    }

    /// Flush an incomplete trailing character as a replacement character
    /// (`end`); call when the stream ends.
    pub fn end(&self) {
        let tail = self
            .decoder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish();
        self.accept(&tail);
    }

    fn accept(&self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let bytes = utf8_byte_length(text);
        let newlines = count_newlines(text);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.total_bytes += bytes;
        state.total_newlines += newlines;
        state.ends_with_newline = text.ends_with('\n');
        if state.full {
            return true;
        }
        self.chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(Chunk {
                text: text.to_string(),
                bytes,
                newlines,
            });
        state.stored_bytes += bytes;
        state.stored_newlines += newlines;
        if self.limits.retain == super::types::OutputRetain::Head {
            // Nothing past a full window is ever needed.
            state.full = state.stored_bytes > self.limits.max_bytes
                || state.stored_newlines >= self.limits.max_lines;
            return true;
        }
        // Drop leading chunks while the rest still holds more than a window:
        // more than `maxBytes` bytes or `maxLines` newlines, plus one, so the
        // window's line start can still be found. Each chunk is dropped once.
        let mut chunks = self
            .chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while chunks.len() > 1 {
            let first_bytes = chunks.front().expect("non-empty").bytes;
            let first_newlines = chunks.front().expect("non-empty").newlines;
            let bytes_after = state.stored_bytes - first_bytes;
            let newlines_after = state.stored_newlines - first_newlines;
            if bytes_after <= self.limits.max_bytes + 1
                && newlines_after <= self.limits.max_lines + 1
            {
                break;
            }
            chunks.pop_front();
            state.stored_bytes = bytes_after;
            state.stored_newlines = newlines_after;
        }
        true
    }

    /// Retained, sanitized output and what the limits dropped from the whole
    /// stream (`snapshot`).
    pub fn snapshot(&self) -> BoundedOutput {
        let mut chunks = self
            .chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stored: String = if chunks.len() == 1 {
            chunks.front().expect("one chunk").text.clone()
        } else {
            chunks.iter().map(|chunk| chunk.text.as_str()).collect()
        };
        let kept = bound_output(&stored, &self.limits);
        let stored_lines = lines(
            state.stored_newlines,
            stored.is_empty() || stored.ends_with('\n'),
        );
        let kept_lines = stored_lines - kept.dropped_lines;
        // Tail windows never reach back before this one, so only the kept
        // slice needs storing.
        if self.limits.retain == super::types::OutputRetain::Tail || chunks.len() > 1 {
            let text = if self.limits.retain == super::types::OutputRetain::Tail {
                kept.text.clone()
            } else {
                stored.clone()
            };
            let bytes = if self.limits.retain == super::types::OutputRetain::Tail {
                kept.bytes
            } else {
                state.stored_bytes
            };
            chunks.clear();
            if !text.is_empty() {
                let newlines = count_newlines(&text);
                chunks.push_back(Chunk {
                    text,
                    bytes,
                    newlines,
                });
            }
            state.stored_bytes = bytes;
            state.stored_newlines = chunks.front().map(|chunk| chunk.newlines).unwrap_or(0);
        }
        BoundedOutput {
            text: sanitize_output(&kept.text),
            dropped_bytes: state.total_bytes - kept.bytes,
            dropped_lines: lines(state.total_newlines, state.ends_with_newline) - kept_lines,
        }
    }
}

/// Lines of text with `newlines` newlines; a final unterminated line counts
/// (`output.ts` `lines`).
fn lines(newlines: usize, terminated: bool) -> usize {
    newlines + usize::from(!terminated)
}

fn count_newlines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == NEWLINE).count()
}

/// Minimum pause between progress commits (`output.ts`); each commit also
/// buys a pause proportional to what it wrote.
pub const MIN_PROGRESS_INTERVAL_MS: u64 = 100;
pub const PROGRESS_BYTES_PER_SECOND: usize = 100 * 1024;

/// Adaptive progress commits (`output.ts` `Progress`), like the environment's
/// shell output capture: the first change after an idle period commits at
/// once; each commit then delays the next by at least 100 ms and by its
/// written size at 100 KiB/s. At most one commit is in flight; changes made
/// meanwhile coalesce into the next one.
pub struct Progress {
    inner: Mutex<ProgressState>,
    /// Wake the scheduler loop.
    wake: mpsc::UnboundedSender<()>,
    stop_token: CancellationToken,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct ProgressState {
    write: Box<dyn Fn() -> futures::future::BoxFuture<'static, usize> + Send>,
    on_error: Box<dyn Fn(&str) + Send + Sync>,
    waiters: Vec<oneshot::Sender<Result<(), String>>>,
    next_at: Option<tokio::time::Instant>,
    dirty: bool,
    stopped: bool,
}

impl Progress {
    /// `write` returns the byte count the commit wrote; `on_error` receives
    /// write failures that do not fail the caller.
    pub fn new(
        write: Box<dyn Fn() -> futures::future::BoxFuture<'static, usize> + Send>,
        on_error: Box<dyn Fn(&str) + Send + Sync>,
    ) -> Arc<Self> {
        let (wake, wake_rx) = mpsc::unbounded_channel();
        let stop_token = CancellationToken::new();
        let progress = Arc::new(Progress {
            inner: Mutex::new(ProgressState {
                write,
                on_error,
                waiters: Vec::new(),
                next_at: None,
                dirty: false,
                stopped: false,
            }),
            wake,
            stop_token: stop_token.clone(),
            handle: Mutex::new(None),
        });
        let scheduler = Arc::clone(&progress);
        let handle = tokio::spawn(async move {
            scheduler.run(wake_rx).await;
        });
        *progress
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
        progress
    }

    /// Schedule a commit (`mark`).
    pub fn mark(&self) {
        {
            let mut state = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.dirty = true;
        }
        let _ = self.wake.send(());
    }

    /// Schedule a commit; the returned future settles with the commit that
    /// includes this change (`markAndWait`).
    pub fn mark_and_wait(&self) -> oneshot::Receiver<Result<(), String>> {
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.waiters.push(tx);
        }
        self.mark();
        rx
    }

    /// Stop committing and wait for the commit in flight (`stop`); returns
    /// the waiters the final commit must settle.
    pub async fn stop(&self) -> Vec<oneshot::Sender<Result<(), String>>> {
        {
            let mut state = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.stopped = true;
        }
        self.stop_token.cancel();
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        std::mem::take(
            &mut self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .waiters,
        )
    }

    async fn run(&self, mut wake: mpsc::UnboundedReceiver<()>) {
        loop {
            let (dirty, next_at, stopped) = {
                let state = self
                    .inner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (state.dirty, state.next_at, state.stopped)
            };
            if stopped {
                return;
            }
            let delay = match (dirty, next_at) {
                (true, Some(next_at)) => {
                    let now = tokio::time::Instant::now();
                    next_at.saturating_duration_since(now)
                }
                (true, None) => Duration::ZERO,
                (false, _) => {
                    // Park until the next mark or shutdown.
                    tokio::select! {
                        _ = wake.recv() => continue,
                        _ = self.stop_token.cancelled() => return,
                    }
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = wake.recv() => continue,
                _ = self.stop_token.cancelled() => return,
            }
            self.flush().await;
        }
    }

    async fn flush(&self) {
        let (_waiters, started) = {
            let mut state = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stopped || !state.dirty {
                return;
            }
            state.dirty = false;
            (
                std::mem::take(&mut state.waiters),
                tokio::time::Instant::now(),
            )
        };
        let write = {
            let state = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (state.write)()
        };
        let bytes = write.await;
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.next_at = Some(
            started
                + Duration::from_millis(MIN_PROGRESS_INTERVAL_MS).max(
                    Duration::from_secs_f64(
                        bytes as f64 * 1000.0 / PROGRESS_BYTES_PER_SECOND as f64,
                    )
                    .min(Duration::from_secs(3600)),
                ),
        );
        let failure = if bytes == usize::MAX {
            Some("progress write failed")
        } else {
            None
        };
        if let Some(message) = failure {
            (state.on_error)(message);
        }
        for waiter in state.waiters.drain(..) {
            let _ = waiter.send(match failure {
                Some(message) => Err(message.to_string()),
                None => Ok(()),
            });
        }
        if state.dirty {
            drop(state);
            let _ = self.wake.send(());
        }
    }
}
