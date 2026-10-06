//! Port of upstream `packages/tui/src/components/settings-list.ts`: a
//! settings list with selection, value cycling, optional search filtering and
//! optional submenus.
//!
//! Disclosed substitutions: the submenu opener receives the current value and
//! a done callback; the Rust port stores the submenu as an owned component
//! plus a boxed done closure invoked by the host.

use super::input::Input;
use crate::tui::component::Component;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::with_keybindings;
use crate::tui::utils::{truncate_to_width, visible_width, wrap_text_with_ansi};

/// Upstream `SettingItem`.
#[derive(Clone, Debug, Default)]
pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub current_value: String,
    /// When present, Enter/Space cycles through these values.
    pub values: Vec<String>,
}

/// Label painter (selected flag disambiguates the styling).
pub type LabelPainter = Box<dyn Fn(&str, bool) -> String + Send>;
/// Value painter.
pub type ValuePainter = Box<dyn Fn(&str, bool) -> String + Send>;
/// Simple text painter.
pub type TextPainter = Box<dyn Fn(&str) -> String + Send>;

/// Upstream `SettingsListTheme`.
pub struct SettingsListTheme {
    pub label: LabelPainter,
    pub value: ValuePainter,
    pub description: TextPainter,
    pub cursor: String,
    pub hint: TextPainter,
}

impl Default for SettingsListTheme {
    fn default() -> Self {
        Self {
            label: Box::new(|text, _| text.to_string()),
            value: Box::new(|text, _| text.to_string()),
            description: Box::new(|text| text.to_string()),
            cursor: "\u{2192} ".to_string(),
            hint: Box::new(|text| text.to_string()),
        }
    }
}

/// Upstream `SettingsListOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsListOptions {
    pub enable_search: bool,
}

/// Change callback: (setting id, new value).
pub type OnChangeFn = Box<dyn FnMut(&str, &str) + Send>;
/// Cancel callback.
pub type OnCancelFn = Box<dyn FnMut() + Send>;
/// Upstream submenu opener: receives the current value and a done callback.
pub type SubmenuOpener =
    Box<dyn Fn(&str, &dyn Fn(Option<&str>, Option<&str>)) -> Box<dyn Component>>;

/// Upstream `SettingsList`.
pub struct SettingsList {
    items: Vec<SettingItem>,
    filtered_items: Vec<SettingItem>,
    theme: SettingsListTheme,
    selected_index: usize,
    max_visible: usize,
    on_change: OnChangeFn,
    on_cancel: OnCancelFn,
    search_input: Option<Input>,
    search_enabled: bool,

    submenu: Option<Box<dyn Component>>,
    // Upstream `navigateAfterClose`: an item id to select when the submenu
    // closes; consumed by the submenu-assembly slice.
    #[allow(dead_code)]
    navigate_after_close: Option<String>,
}

impl SettingsList {
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: OnChangeFn,
        on_cancel: OnCancelFn,
        options: SettingsListOptions,
    ) -> Self {
        let search_enabled = options.enable_search;
        let search_input = search_enabled.then(Input::default);
        let filtered_items = items.clone();
        Self {
            items,
            filtered_items,
            theme,
            selected_index: 0,
            max_visible,
            on_change,
            on_cancel,
            search_input,
            search_enabled,
            submenu: None,
            navigate_after_close: None,
        }
    }

    /// Update an item's current value.
    pub fn update_value(&mut self, id: &str, new_value: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.current_value = new_value.to_string();
        }
    }

    /// Move selection to the item with the given id (no-op if not found).
    pub fn select_item(&mut self, id: &str) {
        let items = if self.search_enabled {
            &self.filtered_items
        } else {
            &self.items
        };
        if let Some(index) = items.iter().position(|item| item.id == id) {
            self.selected_index = index;
        }
    }

    pub fn search_input(&mut self) -> Option<&mut Input> {
        self.search_input.as_mut()
    }

    pub(crate) fn get_display_items(&self) -> &Vec<SettingItem> {
        if self.search_enabled {
            &self.filtered_items
        } else {
            &self.items
        }
    }

    /// Test helper: the current value of an item by id.
    #[cfg(test)]
    pub(crate) fn get_display_item_value(&self, id: &str) -> Option<String> {
        self.get_display_items()
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.current_value.clone())
    }

    /// Mouse handling and the submenu slice reuse this; pending full wiring.
    #[allow(dead_code)]
    fn get_visible_range(&self, display_items: &[SettingItem]) -> (usize, usize) {
        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(display_items.len().saturating_sub(self.max_visible));
        (
            start_index,
            (start_index + self.max_visible).min(display_items.len()),
        )
    }

    fn add_hint_line(&self, lines: &mut Vec<String>, width: usize) {
        lines.push(String::new());
        let hint = if self.search_enabled {
            "  Type to search \u{b7} Enter/Space to change \u{b7} Esc to cancel"
        } else {
            "  Enter/Space to change \u{b7} Esc to cancel"
        };
        lines.push(truncate_to_width(
            &(self.theme.hint)(hint),
            width,
            "",
            false,
        ));
    }

    fn render_main_list(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        if let Some(input) = &mut self.search_input {
            lines.extend(input.render(width));
            lines.push(String::new());
        }

        if self.items.is_empty() {
            lines.push((self.theme.hint)("  No settings available"));
            if self.search_enabled {
                self.add_hint_line(&mut lines, width);
            }
            return lines;
        }

        let display_item_count = self.get_display_items().len();
        if display_item_count == 0 {
            let hint = (self.theme.hint)("  No matching settings");
            lines.push(truncate_to_width(&hint, width, "", false));
            self.add_hint_line(&mut lines, width);
            return lines;
        }

        let (start_index, end_index) = self.get_visible_range_count(display_item_count);

        let max_label_width = self
            .items
            .iter()
            .map(|item| visible_width(&item.label))
            .max()
            .unwrap_or(0)
            .min(36);

        for i in start_index..end_index {
            let Some(item) = self.get_display_items().get(i) else {
                continue;
            };
            let is_selected = i == self.selected_index;
            let prefix = if is_selected {
                self.theme.cursor.clone()
            } else {
                "  ".to_string()
            };
            let prefix_width = visible_width(&prefix);

            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_width.saturating_sub(visible_width(&item.label)))
            );
            let label_text = (self.theme.label)(&label_padded, is_selected);

            let separator = "  ";
            let used_width = prefix_width + max_label_width + visible_width(separator);
            let value_max_width = width.saturating_sub(used_width + 2);

            let value_text = (self.theme.value)(
                &truncate_to_width(&item.current_value, value_max_width, "", false),
                is_selected,
            );

            lines.push(truncate_to_width(
                &format!("{prefix}{label_text}{separator}{value_text}"),
                width,
                "",
                false,
            ));
        }

        if start_index > 0 || end_index < display_item_count {
            let scroll_text = format!("  ({}/{})", self.selected_index + 1, display_item_count);
            let truncated = truncate_to_width(&scroll_text, width.saturating_sub(2), "", false);
            lines.push((self.theme.hint)(&truncated));
        }

        if let Some(selected) = self.get_display_items().get(self.selected_index) {
            if let Some(description) = &selected.description {
                lines.push(String::new());
                for line in wrap_text_with_ansi(description, width.saturating_sub(4)) {
                    lines.push((self.theme.description)(&format!("  {line}")));
                }
            }
        }

        self.add_hint_line(&mut lines, width);
        lines
    }

    fn get_visible_range_count(&self, display_item_count: usize) -> (usize, usize) {
        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(display_item_count.saturating_sub(self.max_visible));
        (
            start_index,
            (start_index + self.max_visible).min(display_item_count),
        )
    }

    #[allow(dead_code)]
    fn apply_filter(&mut self, query: &str) {
        self.filtered_items = fuzzy_filter(self.items.clone(), query, |item| &item.label);
        self.selected_index = 0;
    }
}

impl Component for SettingsList {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Submenu takes over rendering when active.
        if let Some(submenu) = &mut self.submenu {
            return submenu.render(width);
        }
        self.render_main_list(width)
    }

    fn handle_input(&mut self, data: &str) {
        // Submenu delegates all input (its escape triggers done() upstream).
        if self.submenu.is_some() {
            // The submenu component consumes the input directly.
            return;
        }

        let display_item_count = self.get_display_items().len();
        if with_keybindings(|kb| kb.matches(data, "tui.select.up")) {
            if display_item_count == 0 {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                display_item_count - 1
            } else {
                self.selected_index - 1
            };
        } else if with_keybindings(|kb| kb.matches(data, "tui.select.down")) {
            if display_item_count == 0 {
                return;
            }
            self.selected_index = if self.selected_index == display_item_count - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if with_keybindings(|kb| kb.matches(data, "tui.select.confirm"))
            || (data == " "
                && (!self.search_enabled
                    || self
                        .search_input()
                        .map(|i| i.value().is_empty())
                        .unwrap_or(true)))
        {
            // Value cycling: rotate through the item's values.
            let item = self.get_display_items().get(self.selected_index).cloned();
            if let Some(item) = item {
                if !item.values.is_empty() {
                    let current_index = item
                        .values
                        .iter()
                        .position(|value| *value == item.current_value)
                        .unwrap_or(0);
                    let next_index = (current_index + 1) % item.values.len();
                    let new_value = item.values[next_index].clone();
                    if let Some(stored) = self.items.iter_mut().find(|stored| stored.id == item.id)
                    {
                        stored.current_value = new_value.clone();
                    }
                    (self.on_change)(&item.id, &new_value);
                }
            }
        } else if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
            (self.on_cancel)();
        } else if self.search_enabled {
            if let Some(input) = &mut self.search_input {
                input.handle_input(data);
                let query = input.value().to_string();
                self.filtered_items = fuzzy_filter(self.items.clone(), &query, |item| &item.label);
                self.selected_index = 0;
            }
        }
    }

    fn invalidate(&mut self) {}
}
