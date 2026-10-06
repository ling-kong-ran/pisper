//! Port of upstream `packages/tui/src/word-navigation.ts` (findWordBackward /
//! findWordForward) plus the tiny `kill-ring.ts` and `undo-stack.ts` editor
//! support structures.
//!
//! Disclosed substitutions: the default word segmentation is
//! `unicode-segmentation`'s UAX #29 word bounds; upstream `isWordLike` is
//! approximated as a segment containing an alphanumeric char. Cursor offsets
//! are byte offsets in Rust (the Rust editor tracks byte cursors); upstream
//! JS uses UTF-16 indices — identical for the ASCII-heavy editor inputs.

use unicode_segmentation::UnicodeSegmentation;

use crate::tui::utils::is_whitespace_segment;

/// Custom segmenter returning word segments for the given text (upstream
/// `WordNavigationOptions.segment`).
pub type SegmentFn<'a> = &'a dyn Fn(&str) -> Vec<String>;

/// Upstream `WordNavigationOptions`.
#[derive(Default)]
pub struct WordNavigationOptions<'a> {
    /// Custom segmenter returning word segments for the given text.
    pub segment: Option<SegmentFn<'a>>,
    /// Predicate identifying atomic segments treated as single units (e.g.
    /// paste markers).
    pub is_atomic_segment: Option<&'a dyn Fn(&str) -> bool>,
}

#[derive(Clone, Copy, Debug)]
struct Segment<'a> {
    text: &'a str,
    is_word_like: bool,
    is_atomic: bool,
}

fn default_segments<'a>(
    text: &'a str,
    is_atomic: Option<&dyn Fn(&str) -> bool>,
) -> Vec<Segment<'a>> {
    text.split_word_bounds()
        .map(|segment| Segment {
            text: segment,
            is_word_like: segment.chars().any(|c| c.is_alphanumeric()),
            is_atomic: is_atomic.is_some_and(|predicate| predicate(segment)),
        })
        .collect()
}

/// Upstream `findWordBackward`.
pub fn find_word_backward(text: &str, cursor: usize, options: &WordNavigationOptions<'_>) -> usize {
    if cursor == 0 {
        return 0;
    }

    let text_before_cursor = &text[..cursor.min(text.len())];
    let segments: Vec<Segment<'_>> = match options.segment {
        Some(segment) => segment(text_before_cursor)
            .into_iter()
            .map(|segment| Segment {
                is_word_like: segment.chars().any(|c| c.is_alphanumeric()),
                is_atomic: options
                    .is_atomic_segment
                    .is_some_and(|predicate| predicate(&segment)),
                text: Box::leak(segment.into_boxed_str()),
            })
            .collect(),
        None => default_segments(text_before_cursor, options.is_atomic_segment),
    };
    let mut segments = segments;
    let mut new_cursor = cursor;

    // Skip trailing whitespace.
    while let Some(last) = segments.last() {
        if last.is_atomic || !is_whitespace_segment(last.text) {
            break;
        }
        new_cursor -= last.text.len();
        segments.pop();
    }

    if segments.is_empty() {
        return new_cursor;
    }

    let last = *segments.last().expect("checked");

    if last.is_atomic {
        // Skip one atomic segment.
        new_cursor -= last.text.len();
    } else if last.is_word_like {
        // Skip inside one word-like segment, preserving ASCII punctuation
        // boundaries.
        let punctuation_matches: Vec<(usize, char)> = last
            .text
            .char_indices()
            .filter(|(_, c)| is_punctuation_char(*c))
            .collect();
        if punctuation_matches.is_empty() {
            new_cursor -= last.text.len();
        } else {
            let (last_index, last_char) = *punctuation_matches.last().expect("checked");
            // Upstream: newCursor -= segmentLength - (lastMatchIndex +
            // matchLength) — the punctuation and everything BEFORE it stays.
            let tail_len = last.text.len() - (last_index + last_char.len_utf8());
            new_cursor -= tail_len;
        }
    } else {
        // Skip non-word non-whitespace run (punctuation).
        while let Some(last) = segments.last() {
            if last.is_atomic || last.is_word_like || is_whitespace_segment(last.text) {
                break;
            }
            new_cursor -= last.text.len();
            segments.pop();
        }
    }

    new_cursor
}

fn is_punctuation_char(c: char) -> bool {
    crate::tui::utils::is_punctuation_char(c)
}

/// Upstream `findWordForward`.
pub fn find_word_forward(text: &str, cursor: usize, options: &WordNavigationOptions<'_>) -> usize {
    if cursor >= text.len() {
        return text.len();
    }

    let text_after_cursor = &text[cursor.min(text.len())..];
    let segments: Vec<Segment<'_>> = match options.segment {
        Some(segment) => segment(text_after_cursor)
            .into_iter()
            .map(|segment| Segment {
                is_word_like: segment.chars().any(|c| c.is_alphanumeric()),
                is_atomic: options
                    .is_atomic_segment
                    .is_some_and(|predicate| predicate(&segment)),
                text: Box::leak(segment.into_boxed_str()),
            })
            .collect(),
        None => default_segments(text_after_cursor, options.is_atomic_segment),
    };
    let mut iterator = segments.into_iter().peekable();
    let mut new_cursor = cursor;

    // Skip leading whitespace.
    while let Some(next) = iterator.peek() {
        if next.is_atomic || !is_whitespace_segment(next.text) {
            break;
        }
        new_cursor += next.text.len();
        iterator.next();
    }

    let Some(next) = iterator.peek().copied() else {
        return new_cursor;
    };

    if next.is_atomic {
        // Skip one atomic segment.
        new_cursor += next.text.len();
    } else if next.is_word_like {
        // Stop at the first punctuation inside the word-like segment.
        let boundary = next
            .text
            .char_indices()
            .find(|(_, c)| is_punctuation_char(*c))
            .map(|(index, _)| index)
            .unwrap_or(next.text.len());
        new_cursor += boundary;
    } else {
        // Skip non-word non-whitespace run (punctuation).
        while let Some(next) = iterator.peek().copied() {
            if next.is_atomic || next.is_word_like || is_whitespace_segment(next.text) {
                break;
            }
            new_cursor += next.text.len();
            iterator.next();
        }
    }

    new_cursor
}

/// Upstream `KillRing`: ring buffer for Emacs-style kill/yank operations.
#[derive(Clone, Debug, Default)]
pub struct KillRing {
    ring: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct KillRingPushOptions {
    /// If accumulating, prepend (backward deletion) or append (forward).
    pub prepend: bool,
    /// Merge with the most recent entry instead of creating a new one.
    pub accumulate: bool,
}

impl KillRing {
    pub fn push(&mut self, text: &str, opts: KillRingPushOptions) {
        if text.is_empty() {
            return;
        }

        if opts.accumulate && !self.ring.is_empty() {
            let last = self.ring.pop().expect("checked");
            self.ring.push(if opts.prepend {
                format!("{text}{last}")
            } else {
                format!("{last}{text}")
            });
        } else {
            self.ring.push(text.to_string());
        }
    }

    /// Most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&String> {
        self.ring.last()
    }

    /// Move last entry to front (for yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1 {
            let last = self.ring.pop().expect("checked");
            self.ring.insert(0, last);
        }
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}
