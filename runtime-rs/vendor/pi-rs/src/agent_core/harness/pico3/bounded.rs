//! Port of `packages/agent/src/harness/pico3/bounded.ts` (100 lines): the
//! bounded byte collector. It retains a prefix or suffix constrained by both
//! a byte budget and a newline budget, and accounts for every discarded
//! byte/newline.

/// Upstream `Bounded` (`bounded.ts:6-100`).
pub struct Bounded {
    bytes: Vec<u8>,
    max_bytes: usize,
    max_lines: usize,
    retain: Retain,
    /// Upstream `droppedBytes` (`bounded.ts:11`).
    pub dropped_bytes: usize,
    /// Upstream `droppedLines` (`bounded.ts:12`).
    pub dropped_lines: usize,
    /// Upstream `total` (`bounded.ts:13`).
    pub total: usize,
}

/// Upstream `retain: "head" | "tail"` (`bounded.ts:9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retain {
    Head,
    Tail,
}

impl Bounded {
    /// Upstream `constructor` (`bounded.ts:14-18`); negative budgets upstream
    /// clamp to zero, and Rust `usize` constructors reject them at the call
    /// site instead.
    pub fn new(max_bytes: usize, max_lines: usize, retain: Retain) -> Bounded {
        Bounded {
            bytes: Vec::new(),
            max_bytes,
            max_lines,
            retain,
            dropped_bytes: 0,
            dropped_lines: 0,
            total: 0,
        }
    }

    /// Upstream `push` (`bounded.ts:20-32`).
    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len();
        if chunk.is_empty() {
            return;
        }
        if self.max_bytes == 0 || self.max_lines == 0 {
            self.drop(chunk);
            return;
        }
        match self.retain {
            Retain::Head => self.push_head(chunk),
            Retain::Tail => self.push_tail(chunk),
        }
    }

    /// Upstream `pushHead` (`bounded.ts:34-53`).
    fn push_head(&mut self, chunk: &[u8]) {
        let remaining_bytes = self.max_bytes.saturating_sub(self.bytes.len());
        let remaining_lines = self.max_lines.saturating_sub(count_newlines(&self.bytes));
        if remaining_bytes == 0 || remaining_lines == 0 {
            self.drop(chunk);
            return;
        }
        let mut take = chunk.len().min(remaining_bytes);
        let mut lines = 0;
        for (index, byte) in chunk.iter().take(take).enumerate() {
            if *byte != 0x0a {
                continue;
            }
            lines += 1;
            if lines == remaining_lines {
                take = index + 1;
                break;
            }
        }
        self.bytes.extend_from_slice(&chunk[..take]);
        self.drop(&chunk[take..]);
    }

    /// Upstream `pushTail` (`bounded.ts:55-62`).
    fn push_tail(&mut self, chunk: &[u8]) {
        let incoming_start = tail_start(chunk, self.max_bytes, self.max_lines);
        self.drop(&chunk[..incoming_start]);
        let mut combined = Vec::with_capacity(self.bytes.len() + chunk.len() - incoming_start);
        combined.extend_from_slice(&self.bytes);
        combined.extend_from_slice(&chunk[incoming_start..]);
        let start = tail_start(&combined, self.max_bytes, self.max_lines);
        self.drop(&combined[..start]);
        self.bytes = combined[start..].to_vec();
    }

    /// Upstream `drop` (`bounded.ts:64-67`).
    fn drop(&mut self, bytes: &[u8]) {
        self.dropped_bytes += bytes.len();
        self.dropped_lines += count_newlines(bytes);
    }

    /// Upstream `get dropped` (`bounded.ts:69-71`).
    pub fn dropped(&self) -> usize {
        self.dropped_bytes
    }

    /// Upstream `text` (`bounded.ts:73-75`): lossy UTF-8 decode of the
    /// retained bytes (upstream `TextDecoder` defaults to UTF-8 with
    /// replacement).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Upstream `tailStart` (`bounded.ts:78-85`).
fn tail_start(bytes: &[u8], max_bytes: usize, max_lines: usize) -> usize {
    let mut start = bytes.len().saturating_sub(max_bytes);
    let mut excess_lines = count_newlines(&bytes[start..]).saturating_sub(max_lines);
    while start < bytes.len() && excess_lines > 0 {
        if bytes[start] == 0x0a {
            excess_lines -= 1;
        }
        start += 1;
    }
    start
}

/// Upstream `countNewlines` (`bounded.ts:96-100`).
fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == 0x0a).count()
}
