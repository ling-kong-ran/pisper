//! Port of upstream `packages/tui/src/components/select-list.ts`: a scrollable
//! list of selectable items with a primary/description two-column layout.
//!
//! Disclosed substitutions: the JS `onSelect`/`onCancel`/`onSelectionChange`
//! properties become `Option<Box<dyn FnMut>>` fields; the theme is a struct of
//! boxed painter functions with an identity default.

use crate::tui::component::{
    Component, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType,
};
use crate::tui::keybindings::with_keybindings;
use crate::tui::utils::{truncate_to_width, visible_width};

const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;

/// Upstream `SelectItem`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Theme painter callbacks (upstream `SelectListTheme`).
pub type ThemePainter = Box<dyn Fn(&str) -> String + Send + Sync>;

/// Upstream `SelectListTheme`.
pub struct SelectListTheme {
    pub selected_prefix: ThemePainter,
    pub selected_text: ThemePainter,
    pub description: ThemePainter,
    pub scroll_info: ThemePainter,
    pub no_match: ThemePainter,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: Box::new(|text| text.to_string()),
            selected_text: Box::new(|text| text.to_string()),
            description: Box::new(|text| text.to_string()),
            scroll_info: Box::new(|text| text.to_string()),
            no_match: Box::new(|text| text.to_string()),
        }
    }
}

/// Upstream `SelectListLayoutOptions`.
#[derive(Default)]
pub struct SelectListLayoutOptions {
    pub min_primary_column_width: Option<usize>,
    pub max_primary_column_width: Option<usize>,
    /// Upstream `truncatePrimary`: custom primary-cell truncation.
    pub truncate_primary: Option<crate::tui::components::truncate_primary::TruncatePrimaryFn>,
}

/// Select callback (upstream `onSelect`).
pub type SelectCallback = Box<dyn FnMut(&SelectItem) + Send>;
/// Cancel callback (upstream `onCancel`).
pub type CancelCallback = Box<dyn FnMut() + Send>;
/// Selection-change callback (upstream `onSelectionChange`).
pub type SelectionChangeCallback = Box<dyn FnMut(&SelectItem) + Send>;

/// Upstream `SelectList` callbacks.
#[derive(Default)]
pub struct SelectListCallbacks {
    pub on_select: Option<SelectCallback>,
    pub on_cancel: Option<CancelCallback>,
    pub on_selection_change: Option<SelectionChangeCallback>,
}

fn normalize_to_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for c in text.chars() {
        if c == '\r' || c == '\n' {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = c == ' ';
        }
    }
    out.trim().to_string()
}

/// Upstream `SelectList`.
pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_items: Vec<SelectItem>,
    selected_index: usize,
    mouse_pressed_index: Option<usize>,
    max_visible: usize,
    theme: SelectListTheme,
    layout: SelectListLayoutOptions,
    callbacks: SelectListCallbacks,
}

impl SelectList {
    pub fn new(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: SelectListLayoutOptions,
    ) -> Self {
        Self {
            filtered_items: items.clone(),
            items,
            max_visible,
            theme,
            layout,
            selected_index: 0,
            mouse_pressed_index: None,
            callbacks: SelectListCallbacks::default(),
        }
    }

    pub fn with_callbacks(mut self, callbacks: SelectListCallbacks) -> Self {
        self.callbacks = callbacks;
        self
    }

    pub fn set_filter(&mut self, filter: &str) {
        let lower = filter.to_lowercase();
        self.filtered_items = self
            .items
            .iter()
            .filter(|item| item.value.to_lowercase().starts_with(&lower))
            .cloned()
            .collect();
        // Reset selection when the filter changes.
        self.selected_index = 0;
    }

    pub fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index.min(self.filtered_items.len().saturating_sub(1));
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn filtered_len(&self) -> usize {
        self.filtered_items.len()
    }

    fn get_visible_range(&self) -> (usize, usize) {
        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(self.filtered_items.len().saturating_sub(self.max_visible));
        (
            start_index,
            (start_index + self.max_visible).min(self.filtered_items.len()),
        )
    }

    fn get_display_value(item: &SelectItem) -> &str {
        if item.label.is_empty() {
            &item.value
        } else {
            &item.label
        }
    }

    fn get_primary_column_width(&self) -> usize {
        let (raw_min, raw_max) = match (
            self.layout.min_primary_column_width,
            self.layout.max_primary_column_width,
        ) {
            (Some(min), Some(max)) => (min, max),
            (Some(v), None) | (None, Some(v)) => (v, v),
            (None, None) => (DEFAULT_PRIMARY_COLUMN_WIDTH, DEFAULT_PRIMARY_COLUMN_WIDTH),
        };
        let min = 1.max(raw_min.min(raw_max));
        let max = 1.max(raw_min.max(raw_max));

        let widest_primary = self
            .filtered_items
            .iter()
            .map(|item| visible_width(Self::get_display_value(item)) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);

        widest_primary.clamp(min, max)
    }

    fn truncate_primary(
        &self,
        item: &SelectItem,
        is_selected: bool,
        max_width: usize,
        column_width: usize,
    ) -> String {
        let display_value = Self::get_display_value(item);
        let truncated_value = match &self.layout.truncate_primary {
            Some(truncate) => truncate(
                &crate::tui::components::truncate_primary::TruncatePrimaryContext {
                    text: display_value,
                    max_width,
                    column_width,
                    item,
                    is_selected,
                },
            ),
            None => truncate_to_width(display_value, max_width, "", false),
        };
        truncate_to_width(&truncated_value, max_width, "", false)
    }

    fn render_item(
        &self,
        item: &SelectItem,
        is_selected: bool,
        width: usize,
        description_single_line: Option<&str>,
        primary_column_width: usize,
    ) -> String {
        let prefix = if is_selected { "\u{2192} " } else { "  " };
        let prefix_width = visible_width(prefix);

        if let Some(description) = description_single_line {
            if width > 40 {
                let effective_primary_column_width =
                    1.max(primary_column_width.min(width.saturating_sub(prefix_width + 4)));
                let max_primary_width =
                    (effective_primary_column_width - PRIMARY_COLUMN_GAP).max(1);
                let truncated_value = self.truncate_primary(
                    item,
                    is_selected,
                    max_primary_width,
                    effective_primary_column_width,
                );
                let truncated_value_width = visible_width(&truncated_value);
                let spacing = " ".repeat(
                    effective_primary_column_width
                        .saturating_sub(truncated_value_width)
                        .max(1),
                );
                let description_start = prefix_width + truncated_value_width + spacing.len();
                let remaining_width = width.saturating_sub(description_start + 2); // -2 for safety

                if remaining_width > MIN_DESCRIPTION_WIDTH {
                    let truncated_desc = truncate_to_width(description, remaining_width, "", false);
                    if is_selected {
                        return (self.theme.selected_text)(&format!(
                            "{prefix}{truncated_value}{spacing}{truncated_desc}"
                        ));
                    }
                    let desc_text = (self.theme.description)(&format!("{spacing}{truncated_desc}"));
                    return format!("{prefix}{truncated_value}{desc_text}");
                }
            }
        }

        let max_width = width.saturating_sub(prefix_width + 2);
        let truncated_value = self.truncate_primary(item, is_selected, max_width, max_width);
        if is_selected {
            return (self.theme.selected_text)(&format!("{prefix}{truncated_value}"));
        }
        format!("{prefix}{truncated_value}")
    }

    fn notify_selection_change(&mut self) {
        if let Some(selected) = self.filtered_items.get(self.selected_index).cloned() {
            if let Some(callback) = &mut self.callbacks.on_selection_change {
                callback(&selected);
            }
        }
    }
}

impl Component for SelectList {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        // No matching items: show a message.
        if self.filtered_items.is_empty() {
            lines.push((self.theme.no_match)("  No matching commands"));
            return lines;
        }

        let primary_column_width = self.get_primary_column_width();
        let (start_index, end_index) = self.get_visible_range();

        for i in start_index..end_index {
            let Some(item) = self.filtered_items.get(i) else {
                continue;
            };
            let is_selected = i == self.selected_index;
            let description_single_line = item.description.as_deref().map(normalize_to_single_line);
            lines.push(self.render_item(
                item,
                is_selected,
                width,
                description_single_line.as_deref(),
                primary_column_width,
            ));
        }

        // Scroll indicators.
        if start_index > 0 || end_index < self.filtered_items.len() {
            let scroll_text = format!(
                "  ({}/{})",
                self.selected_index + 1,
                self.filtered_items.len()
            );
            let truncated = truncate_to_width(&scroll_text, width.saturating_sub(2), "", false);
            lines.push((self.theme.scroll_info)(&truncated));
        }

        lines
    }

    fn handle_mouse(&mut self, event: &TuiMouseEvent) -> Option<TuiMouseEventResult> {
        if self.filtered_items.is_empty() {
            return None;
        }
        if event.event_type == TuiMouseEventType::Wheel {
            let wheel_delta = event.wheel_delta?;
            // Upstream checks JS truthiness: zero and NaN must not select.
            if wheel_delta == 0.0 || wheel_delta.is_nan() {
                return None;
            }
            let delta = if wheel_delta < 0.0 { -1i64 } else { 1 };
            let previous_index = self.selected_index as i64;
            let next =
                (previous_index + delta).clamp(0, self.filtered_items.len() as i64 - 1) as usize;
            self.selected_index = next;
            let changed = self.selected_index != previous_index as usize;
            if changed {
                self.notify_selection_change();
            }
            return Some(TuiMouseEventResult {
                handled: true,
                render: Some(changed),
                ..Default::default()
            });
        }

        // Hover must not change selection.
        if event.button != TuiMouseButton::Left
            || (event.event_type != TuiMouseEventType::Press
                && event.event_type != TuiMouseEventType::Click)
        {
            return None;
        }
        let (start_index, end_index) = self.get_visible_range();
        let Ok(event_y) = usize::try_from(event.y) else {
            return None;
        };
        let item_index = start_index + event_y;
        if item_index < start_index || item_index >= end_index {
            return None;
        }

        if event.event_type == TuiMouseEventType::Press {
            self.mouse_pressed_index = Some(item_index);
            if self.selected_index != item_index {
                self.selected_index = item_index;
                self.notify_selection_change();
            }
            return Some(TuiMouseEventResult {
                handled: true,
                focus: true,
                ..Default::default()
            });
        }
        if event.event_type == TuiMouseEventType::Click {
            let clicked_index = self.mouse_pressed_index.unwrap_or(item_index);
            self.mouse_pressed_index = None;
            let changed = self.selected_index != clicked_index;
            self.selected_index = clicked_index;
            if changed {
                self.notify_selection_change();
            }
            let selected_item = self.filtered_items.get(self.selected_index).cloned();
            if let Some(item) = selected_item {
                if let Some(callback) = &mut self.callbacks.on_select {
                    callback(&item);
                }
            }
            return Some(TuiMouseEventResult {
                handled: true,
                ..Default::default()
            });
        }
        None
    }

    fn handle_input(&mut self, key_data: &str) {
        if with_keybindings(|kb| kb.matches(key_data, "tui.select.up")) {
            // Wrap to bottom when at top.
            self.selected_index = if self.selected_index == 0 {
                self.filtered_items.len().saturating_sub(1)
            } else {
                self.selected_index - 1
            };
            self.notify_selection_change();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.down")) {
            // Wrap to top when at bottom.
            self.selected_index = if !self.filtered_items.is_empty()
                && self.selected_index == self.filtered_items.len() - 1
            {
                0
            } else {
                (self.selected_index + 1).min(self.filtered_items.len().saturating_sub(1))
            };
            self.notify_selection_change();
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.confirm")) {
            if let Some(selected) = self.filtered_items.get(self.selected_index).cloned() {
                if let Some(callback) = &mut self.callbacks.on_select {
                    callback(&selected);
                }
            }
        } else if with_keybindings(|kb| kb.matches(key_data, "tui.select.cancel")) {
            if let Some(callback) = &mut self.callbacks.on_cancel {
                callback();
            }
        }
    }

    fn invalidate(&mut self) {}
}

impl SelectList {
    /// Test/layout helper: set the primary column bounds.
    pub fn set_layout_min_max(&mut self, min: Option<usize>, max: Option<usize>) {
        self.layout.min_primary_column_width = min;
        self.layout.max_primary_column_width = max;
    }

    /// Test/layout helper: install a custom primary truncation.
    pub fn set_truncate_primary(
        &mut self,
        truncate: impl Fn(&crate::tui::components::truncate_primary::TruncatePrimaryContext<'_>) -> String
            + Send
            + Sync
            + 'static,
    ) {
        self.layout.truncate_primary = Some(Box::new(truncate));
    }

    /// The selected item, if any (upstream `getSelectedItem`).
    pub fn get_selected_item(&self) -> Option<SelectItem> {
        self.filtered_items.get(self.selected_index).cloned()
    }
}
