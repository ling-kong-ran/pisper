//! Raw-unit counterpart of the upstream ANSI wrapping pipeline.
//!
//! Segmentation uses a mapped view, never a rendered replacement string. SGR
//! state is ASCII; OSC 8 params/URLs and all emitted text remain actual units.
use super::super::{is_cjk_break_char, is_js_trim_whitespace, AnsiCodeTracker};
use super::visible_width_utf16;
use crate::tui::utf16::{raw_text, Utf16Text};
use unicode_segmentation::UnicodeSegmentation;

fn visible_width(text: &Utf16Text) -> usize {
    visible_width_utf16(text.as_units())
}

fn graphemes(text: &Utf16Text) -> Vec<Utf16Text> {
    let mut view = String::new();
    let mut offsets = Vec::new();
    let mut unit = 0;
    for point in char::decode_utf16(text.as_units().iter().copied()) {
        offsets.push((view.len(), unit));
        unit += point.as_ref().map_or(1, |c| c.len_utf16());
        view.push(point.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    offsets.push((view.len(), unit));
    view.grapheme_indices(true)
        .map(|(start, s)| {
            let a = offsets
                .binary_search_by_key(&start, |p| p.0)
                .expect("scalar start");
            let b = offsets
                .binary_search_by_key(&(start + s.len()), |p| p.0)
                .expect("scalar end");
            text.slice(offsets[a].1..offsets[b].1)
        })
        .collect()
}

fn grapheme_width(text: &Utf16Text) -> usize {
    if let Ok(s) = text.to_string_checked() {
        super::super::grapheme_width(&s)
    }
    // Lone-unit clusters cannot include ANSI/control/tab characters: each such
    // control forces a grapheme break. Thus no strip/expand step changes them.
    else {
        visible_width(text)
    }
}

fn is_cjk_break_segment(text: &Utf16Text) -> bool {
    char::decode_utf16(text.as_units().iter().copied())
        .filter_map(Result::ok)
        .any(is_cjk_break_char)
}

fn ansi_length(text: &Utf16Text, start: usize) -> Option<usize> {
    super::ansi_length(text.as_units(), start)
}

fn next_text_run_end(text: &Utf16Text, start: usize) -> usize {
    let mut end = start;
    while end < text.len() && ansi_length(text, end).is_none() {
        end += 1;
    }
    end
}

#[derive(Default)]
struct Tracker {
    sgr: AnsiCodeTracker,
    // Exact open sequence, with the matching BEL/ST closing sequence.
    hyperlink: Option<(Utf16Text, &'static str)>,
}
impl Tracker {
    fn new() -> Self {
        Self::default()
    }
    fn process(&mut self, code: &Utf16Text) {
        let terminator = if code.ends_with('\x07') {
            Some((1, "\x1b]8;;\x07"))
        } else if code.ends_with("\x1b\\") {
            Some((2, "\x1b]8;;\x1b\\"))
        } else {
            None
        };
        if code.starts_with("\x1b]8;") {
            if let Some((tail, close)) = terminator {
                let inner = code.slice(4..code.len() - tail);
                if let Some(separator) = inner.find(';') {
                    self.hyperlink = if separator + 1 == inner.len() {
                        None
                    } else {
                        Some((code.clone(), close))
                    };
                    return;
                }
            }
        }
        if !code.ends_with('m') {
            return;
        }
        // Same unanchored ASCII SGR-body search as AnsiCodeTracker. A malformed
        // unit in the body invalidates that candidate, never normalizes content.
        let units = code.as_units();
        for i in 0..units.len().saturating_sub(1) {
            if units[i..i + 2] != [0x1b, 0x5b] {
                continue;
            }
            let mut end = i + 2;
            while end < units.len() && (matches!(units[end], 0x30..=0x39) || units[end] == 0x3b) {
                end += 1;
            }
            if units.get(end) == Some(&0x6d) {
                let ascii: String = units[i..=end]
                    .iter()
                    .map(|&u| char::from(u as u8))
                    .collect();
                self.sgr.process(&ascii);
                break;
            }
        }
    }
    fn get_active_codes(&self) -> Utf16Text {
        let mut output = Utf16Text::from(self.sgr.get_active_codes());
        if let Some((open, _)) = &self.hyperlink {
            output.push(open);
        }
        output
    }
    fn get_line_end_reset(&self) -> Utf16Text {
        let mut output = Utf16Text::from(self.sgr.get_line_end_reset());
        if let Some((_, close)) = &self.hyperlink {
            output.push(*close);
        }
        output
    }
}

fn update_tracker_from_text(text: &Utf16Text, tracker: &mut Tracker) {
    let mut i = 0;
    while i < text.len() {
        if let Some(len) = ansi_length(text, i) {
            tracker.process(&text.slice(i..i + len));
            i += len;
        } else {
            i += 1;
        }
    }
}

fn is_space_unit(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(is_js_trim_whitespace)
}
fn js_trim(text: &Utf16Text) -> Utf16Text {
    let a = text
        .as_units()
        .iter()
        .position(|&u| !is_space_unit(u))
        .unwrap_or(text.len());
    let b = text
        .as_units()
        .iter()
        .rposition(|&u| !is_space_unit(u))
        .map_or(a, |i| i + 1);
    text.slice(a..b)
}
fn js_trim_end(text: &Utf16Text) -> Utf16Text {
    let b = text
        .as_units()
        .iter()
        .rposition(|&u| !is_space_unit(u))
        .map_or(0, |i| i + 1);
    text.slice(..b)
}
fn js_split_lines(text: &Utf16Text) -> Vec<Utf16Text> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < text.len() {
        match text.as_units()[at] {
            10 | 13 => {
                result.push(text.slice(start..at));
                if text.as_units()[at] == 13 && text.as_units().get(at + 1) == Some(&10) {
                    at += 1;
                }
                start = at + 1;
            }
            _ => {}
        }
        at += 1;
    }
    result.push(text.slice(start..));
    result
}

fn split_into_tokens_with_ansi(text: &Utf16Text) -> Vec<Utf16Text> {
    let mut tokens: Vec<Utf16Text> = Vec::new();
    let mut current = Utf16Text::new();
    let mut pending_ansi = Utf16Text::new();
    let mut current_kind: Option<&'static str> = None;
    let mut i = 0;

    while i < text.len() {
        if let Some(len) = ansi_length(text, i) {
            let ansi = text.slice(i..i + len);
            pending_ansi.push_str(&ansi);
            i += ansi.len();
            continue;
        }

        let end = next_text_run_end(text, i);

        for segment in graphemes(&text.slice(i..end)) {
            let segment_is_space = segment == " ";
            if !segment_is_space && is_cjk_break_segment(&segment) {
                flush_current(&mut tokens, &mut current, &mut current_kind);
                let token = raw_text!(&pending_ansi, segment);
                pending_ansi.clear();
                tokens.push(token);
                continue;
            }

            let segment_kind: &'static str = if segment_is_space { "space" } else { "word" };
            if !current.is_empty() && current_kind != Some(segment_kind) {
                flush_current(&mut tokens, &mut current, &mut current_kind);
            }

            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }

            current_kind = Some(segment_kind);
            current.push_str(segment);
        }

        i = end;
    }

    // Remaining pending ANSI codes attach to the last token.
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn flush_current(
    tokens: &mut Vec<Utf16Text>,
    current: &mut Utf16Text,
    current_kind: &mut Option<&'static str>,
) {
    if current.is_empty() {
        return;
    }
    tokens.push(std::mem::take(current));
    *current_kind = None;
}

/// `wrapTextWithAnsi` (utils.ts:843): word wrapping only — no padding, no
/// background colors; active codes are preserved across line breaks.
pub(crate) fn wrap_text_with_ansi(text: &Utf16Text, width: usize) -> Vec<Utf16Text> {
    if let Ok(s) = text.to_string_checked() {
        return super::super::wrap_text_with_ansi(&s, width)
            .into_iter()
            .map(Utf16Text::from)
            .collect();
    }
    if text.is_empty() {
        return vec![Utf16Text::new()];
    }

    let mut result: Vec<Utf16Text> = Vec::new();
    let mut tracker = Tracker::new();

    for input_line in js_split_lines(text) {
        let prefix = if result.is_empty() {
            Utf16Text::new()
        } else {
            tracker.get_active_codes()
        };
        let line = raw_text!(prefix, &input_line);
        for wrapped_line in wrap_single_line(&line, width) {
            result.push(wrapped_line);
        }
        update_tracker_from_text(&input_line, &mut tracker);
    }

    if result.is_empty() {
        vec![Utf16Text::new()]
    } else {
        result
    }
}

fn wrap_single_line(line: &Utf16Text, width: usize) -> Vec<Utf16Text> {
    if line.is_empty() {
        return vec![Utf16Text::new()];
    }

    let visible_length = visible_width(line);
    if visible_length <= width {
        return vec![line.clone()];
    }

    let mut wrapped: Vec<Utf16Text> = Vec::new();
    let mut tracker = Tracker::new();
    let tokens = split_into_tokens_with_ansi(line);

    let mut current_line = Utf16Text::new();
    let mut current_visible_length = 0usize;

    for token in &tokens {
        let token_visible_length = visible_width(token);
        let is_whitespace = js_trim(token).is_empty();

        // Token itself is too long — break it character by character.
        if token_visible_length > width && !is_whitespace {
            if !current_line.is_empty() {
                // Underline-only reset preserves the background.
                let line_end_reset = tracker.get_line_end_reset();
                if !line_end_reset.is_empty() {
                    current_line.push_str(&line_end_reset);
                }
                wrapped.push(std::mem::take(&mut current_line));
                // Upstream also zeroes currentVisibleLength here; the value is
                // reassigned below before any read, so the dead store is elided.
            }
            // Upstream also zeroes currentVisibleLength here before the
            // long-word break reassigns it below; the dead store is elided.

            let broken = break_long_word(token, width, &mut tracker);
            for piece in &broken[..broken.len() - 1] {
                wrapped.push(piece.clone());
            }
            current_line = broken[broken.len() - 1].clone();
            current_visible_length = visible_width(&current_line);
            continue;
        }

        let total_needed = current_visible_length + token_visible_length;

        if total_needed > width && current_visible_length > 0 {
            let mut line_to_wrap = js_trim_end(&current_line);
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                line_to_wrap.push_str(&line_end_reset);
            }
            wrapped.push(line_to_wrap);
            if is_whitespace {
                // Don't start a new line with whitespace.
                current_line = tracker.get_active_codes();
                current_visible_length = 0;
            } else {
                current_line = raw_text!(tracker.get_active_codes(), token);
                current_visible_length = token_visible_length;
            }
        } else {
            current_line.push_str(token);
            current_visible_length += token_visible_length;
        }

        update_tracker_from_text(token, &mut tracker);
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }

    // Trailing whitespace can push lines past the requested width.
    if wrapped.is_empty() {
        vec![Utf16Text::new()]
    } else {
        wrapped.into_iter().map(|line| js_trim_end(&line)).collect()
    }
}

/// `breakLongWord` (utils.ts:965).
fn break_long_word(word: &Utf16Text, width: usize, tracker: &mut Tracker) -> Vec<Utf16Text> {
    let mut lines: Vec<Utf16Text> = Vec::new();
    let mut current_line = tracker.get_active_codes();
    let mut current_width = 0usize;

    let mut i = 0;
    let mut segments: Vec<(bool, Utf16Text)> = Vec::new(); // (is_ansi, value)

    while i < word.len() {
        if let Some(len) = ansi_length(word, i) {
            segments.push((true, word.slice(i..i + len)));
            i += len;
        } else {
            let end = next_text_run_end(word, i);
            for seg in graphemes(&word.slice(i..end)) {
                segments.push((false, seg));
            }
            i = end.max(i + 1);
        }
    }

    for (is_ansi, value) in segments {
        if is_ansi {
            current_line.push_str(&value);
            tracker.process(&value);
            continue;
        }

        if value.is_empty() {
            continue;
        }

        let grapheme_w = grapheme_width(&value);

        if current_width + grapheme_w > width {
            // Underline-only reset preserves the background.
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                current_line.push_str(&line_end_reset);
            }
            lines.push(std::mem::take(&mut current_line));
            current_line = tracker.get_active_codes();
            current_width = 0;
        }

        current_line.push_str(&value);
        current_width += grapheme_w;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        vec![Utf16Text::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::{wrap_text_with_ansi, Utf16Text};
    use serde::Deserialize;

    #[test]
    fn raw_wrapping_matches_actual_upstream_including_osc_url_units() {
        #[derive(Deserialize)]
        struct Corpus {
            cases: Vec<Case>,
        }
        #[derive(Deserialize)]
        struct Case {
            name: String,
            source: Vec<u16>,
            width: usize,
            expected: Vec<Vec<u16>>,
        }
        let corpus: Corpus = serde_json::from_str(include_str!("wrap-fixtures.json")).unwrap();
        assert_eq!(corpus.cases.len(), 2152);
        for case in corpus.cases {
            let actual: Vec<_> =
                wrap_text_with_ansi(&Utf16Text::from_units(case.source), case.width)
                    .into_iter()
                    .map(Utf16Text::into_units)
                    .collect();
            assert_eq!(actual, case.expected, "upstream wrapper case {}", case.name);
        }
    }
}
