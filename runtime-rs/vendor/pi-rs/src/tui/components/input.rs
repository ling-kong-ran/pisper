//! Port of upstream `packages/tui/src/components/input.ts`: a single-line
//! text input with horizontal scrolling, kill ring, undo, bracketed paste,
//! and Kitty CSI-u printable decoding.
//!
//! Disclosed substitutions: the cursor and value lengths are byte offsets
//! (JS: UTF-16 code units) — identical for ASCII inputs; the mouse handler's
//! `grapheme.index` becomes the grapheme's byte offset. Callbacks are
//! `Option<Box<...>>` fields instead of JS properties.

use crate::tui::component::{
    Component, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType, CURSOR_MARKER,
};
use crate::tui::keybindings::with_keybindings;
use crate::tui::keys::decode_kitty_printable;
use crate::tui::undo_stack::UndoStack;
use crate::tui::utils::{is_whitespace_char, slice_by_column, truncate_to_width, visible_width};
use crate::tui::word_navigation::{
    find_word_backward, find_word_forward, KillRing, KillRingPushOptions,
};

use unicode_segmentation::UnicodeSegmentation;

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
struct InputState {
    value: String,
    cursor: usize,
}

/// Styled text painter (upstream `placeholderStyle`).
pub type TextStyleFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
/// Submit callback (upstream `onSubmit`).
pub type SubmitFn = Box<dyn FnMut(&str) + Send>;
/// Escape callback (upstream `onEscape`).
pub type EscapeFn = Box<dyn FnMut() + Send>;

/// Upstream `InputOptions`.
#[derive(Clone, Default)]
pub struct InputOptions {
    pub prompt: Option<String>,
    pub placeholder: Option<String>,
    pub placeholder_style: Option<TextStyleFn>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

/// Upstream `Input`: single-line text input with horizontal scrolling.
pub struct Input {
    value: String,
    cursor: usize,
    prompt: String,
    placeholder: String,
    placeholder_style: Arc<dyn Fn(&str) -> String + Send + Sync>,
    rendered_start_column: usize,
    focused: bool,
    on_submit: Option<SubmitFn>,
    on_escape: Option<EscapeFn>,

    paste_buffer: String,
    is_in_paste: bool,

    kill_ring: Mutex<KillRing>,
    last_action: Option<LastAction>,

    undo_stack: UndoStack<InputState>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new(InputOptions::default())
    }
}

impl Input {
    pub fn new(options: InputOptions) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            prompt: options.prompt.unwrap_or_else(|| "> ".to_string()),
            placeholder: options.placeholder.unwrap_or_default(),
            placeholder_style: options
                .placeholder_style
                .unwrap_or_else(|| Arc::new(|text| text.to_string())),
            rendered_start_column: 0,
            focused: false,
            on_submit: None,
            on_escape: None,
            paste_buffer: String::new(),
            is_in_paste: false,
            kill_ring: Mutex::new(KillRing::default()),
            last_action: None,
            undo_stack: UndoStack::new(),
        }
    }

    pub fn on_submit(&mut self, callback: impl FnMut(&str) + Send + 'static) {
        self.on_submit = Some(Box::new(callback));
    }

    pub fn on_escape(&mut self, callback: impl FnMut() + Send + 'static) {
        self.on_escape = Some(Box::new(callback));
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor = self.cursor.min(self.value.len());
    }

    /// Cursor as a byte offset (JS: UTF-16 index).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(&InputState {
            value: self.value.clone(),
            cursor: self.cursor,
        });
    }

    pub fn handle_input(&mut self, data: &str) {
        // Bracketed paste mode: \x1b[200~ ... \x1b[201~.
        if let Some(start_index) = data.find("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            let mut data = data.to_string();
            data.replace_range(start_index..start_index + 6, "");
            self.feed_paste(&data);
            return;
        }

        if self.is_in_paste {
            self.feed_paste(data);
            return;
        }

        // Escape/Cancel.
        if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
            if let Some(callback) = &mut self.on_escape {
                callback();
            }
            return;
        }

        // Undo.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.undo")) {
            self.undo();
            return;
        }

        // Submit.
        if with_keybindings(|kb| kb.matches(data, "tui.input.submit")) || data == "\n" {
            if let Some(callback) = &mut self.on_submit {
                let value = self.value.clone();
                callback(&value);
            }
            return;
        }

        // Deletion.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteCharBackward")) {
            self.handle_backspace();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteCharForward")) {
            self.handle_forward_delete();
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
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteToLineStart")) {
            self.delete_to_line_start();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.editor.deleteToLineEnd")) {
            self.delete_to_line_end();
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

        // Cursor movement.
        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLeft")) {
            self.last_action = None;
            if self.cursor > 0 {
                let before_cursor = &self.value[..self.cursor];
                let last_grapheme = before_cursor.graphemes(true).next_back();
                self.cursor -= last_grapheme.map_or(1, |g| g.len());
            }
            return;
        }

        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorRight")) {
            self.last_action = None;
            if self.cursor < self.value.len() {
                let after_cursor = &self.value[self.cursor..];
                let first_grapheme = after_cursor.graphemes(true).next();
                self.cursor += first_grapheme.map_or(1, |g| g.len());
            }
            return;
        }

        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLineStart")) {
            self.last_action = None;
            self.cursor = 0;
            return;
        }

        if with_keybindings(|kb| kb.matches(data, "tui.editor.cursorLineEnd")) {
            self.last_action = None;
            self.cursor = self.value.len();
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

        // Kitty CSI-u printable character (protocol flag 1 sends CSI-u for all
        // keys); decoded before the control-char check since CSI-u contains ESC.
        if let Some(kitty_printable) = decode_kitty_printable(data) {
            self.insert_character(&kitty_printable);
            return;
        }

        // Regular character input: printable characters including Unicode,
        // rejecting control characters (C0, DEL, C1).
        let has_control_chars = data.chars().any(|c| {
            let code = c as u32;
            code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
        });
        if !has_control_chars {
            let text = data.to_string();
            self.insert_character(&text);
        }
    }

    fn handle_mouse_impl(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if event.event_type != TuiMouseEventType::Press
            || event.button != crate::tui::component::TuiMouseButton::Left
            || event.y != 0
        {
            return None;
        }
        let visible_column = event.x.saturating_sub(2).max(0) as usize;
        let target_column = self.rendered_start_column + visible_column;
        let mut current_column = 0usize;
        self.cursor = self.value.len();
        for (byte_offset, grapheme) in self.value.grapheme_indices(true) {
            let next_column = current_column + visible_width(grapheme);
            if target_column < next_column {
                self.cursor = byte_offset;
                break;
            }
            current_column = next_column;
        }
        self.last_action = None;
        Some(TuiMouseEventResult {
            handled: true,
            focus: true,
            ..Default::default()
        })
    }

    fn insert_character(&mut self, text: &str) {
        // Undo coalescing: consecutive word chars coalesce into one unit.
        let first_char_is_whitespace = text.chars().next().is_some_and(is_whitespace_char);
        if first_char_is_whitespace || self.last_action != Some(LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = Some(LastAction::TypeWord);

        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn handle_backspace(&mut self) {
        self.last_action = None;
        if self.cursor > 0 {
            self.push_undo();
            let before_cursor = &self.value[..self.cursor];
            let grapheme_length = before_cursor
                .graphemes(true)
                .next_back()
                .map_or(1, |g| g.len());
            let start = self.cursor - grapheme_length;
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    fn handle_forward_delete(&mut self) {
        self.last_action = None;
        if self.cursor < self.value.len() {
            self.push_undo();
            let after_cursor = &self.value[self.cursor..];
            let grapheme_length = after_cursor.graphemes(true).next().map_or(1, |g| g.len());
            self.value
                .replace_range(self.cursor..self.cursor + grapheme_length, "");
        }
    }

    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted_text = self.value[..self.cursor].to_string();
        {
            let mut ring = self.kill_ring.lock().expect("kill ring");
            ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: true,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
        }
        self.last_action = Some(LastAction::Kill);
        self.value = self.value[self.cursor..].to_string();
        self.cursor = 0;
    }

    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted_text = self.value[self.cursor..].to_string();
        {
            let mut ring = self.kill_ring.lock().expect("kill ring");
            ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: false,
                    accumulate: self.last_action == Some(LastAction::Kill),
                },
            );
        }
        self.last_action = Some(LastAction::Kill);
        self.value.truncate(self.cursor);
    }

    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }

        // Save lastAction before cursor movement (moveWordBackwards resets it).
        let was_kill = self.last_action == Some(LastAction::Kill);

        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_backwards();
        let delete_from = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = self.value[delete_from..self.cursor].to_string();
        {
            let mut ring = self.kill_ring.lock().expect("kill ring");
            ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: true,
                    accumulate: was_kill,
                },
            );
        }
        self.last_action = Some(LastAction::Kill);

        self.value.replace_range(delete_from..self.cursor, "");
        self.cursor = delete_from;
    }

    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }

        let was_kill = self.last_action == Some(LastAction::Kill);

        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_forwards();
        let delete_to = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = self.value[self.cursor..delete_to].to_string();
        {
            let mut ring = self.kill_ring.lock().expect("kill ring");
            ring.push(
                &deleted_text,
                KillRingPushOptions {
                    prepend: false,
                    accumulate: was_kill,
                },
            );
        }
        self.last_action = Some(LastAction::Kill);

        self.value.replace_range(self.cursor..delete_to, "");
    }

    fn yank(&mut self) {
        let text = {
            let ring = self.kill_ring.lock().expect("kill ring");
            ring.peek().cloned()
        };
        let Some(text) = text else {
            return;
        };

        self.push_undo();

        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) {
            return;
        }
        let ring_length = self.kill_ring.lock().expect("kill ring").len();
        if ring_length <= 1 {
            return;
        }

        self.push_undo();

        // Delete the previously yanked text (still at end of ring before rotation).
        let prev_text = {
            let ring = self.kill_ring.lock().expect("kill ring");
            ring.peek().cloned().unwrap_or_default()
        };
        let start = self.cursor - prev_text.len();
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;

        // Rotate and insert the new entry.
        self.kill_ring.lock().expect("kill ring").rotate();
        let text = {
            let ring = self.kill_ring.lock().expect("kill ring");
            ring.peek().cloned().unwrap_or_default()
        };
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.value = snapshot.value;
        self.cursor = snapshot.cursor;
        self.last_action = None;
    }

    fn move_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = None;
        let options = crate::tui::word_navigation::WordNavigationOptions::default();
        self.cursor = find_word_backward(&self.value, self.cursor, &options);
    }

    fn move_word_forwards(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = None;
        let options = crate::tui::word_navigation::WordNavigationOptions::default();
        self.cursor = find_word_forward(&self.value, self.cursor, &options);
    }

    fn feed_paste(&mut self, data: &str) {
        self.paste_buffer.push_str(data);

        let Some(end_index) = self.paste_buffer.find("\x1b[201~") else {
            return;
        };
        let paste_content = self.paste_buffer[..end_index].to_string();
        self.handle_paste(&paste_content);

        self.is_in_paste = false;
        let remaining = self.paste_buffer[end_index + "\x1b[201~".len()..].to_string();
        self.paste_buffer.clear();
        if !remaining.is_empty() {
            self.handle_input(&remaining);
        }
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.last_action = None;
        self.push_undo();

        // Clean the pasted text: remove newlines and expand tabs.
        let clean_text = pasted_text
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");

        self.value.insert_str(self.cursor, &clean_text);
        self.cursor += clean_text.len();
    }

    fn render_impl(&mut self, width: usize) -> Vec<String> {
        // Visible window calculation.
        let available_width = width.saturating_sub(visible_width(&self.prompt));

        if available_width == 0 {
            return vec![truncate_to_width(&self.prompt, width, "", false)];
        }

        if self.value.is_empty() && !self.placeholder.is_empty() {
            let placeholder = truncate_to_width(&self.placeholder, available_width, "", false);
            let at_cursor = placeholder
                .graphemes(true)
                .next()
                .unwrap_or(" ")
                .to_string();
            let after_cursor = placeholder[at_cursor.len()..].to_string();
            let marker = if self.focused { CURSOR_MARKER } else { "" };
            let styled = (self.placeholder_style)(&at_cursor);
            let cursor_char = format!("\x1b[7m{styled}\x1b[27m");
            let text_with_cursor = format!(
                "{marker}{cursor_char}{}",
                (self.placeholder_style)(&after_cursor)
            );
            let padding =
                " ".repeat(available_width.saturating_sub(visible_width(&text_with_cursor)));
            return vec![format!("{}{text_with_cursor}{padding}", self.prompt)];
        }

        let mut visible_text = String::new();
        let mut cursor_display = self.cursor;
        self.rendered_start_column = 0;
        let total_width = visible_width(&self.value);

        if total_width < available_width {
            // Everything fits (leave room for the cursor at the end).
            visible_text = self.value.clone();
        } else {
            // Horizontal scrolling; reserve one column for an end cursor.
            let scroll_width = if self.cursor == self.value.len() {
                available_width - 1
            } else {
                available_width
            };
            let cursor_col = visible_width(&self.value[..self.cursor]);

            if scroll_width > 0 {
                let half_width = scroll_width / 2;
                let start_col = if cursor_col < half_width {
                    0
                } else if cursor_col > total_width - half_width {
                    total_width - scroll_width
                } else {
                    cursor_col - half_width
                };

                self.rendered_start_column = start_col;
                visible_text = slice_by_column(&self.value, start_col, scroll_width, true);
                let before = slice_by_column(
                    &self.value,
                    start_col,
                    cursor_col.saturating_sub(start_col),
                    true,
                );
                cursor_display = before.len();
            }
        }

        // Build the line with the fake cursor at the cursor position.
        let at_cursor_start = cursor_display.min(visible_text.len());
        let after_slice = &visible_text[at_cursor_start..];
        let cursor_grapheme = after_slice.graphemes(true).next().map(str::to_string);

        let before_cursor = visible_text[..at_cursor_start].to_string();
        let at_cursor = cursor_grapheme.unwrap_or_else(|| " ".to_string());
        // JS slice() clamps out-of-range starts; mirror that.
        let after_start = (at_cursor_start + at_cursor.len()).min(visible_text.len());
        let after_cursor = visible_text[after_start..].to_string();

        let marker = if self.focused { CURSOR_MARKER } else { "" };
        let cursor_char = format!("\x1b[7m{at_cursor}\x1b[27m");
        let text_with_cursor = format!("{before_cursor}{marker}{cursor_char}{after_cursor}");

        let visual_length = visible_width(&text_with_cursor);
        let padding = " ".repeat(available_width.saturating_sub(visual_length));
        let line = format!("{}{text_with_cursor}{padding}", self.prompt);

        vec![line]
    }
}

impl Component for Input {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_impl(width)
    }

    fn handle_input(&mut self, data: &str) {
        Input::handle_input(self, data)
    }

    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        self.handle_mouse_impl(event)
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
