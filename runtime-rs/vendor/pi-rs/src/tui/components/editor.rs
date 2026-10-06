//! Port of upstream `packages/tui/src/components/editor.ts` (core editing
//! state machine): multi-line editing with grapheme-aware cursor operations,
//! word wrapping with CJK break opportunities, paste-marker atomic segments,
//! kill ring, undo snapshots, prompt history, character jump, scroll
//! indicators and the sticky-column vertical navigation.
//!
//! Disclosed substitutions for review:
//! - Cursor/column offsets are byte offsets (JS: UTF-16 code units); identical
//!   for ASCII-heavy inputs and consistent within the port because every edit
//!   lands on grapheme boundaries.
//! - The autocomplete integration (SelectList, AutocompleteProvider, trigger
//!   and debounce patterns, request callbacks) is NOT ported yet; the
//!   autocomplete branches of `handleInput` join with the select-list slice.
//!   `autocompleteMaxVisible` is stored for API parity.
//! - The upstream constructor receives the `TUI` for `requestRender()` and
//!   `terminal.rows`. The port takes injectable closures instead
//!   ([`Editor::with_terminal_rows`] / [`Editor::with_request_render`]).
//! - Paste-marker segmentation operates on byte offsets produced by
//!   grapheme segmentation.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use regex::Regex;

use crate::tui::autocomplete::{AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions};
use crate::tui::component::{Component, CURSOR_MARKER};
use crate::tui::components::select_list::SelectList;
use crate::tui::keybindings::with_keybindings;
use crate::tui::keys::{decode_printable_key, matches_key};
use crate::tui::undo_stack::UndoStack;
use crate::tui::utils::{
    is_autocomplete_separator_char, is_cjk_break_char, is_whitespace_char, is_whitespace_segment,
    slice_by_column, visible_width,
};
use crate::tui::word_navigation::{
    find_word_backward, find_word_forward, KillRing, KillRingPushOptions,
};
use unicode_segmentation::UnicodeSegmentation;

// ---------------------------------------------------------------------------
// Paste markers
// ---------------------------------------------------------------------------

fn paste_marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]").unwrap())
}

fn paste_marker_single_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$").unwrap())
}

/// Upstream `isPasteMarker`: a whole segment that is a paste marker.
pub fn is_paste_marker(segment: &str) -> bool {
    segment.len() >= 10 && paste_marker_single_regex().is_match(segment)
}

/// Find all paste-marker spans with valid ids in `text`
/// (upstream `segmentWithMarkers` marker scan). Returns (start, end, id).
fn find_marker_spans(text: &str, valid_ids: &BTreeMap<i64, String>) -> Vec<(usize, usize, i64)> {
    let mut spans = Vec::new();
    for captures in paste_marker_regex().captures_iter(text) {
        let Some(capture) = captures.get(0) else {
            continue;
        };
        let Some(id_group) = captures.get(1) else {
            continue;
        };
        let Ok(id) = id_group.as_str().parse::<i64>() else {
            continue;
        };
        if !valid_ids.contains_key(&id) {
            continue;
        }
        spans.push((capture.start(), capture.end(), id));
    }
    spans
}

/// Upstream `segmentWithMarkers`: merge graphemes inside paste markers into
/// single atomic segments. `base_segments` are (segment, byte_index) pairs.
fn segment_with_markers(
    text: &str,
    base_segments: Vec<(String, usize)>,
    valid_ids: &BTreeMap<i64, String>,
) -> Vec<(String, usize)> {
    if valid_ids.is_empty() || !text.contains("[paste #") {
        return base_segments;
    }

    let markers = find_marker_spans(text, valid_ids);
    if markers.is_empty() {
        return base_segments;
    }

    let mut result: Vec<(String, usize)> = Vec::new();
    let mut marker_index = 0usize;

    for (segment, index) in base_segments {
        while marker_index < markers.len() && markers[marker_index].1 <= index {
            marker_index += 1;
        }

        let marker = markers.get(marker_index);
        match marker {
            Some(&(start, end, _)) if index >= start && index < end => {
                if index == start {
                    result.push((text[start..end].to_string(), start));
                }
            }
            _ => result.push((segment, index)),
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Word wrapping
// ---------------------------------------------------------------------------

/// Upstream `TextChunk`: a wrapped chunk with its position in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChunk {
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}

fn cjk_break_segment(segment: &str) -> bool {
    // Upstream cjkBreakRegex: Script_Extensions Han/Hiragana/Katakana/Hangul/
    // Bopomofo (identical to the tui::utils helper).
    crate::tui::utils::is_cjk_break_segment(segment)
}

/// Upstream `wordWrapLine`: split a line into word-wrapped chunks, wrapping at
/// word boundaries when possible and falling back to character-level wrapping.
/// `pre_segmented` supplies grapheme segments as (segment, byte_index) pairs
/// (e.g. paste-marker aware segmentation).
pub fn word_wrap_line(
    line: &str,
    max_width: usize,
    pre_segmented: Option<&[(String, usize)]>,
) -> Vec<TextChunk> {
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }

    let line_width = visible_width(line);
    if line_width <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: line.len(),
        }];
    }

    let mut chunks: Vec<TextChunk> = Vec::new();
    let segments: Vec<(String, usize)> = match pre_segmented {
        Some(segments) => segments.to_vec(),
        None => line
            .grapheme_indices(true)
            .map(|(index, segment)| (segment.to_string(), index))
            .collect(),
    };

    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    let mut wrap_opp_index: Option<usize> = None;
    let mut wrap_opp_width = 0usize;

    for i in 0..segments.len() {
        let (grapheme, char_index) = (&segments[i].0, segments[i].1);
        let g_width = visible_width(grapheme);
        let is_ws = !is_paste_marker(grapheme) && is_whitespace_segment(grapheme);

        // Overflow check before advancing.
        if current_width + g_width > max_width {
            if let Some(wrap_index) = wrap_opp_index {
                if current_width - wrap_opp_width + g_width <= max_width {
                    // Backtrack to the last wrap opportunity.
                    chunks.push(TextChunk {
                        text: line[chunk_start..wrap_index].to_string(),
                        start_index: chunk_start,
                        end_index: wrap_index,
                    });
                    chunk_start = wrap_index;
                    current_width -= wrap_opp_width;
                } else if chunk_start < char_index {
                    // Force-break at the current position.
                    chunks.push(TextChunk {
                        text: line[chunk_start..char_index].to_string(),
                        start_index: chunk_start,
                        end_index: char_index,
                    });
                    chunk_start = char_index;
                    current_width = 0;
                }
            } else if chunk_start < char_index {
                chunks.push(TextChunk {
                    text: line[chunk_start..char_index].to_string(),
                    start_index: chunk_start,
                    end_index: char_index,
                });
                chunk_start = char_index;
                current_width = 0;
            }
            wrap_opp_index = None;
        }

        if g_width > max_width {
            // Single atomic segment wider than maxWidth: re-wrap visually at
            // grapheme granularity (stays logically atomic).
            let sub_chunks = word_wrap_line(grapheme, max_width, None);
            for sub in &sub_chunks[..sub_chunks.len() - 1] {
                chunks.push(TextChunk {
                    text: sub.text.clone(),
                    start_index: char_index + sub.start_index,
                    end_index: char_index + sub.end_index,
                });
            }
            if let Some(last) = sub_chunks.last() {
                chunk_start = char_index + last.start_index;
                current_width = visible_width(&last.text);
            }
            wrap_opp_index = None;
            continue;
        }

        // Advance.
        current_width += g_width;

        // Record wrap opportunities: after whitespace runs, or at CJK
        // boundaries.
        if let Some(next) = segments.get(i + 1) {
            let (next_grapheme, next_index) = (&next.0, next.1);
            if is_ws && !is_paste_marker(next_grapheme) && !is_whitespace_segment(next_grapheme) {
                wrap_opp_index = Some(next_index);
                wrap_opp_width = current_width;
            } else if !is_ws
                && !is_paste_marker(next_grapheme)
                && !is_whitespace_segment(next_grapheme)
            {
                let is_cjk = !is_paste_marker(grapheme) && cjk_break_segment(grapheme);
                let next_is_cjk = cjk_break_segment(next_grapheme);
                if is_cjk || next_is_cjk {
                    wrap_opp_index = Some(next_index);
                    wrap_opp_width = current_width;
                }
            }
        }
    }

    // Final chunk.
    chunks.push(TextChunk {
        text: line[chunk_start..].to_string(),
        start_index: chunk_start,
        end_index: line.len(),
    });

    chunks
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// Upstream `EditorState`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorState {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

/// Upstream `EditorSnapshot`: editor text state plus the paste registry.
#[derive(Clone, Debug, Default)]
pub struct EditorSnapshot {
    pub state: EditorState,
    pub pastes: BTreeMap<i64, String>,
    pub paste_counter: i64,
}

#[derive(Clone, Debug, Default)]
struct LayoutLine {
    text: String,
    has_cursor: bool,
    cursor_pos: Option<usize>,
}

/// A visual line segment: logical line + column range
/// (upstream `buildVisualLineMap` entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualLine {
    pub logical_line: usize,
    pub start_col: usize,
    pub length: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpDirection {
    Forward,
    Backward,
}

/// Upstream `autocompleteState`: `null` = inactive, "regular" or "force".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteMode {
    Regular,
    Force,
}

/// Border painter (upstream `EditorTheme.borderColor`).
pub type BorderColorFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Terminal row-count accessor (upstream `tui.terminal.rows`).
pub type TerminalRowsFn = Box<dyn Fn() -> usize + Send>;

/// Render-request callback (upstream `tui.requestRender()`).
pub type RequestRenderFn = Box<dyn FnMut() + Send>;

/// Submit callback (upstream `onSubmit`).
pub type SubmitCallback = Box<dyn FnMut(&str) + Send>;

/// Change callback (upstream `onChange`).
pub type ChangeCallback = Box<dyn FnMut(&str) + Send>;

/// Upstream `EditorOptions`.
#[derive(Clone, Debug, Default)]
pub struct EditorOptions {
    pub padding_x: usize,
    pub autocomplete_max_visible: Option<usize>,
}

/// Upstream `Editor` (core editing; autocomplete integration pending).
pub struct Editor {
    state: EditorState,
    focused: bool,
    padding_x: usize,
    last_width: usize,
    rendered_visible_line_count: usize,
    rendered_autocomplete_height: usize,
    scroll_offset: usize,
    border_color: BorderColorFn,
    terminal_rows: TerminalRowsFn,
    request_render: RequestRenderFn,

    // Prompt history.
    history: Vec<String>,
    history_index: i64,
    history_draft: Option<EditorState>,

    // Kill ring.
    kill_ring: KillRing,
    last_action: Option<LastAction>,

    // Character jump mode.
    jump_mode: Option<JumpDirection>,

    // Sticky column for vertical movement.
    preferred_visual_col: Option<usize>,
    snapped_from_cursor_col: Option<usize>,

    // Undo.
    undo_stack: UndoStack<EditorSnapshot>,

    // Paste tracking.
    pastes: BTreeMap<i64, String>,
    paste_counter: i64,
    paste_buffer: String,
    is_in_paste: bool,

    // Autocomplete support (provider integration; SelectList-backed picker).
    autocomplete_provider: Option<Arc<dyn AutocompleteProvider + Send + Sync>>,
    autocomplete_list: Option<SelectList>,
    autocomplete_state: Option<AutocompleteMode>,
    autocomplete_prefix: String,
    autocomplete_max_visible: usize,
    /// Upstream `autocompleteTriggerCharacters` (default ["@", "#"]).
    autocomplete_trigger_characters: Vec<char>,

    pub disable_submit: bool,
    on_submit: Option<SubmitCallback>,
    on_change: Option<ChangeCallback>,
}

fn identity_border(border: &str) -> String {
    border.to_string()
}

/// Upstream `buildTriggerPattern(triggerCharacters).test(text)` (editor.ts:263):
/// the pattern is
/// `(?:^|(?:\s|cjkPunctuation))[([{<\`]*(?:@"[^"]*|[trigger](?:(?!\s|cjkPunctuation).)*)$`
/// with the `u` flag. The negative lookahead becomes an all-chars predicate, so
/// this is a hand matcher instead of the `regex` crate.
fn autocomplete_trigger_pattern_matches(text: &str, trigger_characters: &[char]) -> bool {
    let is_wrapper = |c: char| matches!(c, '(' | '[' | '{' | '<' | '`');
    // Candidate starts: position 0 with the `^` branch consuming nothing, or
    // any position whose first code point is a separator (consumed by the
    // separator branch).
    let attempt = |start: usize, consumed: bool| -> bool {
        let mut cursor = if consumed {
            start + text[start..].chars().next().expect("char").len_utf8()
        } else {
            start
        };
        // [([{<`]*
        while let Some(c) = text[cursor..].chars().next() {
            if !is_wrapper(c) {
                break;
            }
            cursor += c.len_utf8();
        }
        // (?:@"[^"]*|[trigger]nonsep*)$
        if text[cursor..].starts_with("@\"") && !text[cursor + 2..].contains('"') {
            return true;
        }
        if let Some(c) = text[cursor..].chars().next() {
            if trigger_characters.contains(&c) {
                let rest = &text[cursor + c.len_utf8()..];
                if rest.chars().all(|ch| !is_autocomplete_separator_char(ch)) {
                    return true;
                }
            }
        }
        false
    };
    if attempt(0, false) {
        return true;
    }
    for (index, character) in text.char_indices() {
        if is_autocomplete_separator_char(character) && attempt(index, true) {
            return true;
        }
    }
    false
}

/// Oracle-test hook for the private trigger matcher.
#[cfg(test)]
pub(crate) fn editor_trigger_pattern_matches_for_test(text: &str, characters: &[char]) -> bool {
    autocomplete_trigger_pattern_matches(text, characters)
}

fn default_terminal_rows() -> usize {
    24
}

fn default_request_render() {}

impl Editor {
    pub fn new(theme_border_color: Option<BorderColorFn>, options: EditorOptions) -> Self {
        let padding_x = options.padding_x;
        let max_visible = options.autocomplete_max_visible.unwrap_or(5).clamp(3, 20);
        let _ = max_visible;
        Self {
            state: EditorState {
                lines: vec![String::new()],
                cursor_line: 0,
                cursor_col: 0,
            },
            focused: false,
            padding_x,
            last_width: 80,
            rendered_visible_line_count: 1,
            rendered_autocomplete_height: 0,
            scroll_offset: 0,
            border_color: theme_border_color.unwrap_or_else(|| Arc::new(identity_border)),
            terminal_rows: Box::new(default_terminal_rows),
            request_render: Box::new(default_request_render),
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            kill_ring: KillRing::default(),
            last_action: None,
            jump_mode: None,
            preferred_visual_col: None,
            snapped_from_cursor_col: None,
            undo_stack: UndoStack::new(),
            pastes: BTreeMap::new(),
            paste_counter: 0,
            paste_buffer: String::new(),
            is_in_paste: false,
            autocomplete_provider: None,
            autocomplete_list: None,
            autocomplete_state: None,
            autocomplete_prefix: String::new(),
            autocomplete_max_visible: 5,
            autocomplete_trigger_characters: vec!['@', '#'],
            disable_submit: false,
            on_submit: None,
            on_change: None,
        }
    }

    /// Upstream constructor injects the TUI for `terminal.rows`.
    pub fn with_terminal_rows(mut self, rows: impl Fn() -> usize + Send + 'static) -> Self {
        self.terminal_rows = Box::new(rows);
        self
    }

    pub fn with_request_render(mut self, request: impl FnMut() + Send + 'static) -> Self {
        self.request_render = Box::new(request);
        self
    }

    pub fn on_submit(&mut self, callback: impl FnMut(&str) + Send + 'static) {
        self.on_submit = Some(Box::new(callback));
    }

    pub fn on_change(&mut self, callback: impl FnMut(&str) + Send + 'static) {
        self.on_change = Some(Box::new(callback));
    }

    pub fn get_padding_x(&self) -> usize {
        self.padding_x
    }

    pub fn set_padding_x(&mut self, padding: usize) {
        if self.padding_x != padding {
            self.padding_x = padding;
            (self.request_render)();
        }
    }

    fn valid_paste_ids(&self) -> BTreeMap<i64, String> {
        self.pastes.clone()
    }

    fn segment(&self, text: &str, mode: &str) -> Vec<(String, usize)> {
        let base: Vec<(String, usize)> = match mode {
            "word" => split_word_segments(text),
            _ => text
                .grapheme_indices(true)
                .map(|(index, segment)| (segment.to_string(), index))
                .collect(),
        };
        segment_with_markers(text, base, &self.valid_paste_ids())
    }

    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some(first) = self.history.first() {
            if first == trimmed {
                return;
            }
        }
        self.history.insert(0, trimmed.to_string());
        if self.history.len() > 100 {
            self.history.pop();
        }
    }

    fn is_editor_empty(&self) -> bool {
        self.state.lines.len() == 1 && self.state.lines[0].is_empty()
    }

    fn is_on_first_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) == 0
    }

    fn is_on_last_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current = self.find_current_visual_line(&visual_lines);
        current + 1 == visual_lines.len()
    }

    fn navigate_history(&mut self, direction: i64) {
        self.last_action = None;
        eprintln!(
            "[dbg-nav] enter dir={direction} index={} len={}",
            self.history_index,
            self.history.len()
        );
        if self.history.is_empty() {
            return;
        }

        let new_index = self.history_index - direction;
        eprintln!(
            "[dbg-nav] new_index={new_index} draft={:?}",
            self.history_draft.is_some()
        );
        if new_index < -1 || new_index >= self.history.len() as i64 {
            return;
        }

        if self.history_index == -1 && new_index >= 0 {
            self.push_undo_snapshot();
            self.history_draft = Some(self.state.clone());
        }

        self.history_index = new_index;
        eprintln!("[dbg-nav] history_index now {}", self.history_index);

        if self.history_index == -1 {
            if let Some(draft) = self.history_draft.take() {
                self.state = draft;
                self.preferred_visual_col = None;
                self.snapped_from_cursor_col = None;
                self.scroll_offset = 0;
                let text = self.get_text();
                if let Some(on_change) = &mut self.on_change {
                    on_change(&text);
                }
            } else {
                self.set_text_internal("");
            }
        } else {
            let entry = self
                .history
                .get(self.history_index as usize)
                .cloned()
                .unwrap_or_default();
            self.set_text_internal_cursor(&entry, direction == -1);
        }
    }

    fn exit_history_browsing(&mut self) {
        self.history_index = -1;
        self.history_draft = None;
    }

    fn set_text_internal_cursor(&mut self, text: &str, at_start: bool) {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        self.state.lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        self.state.cursor_line = if at_start {
            0
        } else {
            self.state.lines.len() - 1
        };
        let col = if at_start {
            0
        } else {
            self.state.lines[self.state.cursor_line].len()
        };
        self.set_cursor_col(col);
        self.scroll_offset = 0;
        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn set_text_internal(&mut self, text: &str) {
        self.set_text_internal_cursor(text, false);
    }

    fn create_scroll_border(
        &self,
        direction: char,
        hidden_line_count: usize,
        width: usize,
    ) -> String {
        let available_width = width;
        let label = format!(" {direction} {hidden_line_count} more ");
        let label_width = visible_width(&label);
        if label_width + 2 <= available_width {
            let left_width = (available_width - label_width) / 2;
            return format!(
                "{}{label}{}",
                "\u{2500}".repeat(left_width),
                "\u{2500}".repeat(available_width - left_width - label_width)
            );
        }

        let indicator = format!("\u{2500}\u{2500}\u{2500} {direction} {hidden_line_count} more ");
        let remaining = available_width.saturating_sub(visible_width(&indicator));
        if visible_width(&indicator) <= available_width {
            return format!("{indicator}{}", "\u{2500}".repeat(remaining));
        }

        let ellipsis = "...".chars().take(available_width).collect::<String>();
        let indicator_width = available_width - visible_width(&ellipsis);
        format!(
            "{}{}",
            slice_by_column(&indicator, 0, indicator_width, true),
            ellipsis
        )
    }

    fn render_top_border(&self, width: usize, hidden_line_count: usize) -> String {
        let border = if hidden_line_count > 0 {
            self.create_scroll_border('\u{2191}', hidden_line_count, width)
        } else {
            "\u{2500}".repeat(width)
        };
        (self.border_color)(&border)
    }

    fn render_bottom_border(&self, width: usize, hidden_line_count: usize) -> String {
        let border = if hidden_line_count > 0 {
            self.create_scroll_border('\u{2193}', hidden_line_count, width)
        } else {
            "\u{2500}".repeat(width)
        };
        (self.border_color)(&border)
    }

    fn render_impl(&mut self, width: usize) -> Vec<String> {
        let max_padding = width.saturating_sub(1) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = (width - padding_x * 2).max(1);
        let layout_width = content_width.max(1) - if padding_x == 0 { 1 } else { 0 };

        self.last_width = layout_width;

        let layout_lines = self.layout_text(layout_width);

        let terminal_rows = (self.terminal_rows)();
        let max_visible_lines = (terminal_rows * 3 / 10).max(5);

        let cursor_line_index = layout_lines
            .iter()
            .position(|line| line.has_cursor)
            .unwrap_or(0);

        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index - max_visible_lines + 1;
        }

        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        let visible_lines: Vec<LayoutLine> = layout_lines
            .iter()
            .skip(self.scroll_offset)
            .take(max_visible_lines)
            .cloned()
            .collect();
        self.rendered_visible_line_count = visible_lines.len();

        let mut result: Vec<String> = Vec::new();
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();

        result.push(self.render_top_border(width, self.scroll_offset));

        let emit_cursor_marker = self.focused;

        for layout_line in &visible_lines {
            let mut display_text = layout_line.text.clone();
            let mut line_visible_width = visible_width(&layout_line.text);
            let mut cursor_in_padding = false;

            if layout_line.has_cursor && layout_line.cursor_pos.is_some() {
                let cursor_pos = layout_line.cursor_pos.unwrap_or(0).min(display_text.len());
                let before = display_text[..cursor_pos].to_string();
                let after = display_text[cursor_pos..].to_string();
                let marker = if emit_cursor_marker {
                    CURSOR_MARKER
                } else {
                    ""
                };

                if !after.is_empty() {
                    let first_grapheme = after.graphemes(true).next().unwrap_or("").to_string();
                    let rest_after = after[first_grapheme.len()..].to_string();
                    let cursor = format!("\x1b[7m{first_grapheme}\x1b[0m");
                    display_text = format!("{before}{marker}{cursor}{rest_after}");
                } else {
                    let cursor = "\x1b[7m \x1b[0m".to_string();
                    display_text = format!("{before}{marker}{cursor}");
                    line_visible_width += 1;
                    if line_visible_width > content_width && padding_x > 0 {
                        cursor_in_padding = true;
                    }
                }
            }

            let padding = " ".repeat(content_width.saturating_sub(line_visible_width));
            let line_right_padding = if cursor_in_padding {
                right_padding[1..].to_string()
            } else {
                right_padding.clone()
            };

            result.push(format!(
                "{left_padding}{display_text}{padding}{line_right_padding}"
            ));
        }

        let lines_below =
            layout_lines.len() - (self.scroll_offset + visible_lines.len()).min(layout_lines.len());
        result.push(self.render_bottom_border(width, lines_below));

        // Add autocomplete list if active.
        self.rendered_autocomplete_height = 0;
        if let Some(list) = self.autocomplete_list.as_mut() {
            let autocomplete_result = list.render(content_width);
            self.rendered_autocomplete_height = autocomplete_result.len();
            for line in autocomplete_result {
                let line_width = visible_width(&line);
                let line_padding = " ".repeat(content_width.saturating_sub(line_width));
                result.push(format!("{left_padding}{line}{line_padding}{right_padding}"));
            }
        }

        result
    }

    fn layout_text(&self, content_width: usize) -> Vec<LayoutLine> {
        let mut layout_lines: Vec<LayoutLine> = Vec::new();

        if self.state.lines.is_empty()
            || (self.state.lines.len() == 1 && self.state.lines[0].is_empty())
        {
            layout_lines.push(LayoutLine {
                text: String::new(),
                has_cursor: true,
                cursor_pos: Some(0),
            });
            return layout_lines;
        }

        for (i, line) in self.state.lines.iter().enumerate() {
            let line = line.as_str();
            let is_current_line = i == self.state.cursor_line;
            let line_visible_width = visible_width(line);

            if line_visible_width <= content_width {
                layout_lines.push(LayoutLine {
                    text: line.to_string(),
                    has_cursor: is_current_line,
                    cursor_pos: if is_current_line {
                        Some(self.state.cursor_col)
                    } else {
                        None
                    },
                });
            } else {
                let pre_segmented = self.segment(line, "grapheme");
                let chunks = word_wrap_line(line, content_width, Some(&pre_segmented));

                for (chunk_index, chunk) in chunks.iter().enumerate() {
                    let cursor_pos = self.state.cursor_col;
                    let is_last_chunk = chunk_index == chunks.len() - 1;

                    let mut has_cursor_in_chunk = false;
                    let mut adjusted_cursor_pos = 0usize;

                    if is_current_line {
                        if is_last_chunk {
                            has_cursor_in_chunk = cursor_pos >= chunk.start_index;
                            adjusted_cursor_pos = cursor_pos - chunk.start_index;
                        } else {
                            has_cursor_in_chunk =
                                cursor_pos >= chunk.start_index && cursor_pos < chunk.end_index;
                            if has_cursor_in_chunk {
                                adjusted_cursor_pos = cursor_pos - chunk.start_index;
                                if adjusted_cursor_pos > chunk.text.len() {
                                    adjusted_cursor_pos = chunk.text.len();
                                }
                            }
                        }
                    }

                    layout_lines.push(LayoutLine {
                        text: chunk.text.clone(),
                        has_cursor: has_cursor_in_chunk,
                        cursor_pos: if has_cursor_in_chunk {
                            Some(adjusted_cursor_pos)
                        } else {
                            None
                        },
                    });
                }
            }
        }

        layout_lines
    }

    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    fn expand_paste_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (paste_id, paste_content) in &self.pastes {
            let pattern = Regex::new(&format!(
                r"\[paste #{paste_id}( (\+\d+ lines|\d+ chars))?\]"
            ))
            .expect("static pattern");
            let content = paste_content.clone();
            fn replace_match(_captures: &regex::Captures, content: &str) -> String {
                content.to_string()
            }
            let content = content.clone();
            result = pattern
                .replace_all(&result, |captures: &regex::Captures| {
                    replace_match(captures, &content)
                })
                .into_owned();
        }
        result
    }

    pub fn get_expanded_text(&self) -> String {
        self.expand_paste_markers(&self.get_text())
    }

    pub fn get_lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    pub fn get_cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    pub fn set_text(&mut self, text: &str) {
        self.last_action = None;
        self.exit_history_browsing();
        let normalized = normalize_text(text);
        if self.get_text() != normalized {
            self.push_undo_snapshot();
        }
        self.pastes.clear();
        self.paste_counter = 0;
        self.set_text_internal(&normalized);
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        self.last_action = None;
        self.exit_history_browsing();
        self.insert_text_at_cursor_internal(text);
    }

    fn insert_text_at_cursor_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let normalized = normalize_text(text);
        let inserted_lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();

        let current_line = self.state.lines[self.state.cursor_line].clone();
        let before_cursor = current_line[..self.state.cursor_col].to_string();
        let after_cursor = current_line[self.state.cursor_col..].to_string();

        if inserted_lines.len() == 1 {
            self.state.lines[self.state.cursor_line] =
                format!("{before_cursor}{normalized}{after_cursor}");
            self.set_cursor_col(self.state.cursor_col + normalized.len());
        } else {
            let mut lines: Vec<String> = Vec::new();
            lines.extend(self.state.lines[..self.state.cursor_line].iter().cloned());
            lines.push(format!("{before_cursor}{}", inserted_lines[0]));
            lines.extend(inserted_lines[1..inserted_lines.len() - 1].iter().cloned());
            lines.push(format!(
                "{}{after_cursor}",
                inserted_lines[inserted_lines.len() - 1]
            ));
            lines.extend(
                self.state.lines[self.state.cursor_line + 1..]
                    .iter()
                    .cloned(),
            );

            self.state.lines = lines;
            self.state.cursor_line += inserted_lines.len() - 1;
            let last = inserted_lines[inserted_lines.len() - 1].len();
            self.set_cursor_col(last);
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn insert_character(&mut self, text: &str, skip_undo_coalescing: bool) {
        self.exit_history_browsing();

        // Upstream's trigger checks test the typed `char`; keep it reachable
        // before the full-buffer `text` shadow below.
        let typed: &str = text;
        if !skip_undo_coalescing {
            let first_char_is_ws = text.chars().next().is_some_and(is_whitespace_char);
            if first_char_is_ws || self.last_action != Some(LastAction::TypeWord) {
                self.push_undo_snapshot();
            }
            self.last_action = Some(LastAction::TypeWord);
        }

        let line = &self.state.lines[self.state.cursor_line];
        let before = line[..self.state.cursor_col].to_string();
        let after = line[self.state.cursor_col..].to_string();
        self.state.lines[self.state.cursor_line] = format!("{before}{text}{after}");
        self.set_cursor_col(self.state.cursor_col + text.len());

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }

        // Autocomplete trigger checks (upstream editor.ts:1227-1252).
        if self.autocomplete_state.is_none() {
            let current_line = self.state.lines[self.state.cursor_line].clone();
            let text_before_cursor = &current_line[..self.state.cursor_col];
            if typed == "/" && self.is_at_start_of_message() {
                self.try_trigger_autocomplete();
            }
            // Auto-trigger for symbol-based completion like @, #, or provider
            // triggers at token boundaries.
            else if typed
                .chars()
                .next()
                .is_some_and(|c| self.autocomplete_trigger_characters.contains(&c))
            {
                if self.autocomplete_trigger_pattern_matches(text_before_cursor) {
                    self.try_trigger_autocomplete();
                }
            }
            // Also auto-trigger when typing letters in a slash command or
            // symbol completion context.
            else if typed.len() == 1
                && typed.chars().next().is_some_and(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '-' | '_')
                        || is_cjk_break_char(c)
                })
            {
                // Check if we're in a slash command (with or without space
                // for arguments) or a symbol-based completion context like @,
                // #, or provider triggers.
                if self.is_in_slash_command_context(text_before_cursor)
                    || self.autocomplete_trigger_pattern_matches(text_before_cursor)
                {
                    self.try_trigger_autocomplete();
                }
            }
        } else {
            self.update_autocomplete(false);
        }
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.exit_history_browsing();
        self.last_action = None;
        self.push_undo_snapshot();

        // Decode CSI-u Ctrl+<letter> sequences some terminals re-encode inside
        // pastes (upstream editor.ts:1260).
        let decoded = decode_pasted_csi_u(pasted_text);
        let clean_text = normalize_text(&decoded);

        let filtered_text: String = clean_text
            .chars()
            .filter(|&c| c == '\n' || (c as u32) >= 32)
            .collect();

        let mut filtered_text = filtered_text;
        if filtered_text.starts_with(['/', '~', '.']) && self.state.cursor_col > 0 {
            let current_line = &self.state.lines[self.state.cursor_line];
            let char_before = current_line[..self.state.cursor_col].chars().next_back();
            if char_before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                filtered_text = format!(" {filtered_text}");
            }
        }

        let pasted_lines: Vec<&str> = filtered_text.split('\n').collect();
        let total_chars = filtered_text.len();

        if pasted_lines.len() > 10 || total_chars > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, filtered_text.clone());

            let marker = if pasted_lines.len() > 10 {
                format!("[paste #{paste_id} +{} lines]", pasted_lines.len())
            } else {
                format!("[paste #{paste_id} {total_chars} chars]")
            };
            self.insert_text_at_cursor_internal(&marker);
            return;
        }

        self.insert_text_at_cursor_internal(&filtered_text);
    }

    fn add_new_line(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;
        self.push_undo_snapshot();

        let current_line = self.state.lines[self.state.cursor_line].clone();
        let before = current_line[..self.state.cursor_col].to_string();
        let after = current_line[self.state.cursor_col..].to_string();

        self.state.lines[self.state.cursor_line] = before;
        self.state.lines.insert(self.state.cursor_line + 1, after);

        self.state.cursor_line += 1;
        self.set_cursor_col(0);

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn should_submit_on_backslash_enter(&self, data: &str) -> bool {
        if self.disable_submit {
            return false;
        }
        if !matches_key(data, "enter") {
            return false;
        }
        let has_shift_enter = with_keybindings(|kb| {
            let submit_keys = kb.get_keys("tui.input.submit");
            submit_keys
                .iter()
                .any(|key| key == "shift+enter" || key == "shift+return")
        });
        if !has_shift_enter {
            return false;
        }

        let current_line = &self.state.lines[self.state.cursor_line];
        self.state.cursor_col > 0 && current_line[..self.state.cursor_col].ends_with('\\')
    }

    fn submit_value(&mut self) {
        let result = self
            .expand_paste_markers(&self.get_text())
            .trim()
            .to_string();

        self.state = EditorState::default();
        self.pastes.clear();
        self.paste_counter = 0;
        self.exit_history_browsing();
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.last_action = None;

        if let Some(on_change) = &mut self.on_change {
            on_change("");
        }
        if let Some(on_submit) = &mut self.on_submit {
            on_submit(&result);
        }
    }

    fn handle_backspace(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();

            let line = self.state.lines[self.state.cursor_line].clone();
            let before_cursor = line[..self.state.cursor_col].to_string();
            // Marker-aware segmentation: a paste marker is one atomic segment.
            let segments = self.segment(&before_cursor, "grapheme");
            let last_grapheme = segments.last().map(|(segment, _)| segment.clone());
            let grapheme_length = last_grapheme.as_deref().map_or(1, str::len);

            if let Some(last_grapheme) = &last_grapheme {
                if let Some(captures) = paste_marker_single_regex().captures(last_grapheme) {
                    if let Some(target_id) =
                        captures.get(1).and_then(|m| m.as_str().parse::<i64>().ok())
                    {
                        self.pastes.remove(&target_id);
                        self.paste_counter -= 1;

                        let higher_ids: Vec<i64> = self
                            .pastes
                            .keys()
                            .copied()
                            .filter(|id| *id > target_id)
                            .collect();
                        for id in higher_ids {
                            if let Some(content) = self.pastes.remove(&id) {
                                self.pastes.insert(id - 1, content);
                            }
                        }

                        for line in &mut self.state.lines {
                            *line = renumber_paste_markers(line, target_id);
                        }
                    }
                }
            }

            let line = &self.state.lines[self.state.cursor_line];
            let before = line[..self.state.cursor_col - grapheme_length].to_string();
            let after = line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            self.set_cursor_col(self.state.cursor_col - grapheme_length);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();

            let current_line = self.state.lines[self.state.cursor_line].clone();
            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();

            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);

            self.state.cursor_line -= 1;
            self.set_cursor_col(previous_line.len());
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn set_cursor_col(&mut self, col: usize) {
        self.state.cursor_col = col;
        self.preferred_visual_col = None;
        self.snapped_from_cursor_col = None;
    }

    fn move_to_visual_line(&mut self, visual_lines: &[VisualLine], current: usize, target: usize) {
        let Some(current_vl) = visual_lines.get(current).copied() else {
            return;
        };
        let Some(target_vl) = visual_lines.get(target).copied() else {
            return;
        };

        let current_visual_col = match self.snapped_from_cursor_col {
            Some(snapped) => {
                let vl_index =
                    self.find_visual_line_at(visual_lines, current_vl.logical_line, snapped);
                snapped - visual_lines[vl_index].start_col
            }
            None => self.state.cursor_col - current_vl.start_col,
        };

        let is_last_source_segment = current == visual_lines.len() - 1
            || visual_lines[current + 1].logical_line != current_vl.logical_line;
        let source_max_visual_col = if is_last_source_segment {
            current_vl.length
        } else {
            current_vl.length.saturating_sub(1)
        };

        let is_last_target_segment = target == visual_lines.len() - 1
            || visual_lines
                .get(target + 1)
                .is_none_or(|vl| vl.logical_line != target_vl.logical_line);
        let target_max_visual_col = if is_last_target_segment {
            target_vl.length
        } else {
            target_vl.length.saturating_sub(1)
        };

        let move_to = self.compute_vertical_move_column(
            current_visual_col,
            source_max_visual_col,
            target_max_visual_col,
        );

        self.state.cursor_line = target_vl.logical_line;
        let target_col = target_vl.start_col + move_to;
        let logical_line = &self.state.lines[target_vl.logical_line];
        self.state.cursor_col = target_col.min(logical_line.len());

        // Snap to atomic segment boundaries (paste markers).
        let segments = self.segment(logical_line, "grapheme");
        for (segment, index) in &segments {
            if *index > self.state.cursor_col {
                break;
            }
            if segment.len() <= 1 {
                continue;
            }
            if self.state.cursor_col < *index + segment.len() {
                let is_continuation = *index < target_vl.start_col;
                let is_moving_down = target > current;

                if is_continuation && is_moving_down {
                    let segment_end = *index + segment.len();
                    let mut next = target + 1;
                    while next < visual_lines.len()
                        && visual_lines[next].logical_line == target_vl.logical_line
                        && visual_lines[next].start_col < segment_end
                    {
                        next += 1;
                    }
                    if next < visual_lines.len() {
                        self.move_to_visual_line(visual_lines, current, next);
                        return;
                    }
                }

                self.snapped_from_cursor_col = Some(self.state.cursor_col);
                self.state.cursor_col = *index;
                return;
            }
        }

        self.snapped_from_cursor_col = None;
    }

    fn compute_vertical_move_column(
        &mut self,
        current_visual_col: usize,
        source_max_visual_col: usize,
        target_max_visual_col: usize,
    ) -> usize {
        let has_preferred = self.preferred_visual_col.is_some();
        let cursor_in_middle = current_visual_col < source_max_visual_col;
        let target_too_short = target_max_visual_col < current_visual_col;

        if !has_preferred || cursor_in_middle {
            if target_too_short {
                self.preferred_visual_col = Some(current_visual_col);
                return target_max_visual_col;
            }
            self.preferred_visual_col = None;
            return current_visual_col;
        }

        let preferred = self.preferred_visual_col.unwrap_or(0);
        let target_cant_fit_preferred = target_max_visual_col < preferred;
        if target_too_short || target_cant_fit_preferred {
            return target_max_visual_col;
        }

        self.preferred_visual_col = None;
        preferred
    }

    fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    fn move_to_line_end(&mut self) {
        self.last_action = None;
        let current_line = self.state.lines[self.state.cursor_line].len();
        self.set_cursor_col(current_line);
    }

    fn delete_to_start_of_line(&mut self) {
        self.exit_history_browsing();
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let deleted_text = current_line[..self.state.cursor_col].to_string();
            {
                let accumulate = self.last_action == Some(LastAction::Kill);
                let ring = self.kill_ring_mut();
                ring.push(
                    &deleted_text,
                    KillRingPushOptions {
                        prepend: true,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] =
                current_line[self.state.cursor_col..].to_string();
            self.set_cursor_col(0);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            {
                let accumulate = self.last_action == Some(LastAction::Kill);
                let ring = self.kill_ring_mut();
                ring.push(
                    "\n",
                    KillRingPushOptions {
                        prepend: true,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.set_cursor_col(previous_line.len());
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn delete_to_end_of_line(&mut self) {
        self.exit_history_browsing();
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col < current_line.len() {
            self.push_undo_snapshot();
            let deleted_text = current_line[self.state.cursor_col..].to_string();
            {
                let accumulate = self.last_action == Some(LastAction::Kill);
                let ring = self.kill_ring_mut();
                ring.push(
                    &deleted_text,
                    KillRingPushOptions {
                        prepend: false,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] =
                current_line[..self.state.cursor_col].to_string();
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();
            {
                let accumulate = self.last_action == Some(LastAction::Kill);
                let ring = self.kill_ring_mut();
                ring.push(
                    "\n",
                    KillRingPushOptions {
                        prepend: false,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn delete_word_backwards(&mut self) {
        self.exit_history_browsing();
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.push_undo_snapshot();
                {
                    let accumulate = self.last_action == Some(LastAction::Kill);
                    let ring = self.kill_ring_mut();
                    ring.push(
                        "\n",
                        KillRingPushOptions {
                            prepend: true,
                            accumulate,
                        },
                    );
                }
                self.last_action = Some(LastAction::Kill);
                let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
                self.state.lines[self.state.cursor_line - 1] =
                    format!("{previous_line}{current_line}");
                self.state.lines.remove(self.state.cursor_line);
                self.state.cursor_line -= 1;
                self.set_cursor_col(previous_line.len());
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_backwards();
            let delete_from = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text = current_line[delete_from..self.state.cursor_col].to_string();
            {
                let accumulate = was_kill;
                let ring = self.kill_ring_mut();
                ring.push(
                    &deleted_text,
                    KillRingPushOptions {
                        prepend: true,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                &current_line[..delete_from],
                &current_line[self.state.cursor_col..]
            );
            self.set_cursor_col(delete_from);
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn delete_word_forward(&mut self) {
        self.exit_history_browsing();
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col >= current_line.len() {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.push_undo_snapshot();
                {
                    let accumulate = self.last_action == Some(LastAction::Kill);
                    let ring = self.kill_ring_mut();
                    ring.push(
                        "\n",
                        KillRingPushOptions {
                            prepend: false,
                            accumulate,
                        },
                    );
                }
                self.last_action = Some(LastAction::Kill);
                let next_line = self.state.lines[self.state.cursor_line + 1].clone();
                self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
                self.state.lines.remove(self.state.cursor_line + 1);
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_forwards();
            let delete_to = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text = current_line[self.state.cursor_col..delete_to].to_string();
            {
                let accumulate = was_kill;
                let ring = self.kill_ring_mut();
                ring.push(
                    &deleted_text,
                    KillRingPushOptions {
                        prepend: false,
                        accumulate,
                    },
                );
            }
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                &current_line[..self.state.cursor_col],
                &current_line[delete_to..]
            );
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn handle_forward_delete(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col < current_line.len() {
            self.push_undo_snapshot();
            let after_cursor = &current_line[self.state.cursor_col..];
            let grapheme_length = after_cursor.graphemes(true).next().map_or(1, str::len);
            let before = current_line[..self.state.cursor_col].to_string();
            let after = current_line[self.state.cursor_col + grapheme_length..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();
            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn build_visual_line_map(&self, width: usize) -> Vec<VisualLine> {
        let mut visual_lines: Vec<VisualLine> = Vec::new();

        for (i, line) in self.state.lines.iter().enumerate() {
            let line_vis_width = visible_width(line);
            if line.is_empty() {
                visual_lines.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: 0,
                });
            } else if line_vis_width <= width {
                visual_lines.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: line.len(),
                });
            } else {
                let pre_segmented = self.segment(line, "grapheme");
                for chunk in word_wrap_line(line, width, Some(&pre_segmented)) {
                    visual_lines.push(VisualLine {
                        logical_line: i,
                        start_col: chunk.start_index,
                        length: chunk.end_index - chunk.start_index,
                    });
                }
            }
        }

        visual_lines
    }

    fn find_visual_line_at(&self, visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
        for (i, vl) in visual_lines.iter().enumerate() {
            if vl.logical_line != line {
                continue;
            }
            let offset = col.saturating_sub(vl.start_col);
            let is_last_segment_of_line =
                i == visual_lines.len() - 1 || visual_lines[i + 1].logical_line != vl.logical_line;
            if col >= vl.start_col
                && (offset < vl.length || (is_last_segment_of_line && offset == vl.length))
            {
                return i;
            }
        }
        visual_lines.len().saturating_sub(1)
    }

    fn find_current_visual_line(&self, visual_lines: &[VisualLine]) -> usize {
        self.find_visual_line_at(visual_lines, self.state.cursor_line, self.state.cursor_col)
    }

    fn move_cursor(&mut self, delta_line: i64, delta_col: i64) {
        self.last_action = None;
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);

        if delta_line != 0 {
            let target = current_visual_line as i64 + delta_line;
            if target >= 0 && (target as usize) < visual_lines.len() {
                self.move_to_visual_line(&visual_lines, current_visual_line, target as usize);
            }
        }

        if delta_col != 0 {
            let current_line = self.state.lines[self.state.cursor_line].clone();

            if delta_col > 0 {
                if self.state.cursor_col < current_line.len() {
                    let after_cursor = &current_line[self.state.cursor_col..];
                    let first = after_cursor.graphemes(true).next().map_or(1, str::len);
                    self.set_cursor_col(self.state.cursor_col + first);
                } else if self.state.cursor_line < self.state.lines.len() - 1 {
                    self.state.cursor_line += 1;
                    self.set_cursor_col(0);
                } else if let Some(current_vl) = visual_lines.get(current_visual_line) {
                    self.preferred_visual_col = Some(self.state.cursor_col - current_vl.start_col);
                }
            } else if self.state.cursor_col > 0 {
                let before_cursor = &current_line[..self.state.cursor_col];
                let last = before_cursor
                    .graphemes(true)
                    .next_back()
                    .map_or(1, str::len);
                self.set_cursor_col(self.state.cursor_col - last);
            } else if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let prev_line = self.state.lines[self.state.cursor_line].len();
                self.set_cursor_col(prev_line);
            }
        }
    }

    fn page_scroll(&mut self, direction: i64) {
        self.last_action = None;
        let terminal_rows = (self.terminal_rows)();
        let page_size = (terminal_rows * 3 / 10).max(5);

        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);
        let target = (current_visual_line as i64 + direction * page_size as i64)
            .clamp(0, visual_lines.len() as i64 - 1) as usize;

        self.move_to_visual_line(&visual_lines, current_visual_line, target);
    }

    fn move_word_backwards(&mut self) {
        self.last_action = None;
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let prev_line = self.state.lines[self.state.cursor_line].len();
                self.set_cursor_col(prev_line);
            }
            return;
        }

        let segments = self.segment(&current_line, "word");
        let options = crate::tui::word_navigation::WordNavigationOptions {
            segment: Some(&|_text: &str| {
                segments
                    .iter()
                    .map(|(segment, _)| segment.clone())
                    .collect()
            }),
            is_atomic_segment: Some(&is_paste_marker),
        };
        let cursor = find_word_backward(&current_line, self.state.cursor_col, &options);
        self.set_cursor_col(cursor);
    }

    fn move_word_forwards(&mut self) {
        self.last_action = None;
        let current_line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col >= current_line.len() {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }

        let segments = self.segment(&current_line, "word");
        let options = crate::tui::word_navigation::WordNavigationOptions {
            segment: Some(&|_text: &str| {
                segments
                    .iter()
                    .map(|(segment, _)| segment.clone())
                    .collect()
            }),
            is_atomic_segment: Some(&is_paste_marker),
        };
        let cursor = find_word_forward(&current_line, self.state.cursor_col, &options);
        self.set_cursor_col(cursor);
    }

    fn yank(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        let text = self.kill_ring.peek().cloned().unwrap_or_default();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo_snapshot();
        self.delete_yanked_text();
        self.kill_ring.rotate();
        let text = self.kill_ring.peek().cloned().unwrap_or_default();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn insert_yanked_text(&mut self, text: &str) {
        self.exit_history_browsing();
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();

        if lines.len() == 1 {
            let current_line = &self.state.lines[self.state.cursor_line];
            let before = current_line[..self.state.cursor_col].to_string();
            let after = current_line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{before}{text}{after}");
            self.set_cursor_col(self.state.cursor_col + text.len());
        } else {
            let current_line = &self.state.lines[self.state.cursor_line];
            let before = current_line[..self.state.cursor_col].to_string();
            let after = current_line[self.state.cursor_col..].to_string();

            self.state.lines[self.state.cursor_line] = format!("{before}{}", lines[0]);

            for (i, line) in lines.iter().enumerate().skip(1).take(lines.len() - 2) {
                self.state
                    .lines
                    .insert(self.state.cursor_line + i, line.clone());
            }

            let last_line_index = self.state.cursor_line + lines.len() - 1;
            let last_line = lines[lines.len() - 1].clone();
            self.state
                .lines
                .insert(last_line_index, format!("{last_line}{after}"));

            self.state.cursor_line = last_line_index;
            self.set_cursor_col(last_line.len());
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn delete_yanked_text(&mut self) {
        let Some(yanked_text) = self.kill_ring.peek().cloned() else {
            return;
        };

        let yank_lines: Vec<&str> = yanked_text.split('\n').collect();

        if yank_lines.len() == 1 {
            let current_line = &self.state.lines[self.state.cursor_line];
            let delete_len = yanked_text.len();
            let before = current_line[..self.state.cursor_col - delete_len].to_string();
            let after = current_line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            self.set_cursor_col(self.state.cursor_col - delete_len);
        } else {
            let start_line = self.state.cursor_line - (yank_lines.len() - 1);
            let start_col = self.state.lines[start_line].len() - yank_lines[0].len();
            let after_cursor =
                self.state.lines[self.state.cursor_line][self.state.cursor_col..].to_string();
            let before_yank = self.state.lines[start_line][..start_col].to_string();

            self.state.lines.splice(
                start_line..=self.state.cursor_line,
                vec![format!("{before_yank}{after_cursor}")],
            );

            self.state.cursor_line = start_line;
            self.set_cursor_col(start_col);
        }

        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    /// Kill-ring accessor for the delete operations (upstream field access).
    fn kill_ring_mut(&mut self) -> &mut KillRing {
        &mut self.kill_ring
    }

    /// Upstream `setAutocompleteProvider`.
    pub fn set_autocomplete_provider(
        &mut self,
        provider: Option<Arc<dyn AutocompleteProvider + Send + Sync>>,
    ) {
        self.cancel_autocomplete();
        self.autocomplete_provider = provider.clone();
        let trigger = provider.map(|p| p.trigger_characters()).unwrap_or_default();
        self.set_autocomplete_trigger_characters(trigger);
    }

    fn set_autocomplete_trigger_characters(&mut self, characters: Vec<char>) {
        // Upstream rebuilds the trigger/debounce regexes from the characters;
        // the matchers here read the list directly.
        self.autocomplete_trigger_characters = characters;
    }

    pub fn get_autocomplete_max_visible(&self) -> usize {
        self.autocomplete_max_visible
    }

    pub fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        let new_max = max_visible.clamp(3, 20);
        if self.autocomplete_max_visible != new_max {
            self.autocomplete_max_visible = new_max;
            (self.request_render)();
        }
    }

    fn make_select_list(&self, items: Vec<AutocompleteItem>) -> SelectList {
        let select_items = items
            .iter()
            .map(|item| crate::tui::components::select_list::SelectItem {
                value: item.value.clone(),
                label: item.label.clone(),
                description: item.description.clone(),
            })
            .collect();
        SelectList::new(
            select_items,
            self.autocomplete_max_visible,
            Default::default(),
            crate::tui::components::select_list::SelectListLayoutOptions {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(32),
                ..Default::default()
            },
        )
    }

    /// Test/diagnostic accessor: whether the autocomplete picker is active.
    pub fn has_active_autocomplete(&self) -> bool {
        self.autocomplete_state.is_some() && self.autocomplete_list.is_some()
    }

    /// Upstream `cancelAutocomplete`.
    pub fn cancel_autocomplete(&mut self) {
        self.autocomplete_state = None;
        self.autocomplete_list = None;
        self.autocomplete_prefix = String::new();
    }

    /// Upstream `updateAutocomplete`: query the provider for the cursor
    /// position and refresh (or drop) the picker.
    fn update_autocomplete(&mut self, force: bool) {
        let Some(provider) = &self.autocomplete_provider else {
            return;
        };
        let suggestions: Option<AutocompleteSuggestions> = provider.get_suggestions(
            &self.state.lines,
            self.state.cursor_line,
            self.state.cursor_col,
            force,
        );

        match suggestions {
            Some(suggestions) => {
                // Upstream applyAutocompleteSuggestions: the picker is rebuilt
                // from the suggestion items on every request (no incremental
                // filter); the best-match index defaults to the first item.
                self.autocomplete_prefix = suggestions.prefix.clone();
                self.autocomplete_list = Some(self.make_select_list(suggestions.items));
                self.autocomplete_state = Some(if force {
                    AutocompleteMode::Force
                } else {
                    AutocompleteMode::Regular
                });
            }
            None => self.cancel_autocomplete(),
        }
    }

    /// Upstream `tryTriggerAutocomplete`.
    fn try_trigger_autocomplete(&mut self) {
        self.update_autocomplete(false);
    }

    /// Upstream `handleTabCompletion`: force file/suggestion completion.
    fn handle_tab_completion(&mut self) {
        self.update_autocomplete(true);
    }

    fn is_slash_menu_allowed(&self) -> bool {
        self.state.cursor_line == 0
    }

    fn is_at_start_of_message(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let current_line = &self.state.lines[self.state.cursor_line];
        let before_cursor = current_line[..self.state.cursor_col].trim();
        before_cursor.is_empty() || before_cursor == "/"
    }

    fn is_in_slash_command_context(&self, text_before_cursor: &str) -> bool {
        self.is_slash_menu_allowed() && text_before_cursor.trim_start().starts_with('/')
    }

    /// Upstream `buildTriggerPattern(...).test(text)` for `characters`
    /// (editor.ts:263): `(?:^|(?:\s|cjkPunctuation))[([{<\`]*(?:
    /// @"[^"]* | [trigger](?:(?!\s|cjkPunctuation).)*)$` — implemented as a
    /// matcher because the JS pattern needs lookaround.
    pub(crate) fn autocomplete_trigger_pattern_matches(&self, text: &str) -> bool {
        autocomplete_trigger_pattern_matches(text, &self.autocomplete_trigger_characters)
    }

    /// Upstream Tab/Enter application inside autocomplete mode. Returns
    /// `true` when the applied prefix was a slash command (the caller then
    /// falls through to submit).
    fn apply_selected_completion(&mut self) -> bool {
        let selected_item = self
            .autocomplete_list
            .as_ref()
            .and_then(|list| list.get_selected_item());
        let provider = self.autocomplete_provider.clone();
        let (Some(selected_item), Some(provider)) = (selected_item, provider) else {
            return false;
        };
        // SelectList stores SelectItem; the provider consumes AutocompleteItem.
        let selected = AutocompleteItem {
            value: selected_item.value.clone(),
            label: selected_item.label.clone(),
            description: selected_item.description.clone(),
        };

        self.push_undo_snapshot();
        self.last_action = None;
        let prefix = self.autocomplete_prefix.clone();
        let result = provider.apply_completion(
            &self.state.lines,
            self.state.cursor_line,
            self.state.cursor_col,
            &selected,
            &prefix,
        );
        self.state.lines = result.lines;
        self.state.cursor_line = result.cursor_line;
        self.set_cursor_col(result.cursor_col);
        prefix.starts_with('/')
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(&EditorSnapshot {
            state: self.state.clone(),
            pastes: self.pastes.clone(),
            paste_counter: self.paste_counter,
        });
    }

    fn undo(&mut self) {
        self.exit_history_browsing();
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.state = snapshot.state;
        self.pastes = snapshot.pastes;
        self.paste_counter = snapshot.paste_counter;
        self.last_action = None;
        self.preferred_visual_col = None;
        let text = self.get_text();
        if let Some(on_change) = &mut self.on_change {
            on_change(&text);
        }
    }

    fn jump_to_char(&mut self, ch: char, direction: JumpDirection) {
        self.last_action = None;
        let is_forward = direction == JumpDirection::Forward;
        let line_count = self.state.lines.len() as i64;
        let mut line_index = self.state.cursor_line as i64;
        let end = if is_forward { line_count } else { -1 };
        let step: i64 = if is_forward { 1 } else { -1 };

        while line_index != end {
            let line = &self.state.lines[line_index as usize];
            let is_current_line = line_index == self.state.cursor_line as i64;
            let chars: Vec<char> = line.chars().collect();
            let cursor_char_index = line[..self.cursor_col_in(line_index as usize)]
                .chars()
                .count();

            let search: Vec<char> = if is_current_line {
                if is_forward {
                    if cursor_char_index < chars.len() {
                        chars[cursor_char_index + 1..].to_vec()
                    } else {
                        Vec::new()
                    }
                } else {
                    chars[..cursor_char_index.min(chars.len())].to_vec()
                }
            } else {
                chars.clone()
            };

            let found = if is_forward {
                search.iter().position(|&c| c == ch)
            } else {
                search.iter().rposition(|&c| c == ch)
            };

            if let Some(relative) = found {
                let char_index = if is_forward && is_current_line {
                    cursor_char_index + 1 + relative
                } else {
                    // Backward on the current line: the search covered
                    // [..cursor) so the relative position is absolute.
                    relative
                };
                self.state.cursor_line = line_index as usize;
                let byte_col = line
                    .char_indices()
                    .nth(char_index)
                    .map(|(i, _)| i)
                    .unwrap_or(line.len());
                self.set_cursor_col(byte_col);
                return;
            }

            line_index += step;
        }
    }

    fn cursor_col_in(&self, line_index: usize) -> usize {
        self.state
            .cursor_col
            .min(self.state.lines.get(line_index).map_or(0, |l| l.len()))
    }

    fn handle_input_impl(&mut self, data: &str) {
        // Character jump mode.
        if self.jump_mode.is_some() {
            let jump_forward = with_keybindings(|kb| kb.matches(data, "tui.editor.jumpForward"));
            let jump_backward = with_keybindings(|kb| kb.matches(data, "tui.editor.jumpBackward"));
            if jump_forward || jump_backward {
                self.jump_mode = None;
                return;
            }

            let printable = decode_printable_key(data).or_else(|| {
                data.as_bytes()
                    .first()
                    .filter(|&&b| b >= 32)
                    .map(|_| data.to_string())
            });
            if let Some(printable) = printable {
                let direction = self.jump_mode.take();
                if let Some(direction) = direction {
                    let ch = printable.chars().next().unwrap_or(' ');
                    self.jump_to_char(ch, direction);
                }
                return;
            }

            self.jump_mode = None;
        }

        // Bracketed paste mode.
        if let Some(start_index) = data.find("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            let mut data = data.to_string();
            data.replace_range(start_index..start_index + 6, "");
            self.feed_paste(&data);
            return;
        }

        if self.is_in_paste {
            self.paste_buffer.push_str(data);
            if let Some(end_index) = self.paste_buffer.find("\x1b[201~") {
                let paste_content = self.paste_buffer[..end_index].to_string();
                if !paste_content.is_empty() {
                    self.handle_paste(&paste_content);
                }
                self.is_in_paste = false;
                let remaining = self.paste_buffer[end_index + 6..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input_impl(&remaining);
                }
            }
            return;
        }

        // Ctrl+C - let parent handle.
        if with_keybindings(|kb| kb.matches(data, "tui.input.copy")) {
            return;
        }

        // Undo.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.undo")) {
            self.undo();
            return;
        }

        // Handle autocomplete mode.
        if self.autocomplete_state.is_some() && self.autocomplete_list.is_some() {
            if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
                self.cancel_autocomplete();
                return;
            }

            if with_keybindings(|kb| kb.matches(data, "tui.select.up"))
                || with_keybindings(|kb| kb.matches(data, "tui.select.down"))
            {
                if let Some(list) = &mut self.autocomplete_list {
                    list.handle_input(data);
                }
                return;
            }

            if with_keybindings(|kb| kb.matches(data, "tui.input.tab")) {
                self.apply_selected_completion();
                self.cancel_autocomplete();
                let text = self.get_text();
                if let Some(on_change) = &mut self.on_change {
                    on_change(&text);
                }
                return;
            }

            if with_keybindings(|kb| kb.matches(data, "tui.select.confirm")) {
                let is_slash_prefix = self.autocomplete_prefix.starts_with('/');
                self.apply_selected_completion();
                self.cancel_autocomplete();

                if is_slash_prefix {
                    // Slash command confirm falls through to submit.
                } else {
                    let text = self.get_text();
                    if let Some(on_change) = &mut self.on_change {
                        on_change(&text);
                    }
                    return;
                }
            }
        }

        // Tab - trigger completion.
        if self.autocomplete_state.is_none()
            && with_keybindings(|kb| kb.matches(data, "tui.input.tab"))
        {
            self.handle_tab_completion();
            return;
        }

        // Deletion actions.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteToLineEnd")) {
            self.delete_to_end_of_line();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteToLineStart")) {
            self.delete_to_start_of_line();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteWordBackward")) {
            self.delete_word_backwards();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteWordForward")) {
            self.delete_word_forward();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteCharBackward"))
            || matches_key(data, "shift+backspace")
        {
            self.handle_backspace();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteCharForward"))
            || matches_key(data, "shift+delete")
        {
            self.handle_forward_delete();
            return;
        }

        // Kill ring actions.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.yank")) {
            self.yank();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.yankPop")) {
            self.yank_pop();
            return;
        }

        // Dedicated history actions.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.historyPrevious")) {
            self.navigate_history(-1);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.historyNext")) {
            self.navigate_history(1);
            return;
        }

        // Cursor movement.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLineStart")) {
            self.move_to_line_start();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLineEnd")) {
            self.move_to_line_end();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorWordLeft")) {
            self.move_word_backwards();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorWordRight")) {
            self.move_word_forwards();
            return;
        }

        // New line.
        let first_byte = data.as_bytes().first().copied();
        let new_line = with_keybindings(|kb| kb.matches(data, "tui.input.newLine"))
            || (first_byte == Some(10) && data.len() > 1)
            || data == "\x1b\r"
            || data == "\x1b[13;2~"
            || (data.len() > 1 && data.contains('\x1b') && data.contains('\r'))
            || (data == "\n" && data.len() == 1);
        if new_line {
            if self.should_submit_on_backslash_enter(data) {
                self.handle_backspace();
                self.submit_value();
                return;
            }
            self.add_new_line();
            return;
        }

        // Submit (Enter).
        if with_keybindings(|kb| kb.matches(data, "tui.input.submit")) {
            if self.disable_submit {
                return;
            }

            let current_line = self.state.lines[self.state.cursor_line].clone();
            let char_before_is_backslash =
                self.state.cursor_col > 0 && current_line[..self.state.cursor_col].ends_with('\\');
            if char_before_is_backslash {
                self.handle_backspace();
                self.add_new_line();
                return;
            }

            self.submit_value();
            return;
        }

        // Arrow key navigation.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorUp")) {
            if self.is_on_first_visual_line()
                && (self.is_editor_empty() || self.history_index > -1 || self.state.cursor_col == 0)
            {
                self.navigate_history(-1);
            } else if self.is_on_first_visual_line() {
                self.move_to_line_start();
            } else {
                self.move_cursor(-1, 0);
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorDown")) {
            if self.history_index > -1 && self.is_on_last_visual_line() {
                self.navigate_history(1);
            } else if self.is_on_last_visual_line() {
                self.move_to_line_end();
            } else {
                self.move_cursor(1, 0);
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorRight")) {
            self.move_cursor(0, 1);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLeft")) {
            self.move_cursor(0, -1);
            return;
        }

        // Page up/down.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.pageUp")) {
            self.page_scroll(-1);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.pageDown")) {
            self.page_scroll(1);
            return;
        }

        // Character jump triggers.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.jumpForward")) {
            self.jump_mode = Some(JumpDirection::Forward);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.jumpBackward")) {
            self.jump_mode = Some(JumpDirection::Backward);
            return;
        }

        // Shift+Space.
        if matches_key(data, "shift+space") {
            self.insert_character(" ", false);
            return;
        }

        if let Some(printable) = decode_printable_key(data) {
            self.insert_character(&printable, false);
            return;
        }

        // Regular characters.
        if data.as_bytes().first().is_some_and(|&b| b >= 32) {
            self.insert_character(data, false);
        }
    }

    fn feed_paste(&mut self, data: &str) {
        self.paste_buffer.push_str(data);
        if let Some(end_index) = self.paste_buffer.find("\x1b[201~") {
            let paste_content = self.paste_buffer[..end_index].to_string();
            if !paste_content.is_empty() {
                self.handle_paste(&paste_content);
            }
            self.is_in_paste = false;
            let remaining = self.paste_buffer[end_index + 6..].to_string();
            self.paste_buffer.clear();
            if !remaining.is_empty() {
                self.handle_input_impl(&remaining);
            }
        }
    }

    pub fn get_value(&self) -> String {
        self.get_text()
    }

    pub fn set_value(&mut self, text: &str) {
        self.set_text(text);
    }
}

/// Upstream `normalizeText`: \r\n and \r become \n, tabs expand to 4 spaces.
fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
}

/// Upstream paste-marker renumbering: shift marker ids above `target_id`
/// down by one, keeping earlier ids untouched.
fn renumber_paste_markers(line: &str, target_id: i64) -> String {
    paste_marker_regex()
        .replace_all(line, |captures: &regex::Captures| {
            let full = captures.get(0).map(|m| m.as_str()).unwrap_or_default();
            let id: i64 = captures
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            if id <= target_id {
                return full.to_string();
            }
            let suffix = captures
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let _ = suffix;
            format!("[paste #{}{}]", id - 1, suffix)
        })
        .into_owned()
}

fn decode_pasted_csi_u(pasted_text: &str) -> String {
    let re = csi_u_ctrl_regex();
    re.replace_all(pasted_text, |captures: &regex::Captures| {
        let code: u32 = captures
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        if (97..=122).contains(&code) {
            return char::from_u32(code - 96).unwrap_or_default().to_string();
        }
        if (65..=90).contains(&code) {
            return char::from_u32(code - 64).unwrap_or_default().to_string();
        }
        captures
            .get(0)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default()
    })
    .into_owned()
}

fn csi_u_ctrl_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\x1b\[(\d+);5u").unwrap())
}

fn split_word_segments(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut byte_offset = 0usize;
    for segment in text.split_word_bounds() {
        out.push((segment.to_string(), byte_offset));
        byte_offset += segment.len();
    }
    out
}

impl Component for Editor {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_impl(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_impl(data)
    }

    fn invalidate(&mut self) {}

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}
