//! Paste-burst detection for terminals that emit pasted text as key events.
//!
//! Adapted from Codex CLI's `PasteBurst` state machine. Enter is captured only
//! while a burst is active, so the first Enter after a visible paste still submits.

use std::time::Duration;
use tokio::time::Instant;

const CHAR_INTERVAL: Duration = Duration::from_millis(8);

#[cfg(not(windows))]
const ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(8);
#[cfg(windows)]
const ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(60);

#[derive(Debug, Default)]
pub struct PasteBurst {
    last_char_at: Option<Instant>,
    buffer: String,
    active: bool,
    armed: bool,
    pending_first_char: Option<(char, Instant)>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum CharDecision {
    RetainFirst,
    BeginFromPending,
    Append,
}

#[derive(Debug, Eq, PartialEq)]
pub enum FlushResult {
    Paste(String),
    Typed(String),
    None,
}

impl PasteBurst {
    pub fn arm(&mut self) {
        self.armed = true;
    }

    pub fn on_char(&mut self, character: char, now: Instant) -> CharDecision {
        self.last_char_at = Some(now);
        if self.armed {
            self.armed = false;
            self.active = true;
            return CharDecision::Append;
        }
        if self.active {
            return CharDecision::Append;
        }
        if let Some((held, held_at)) = self.pending_first_char {
            if now.duration_since(held_at) <= CHAR_INTERVAL {
                self.active = true;
                self.pending_first_char = None;
                self.buffer.push(held);
                return CharDecision::BeginFromPending;
            }
        }
        self.pending_first_char = Some((character, now));
        CharDecision::RetainFirst
    }

    pub fn append_char(&mut self, character: char, now: Instant) {
        self.buffer.push(character);
        self.last_char_at = Some(now);
    }

    pub fn append_newline_if_active(&mut self, now: Instant) -> bool {
        self.try_append_char_if_active('\n', now)
    }

    pub fn try_append_char_if_active(&mut self, character: char, now: Instant) -> bool {
        if self.armed {
            self.armed = false;
            self.active = true;
        } else if !self.active_internal() {
            return false;
        }
        self.append_char(character, now);
        true
    }

    pub fn deadline(&self) -> Option<Instant> {
        let last = self.last_char_at?;
        Some(
            last + if self.active_internal() {
                ACTIVE_IDLE_TIMEOUT
            } else {
                CHAR_INTERVAL
            },
        )
    }

    pub fn flush_if_due(&mut self, now: Instant) -> FlushResult {
        let Some(deadline) = self.deadline() else {
            return FlushResult::None;
        };
        if now <= deadline {
            return FlushResult::None;
        }
        self.flush()
    }

    pub fn flush(&mut self) -> FlushResult {
        if self.active_internal() {
            self.active = false;
            self.last_char_at = None;
            let text = std::mem::take(&mut self.buffer);
            if text.chars().count() >= 80 || text.contains('\n') || text.contains('\t') {
                return FlushResult::Paste(text);
            }
            return FlushResult::Typed(text);
        }
        if let Some((character, _)) = self.pending_first_char.take() {
            self.last_char_at = None;
            return FlushResult::Typed(character.to_string());
        }
        FlushResult::None
    }

    pub fn cancel(&mut self) {
        self.last_char_at = None;
        self.buffer.clear();
        self.active = false;
        self.armed = false;
        self.pending_first_char = None;
    }

    pub fn clear_after_explicit_paste(&mut self) {
        self.cancel();
    }

    fn active_internal(&self) -> bool {
        self.active || !self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_paste_captures_the_first_character() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        burst.arm();
        assert_eq!(burst.on_char('a', start), CharDecision::Append);
        burst.append_char('a', start);
        assert_eq!(burst.flush(), FlushResult::Typed("a".to_owned()));
    }

    #[test]
    fn one_character_flushes_as_typing() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('a', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.flush_if_due(start + CHAR_INTERVAL + Duration::from_millis(1)),
            FlushResult::Typed("a".to_owned())
        );
        assert!(burst.deadline().is_none());
    }

    #[test]
    fn fast_characters_and_enter_flush_as_one_paste() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('a', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('b', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('b', start + Duration::from_millis(1));
        assert!(burst.append_newline_if_active(start + Duration::from_millis(2)));
        assert_eq!(
            burst.flush_if_due(start + ACTIVE_IDLE_TIMEOUT + Duration::from_millis(4)),
            FlushResult::Paste("ab\n".to_owned())
        );
    }

    #[test]
    fn unicode_characters_activate_the_paste_before_enter() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('你', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('好', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('好', start + Duration::from_millis(1));
        assert!(burst.append_newline_if_active(start + Duration::from_millis(2)));
        assert_eq!(burst.flush(), FlushResult::Paste("你好\n".to_owned()));
    }

    #[test]
    fn short_fast_unicode_text_remains_normal_input() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        assert_eq!(burst.on_char('一', start), CharDecision::RetainFirst);
        assert_eq!(
            burst.on_char('段', start + Duration::from_millis(1)),
            CharDecision::BeginFromPending
        );
        burst.append_char('段', start + Duration::from_millis(1));
        assert_eq!(burst.flush(), FlushResult::Typed("一段".to_owned()));
    }

    #[test]
    fn enter_is_not_captured_after_burst_flush() {
        let mut burst = PasteBurst::default();
        let start = Instant::now();
        burst.on_char('a', start);
        burst.on_char('b', start + Duration::from_millis(1));
        burst.append_char('b', start + Duration::from_millis(1));
        let flushed_at = start + ACTIVE_IDLE_TIMEOUT + Duration::from_millis(2);
        assert!(matches!(
            burst.flush_if_due(flushed_at),
            FlushResult::Typed(_)
        ));
        assert!(!burst.append_newline_if_active(flushed_at));
    }
}
