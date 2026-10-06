//! The interactive UI glue mirrors upstream callback signatures whose types
//! are inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/settings-selector.ts` (945 lines,
//! sha256 `97f30cbd3eb6bf965781cb554d6b94d4364f7dc338c72dbff991bf9ab45268af`).
//!
//! Slice conventions: see [`super::model_selector`] (theme seam, inline
//! composite rendering). Additional disclosed substitutions:
//! - **SettingsList submenu slice**: the vendored `pi-rust`
//!   `tui::components::settings_list` predates the submenu wiring
//!   (`SettingItem` carries no `submenu` opener and active submenus are not
//!   delegated input), so this module ports the settings-list behavior it
//!   needs — including the upstream open/delegate/done submenu flow — as
//!   [`SelectorList`], driven by the full upstream item table.
//! - **SelectSubmenu / SteppedSubmenu** (`settings-submenu.ts`) are ported as
//!   [`SelectSubmenu`] / [`SteppedSubmenu`]. The upstream
//!   `done(selectedValue?, {navigateTo?})` callback and the select-list
//!   `onSelect`/`onCancel` closures fire synchronously during input handling
//!   and then mutate the owning component; Rust ownership forbids those
//!   back-references, so the submenus *record* the outcome
//!   ([`SubmenuComponent::take_done`], [`SelectSubmenu::take_selection`],
//!   [`SelectSubmenu::take_cancelled`]) and the owner applies it immediately
//!   after delegating the same input event — the observable sequence is
//!   identical.
//! - **`localeCompare`** maps to plain lexicographic comparison (repo-wide
//!   convention for the deterministic core).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::ai::models::get_supported_thinking_levels;
use crate::ai::types::Model;
use crate::coding_agent::core::http_dispatcher::{
    format_http_idle_timeout_ms, HTTP_IDLE_TIMEOUT_CHOICES,
};
use crate::coding_agent::core::settings_manager::QuietStartup;
use crate::coding_agent::modes::interactive::system_theme::SYSTEM_THEME_NAME;
use crate::coding_agent::modes::interactive::theme::{parse_auto_theme_setting, Theme};
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::select_list::{
    SelectItem, SelectList, SelectListCallbacks, SelectListLayoutOptions, SelectListTheme,
};
use crate::tui::components::text::Text;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::with_keybindings;
use crate::tui::terminal_image::get_capabilities;
use crate::tui::utils::{truncate_to_width, visible_width, wrap_text_with_ansi};

use super::model_selector::{key_display_text, spacer_lines, theme_fg, DynamicBorder};

const MODEL_PICKER_MIN_PRIMARY_COLUMN_WIDTH: usize = 12;
const MODEL_PICKER_MAX_PRIMARY_COLUMN_WIDTH: usize = 46;
const SUBMENU_MIN_PRIMARY_COLUMN_WIDTH: usize = 12;
const SUBMENU_MAX_PRIMARY_COLUMN_WIDTH: usize = 32;

const THINKING_DESCRIPTIONS: [(&str, &str); 7] = [
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning (~1k tokens)"),
    ("low", "Light reasoning (~2k tokens)"),
    ("medium", "Moderate reasoning (~8k tokens)"),
    ("high", "Deep reasoning (~16k tokens)"),
    ("xhigh", "Extra-high reasoning (~32k tokens)"),
    ("max", "Maximum reasoning"),
];

const DEFAULT_PROJECT_TRUST_LABELS: [(&str, &str); 3] = [
    ("ask", "Ask"),
    ("always", "Always trust"),
    ("never", "Never trust"),
];

const CLEAR_OVERRIDE_VALUE: &str = "__clear__";
const AUTOMATIC_THEME_VALUE: &str = "/";

/// Upstream `CACHE_WARMING_MODES` (settings-manager.ts).
const CACHE_WARMING_MODES: [&str; 3] = ["off", "streaming", "idle"];

/// The fullscreen wheel-scroll-lines choice list (upstream builds it from
/// `["auto", ...new Set([1, 2, 3, 5, 10, current])]` — `"auto"` first, then
/// the base set plus the current numeric value, deduped, ascending).
fn wheel_scroll_line_values(current: &str) -> Vec<String> {
    let mut lines: Vec<u64> = vec![1, 2, 3, 5, 10];
    if let Ok(parsed) = current.parse::<u64>() {
        if !lines.contains(&parsed) {
            lines.push(parsed);
        }
    }
    lines.sort_unstable();
    let mut values = vec!["auto".to_string()];
    values.extend(lines.iter().map(|line| line.to_string()));
    values
}

fn model_setting_key(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn model_display_label(model: &Model) -> String {
    format!("{} [{}]", model.id, model.provider)
}

fn model_thinking_overrides_summary(overrides: &[(String, String)]) -> String {
    if overrides.is_empty() {
        "none".to_string()
    } else {
        format!("{} configured", overrides.len())
    }
}

#[allow(dead_code)] // wired into the interactive shell in r19+
fn model_item_label(theme: &Theme, model: &Model) -> String {
    format!(
        "{} {}",
        model.id,
        theme_fg(theme, "muted", &format!("[{}]", model.provider))
    )
}

pub(crate) fn theme_items(available_themes: &[String], current_theme: &str) -> Vec<SelectItem> {
    available_themes
        .iter()
        .map(|name| SelectItem {
            value: name.clone(),
            label: format!("{}{name}", if name == current_theme { "✓ " } else { "  " }),
            description: if name == SYSTEM_THEME_NAME {
                Some("Theme created from your terminal's colors".to_string())
            } else {
                None
            },
        })
        .collect()
}

/// The system theme comes first, then automatic mode, then the remaining
/// themes.
pub(crate) fn single_mode_theme_items(
    available_themes: &[String],
    current_theme: &str,
) -> Vec<SelectItem> {
    let mut items = theme_items(available_themes, current_theme);
    let system_index = items
        .iter()
        .position(|item| item.value == SYSTEM_THEME_NAME);
    let system = system_index.map(|index| items.remove(index));
    let mut out = Vec::new();
    out.extend(system);
    out.push(SelectItem {
        value: AUTOMATIC_THEME_VALUE.to_string(),
        label: "  automatic".to_string(),
        description: Some("Use separate themes for light and dark terminal appearance".to_string()),
    });
    out.extend(items);
    out
}

fn preferred_theme(available_themes: &[String], preferred: Option<&str>, fallback: &str) -> String {
    if let Some(preferred) = preferred {
        if available_themes.iter().any(|theme| theme == preferred) {
            return preferred.to_string();
        }
    }
    if available_themes.iter().any(|theme| theme == fallback) {
        return fallback.to_string();
    }
    available_themes
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn default_automatic_themes(
    current_theme_setting: &str,
    available_themes: &[String],
) -> (String, String) {
    if let Some(auto_theme) = parse_auto_theme_setting(Some(current_theme_setting)) {
        return auto_theme;
    }
    let current_fixed_theme = if current_theme_setting.contains('/') {
        None
    } else {
        Some(current_theme_setting)
    };
    let theme_name = preferred_theme(available_themes, current_fixed_theme, SYSTEM_THEME_NAME);
    (theme_name.clone(), theme_name)
}

/// Upstream `TerminalTheme` ("light" | "dark").
pub type TerminalThemeKind = String;

// ===========================================================================
// Settings config / callbacks
// ===========================================================================

/// Upstream `WarningSettings` (settings-manager.ts shape used here).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WarningSettings {
    pub anthropic_extra_usage: Option<bool>,
}

/// Upstream `SettingsConfig` (the fields this component reads).
#[allow(clippy::struct_excessive_bools)]
pub struct SettingsConfig {
    pub auto_compact: bool,
    pub default_model: String,
    pub current_model: Option<Model>,
    pub available_default_models: Vec<Model>,
    pub show_images: bool,
    pub image_width_cells: u32,
    pub auto_resize_images: bool,
    pub block_images: bool,
    pub enable_skill_commands: bool,
    pub steering_mode: String,
    pub follow_up_mode: String,
    pub transport: String,
    pub http_idle_timeout_ms: u64,
    /// Upstream `CacheWarmingMode` (`"off" | "streaming" | "idle"`).
    pub cache_warming_mode: String,
    pub thinking_level: String,
    pub available_thinking_levels: Vec<String>,
    pub model_thinking_levels: Vec<(String, String)>,
    pub current_theme: String,
    pub terminal_theme: TerminalThemeKind,
    pub available_themes: Vec<String>,
    pub hide_thinking_block: bool,
    pub mermaid_rendering_mode: String,
    pub show_cache_miss_notices: bool,
    pub collapse_changelog: bool,
    pub enable_install_telemetry: bool,
    pub double_escape_action: String,
    pub tree_filter_mode: String,
    pub show_hardware_cursor: bool,
    pub editor_padding_x: u32,
    pub output_pad: u8,
    pub autocomplete_max_visible: u32,
    pub quiet_startup: QuietStartup,
    pub default_project_trust: String,
    pub clear_on_shrink: bool,
    pub show_terminal_progress: bool,
    pub tui_mode: String,
    pub fullscreen_exit_output: String,
    pub fullscreen_scrollbar: String,
    pub fullscreen_copy_on_select: bool,
    /// Upstream `WheelScrollLines` (`"auto"` or a line count).
    pub fullscreen_wheel_scroll_lines: String,
    pub warnings: WarningSettings,
}

/// Upstream `SettingsCallbacks` (`on*` properties).
#[allow(clippy::type_complexity)]
pub struct SettingsCallbacks {
    pub on_auto_compact_change: Box<dyn FnMut(bool) + Send>,
    pub on_show_images_change: Box<dyn FnMut(bool) + Send>,
    pub on_image_width_cells_change: Box<dyn FnMut(u32) + Send>,
    pub on_auto_resize_images_change: Box<dyn FnMut(bool) + Send>,
    pub on_block_images_change: Box<dyn FnMut(bool) + Send>,
    pub on_enable_skill_commands_change: Box<dyn FnMut(bool) + Send>,
    pub on_steering_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_follow_up_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_transport_change: Box<dyn FnMut(&str) + Send>,
    pub on_http_idle_timeout_ms_change: Box<dyn FnMut(u64) + Send>,
    pub on_cache_warming_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_model_thinking_level_change: Box<dyn FnMut(&str, &str, &str) + Send>,
    pub on_model_thinking_level_remove: Box<dyn FnMut(&str, &str) + Send>,
    pub on_theme_change: Box<dyn FnMut(&str) + Send>,
    pub on_theme_preview: Option<Box<dyn FnMut(&str) + Send>>,
    pub on_hide_thinking_block_change: Box<dyn FnMut(bool) + Send>,
    pub on_mermaid_rendering_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_show_cache_miss_notices_change: Box<dyn FnMut(bool) + Send>,
    pub on_collapse_changelog_change: Box<dyn FnMut(bool) + Send>,
    pub on_enable_install_telemetry_change: Box<dyn FnMut(bool) + Send>,
    pub on_double_escape_action_change: Box<dyn FnMut(&str) + Send>,
    pub on_tree_filter_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_show_hardware_cursor_change: Box<dyn FnMut(bool) + Send>,
    pub on_editor_padding_x_change: Box<dyn FnMut(u32) + Send>,
    pub on_output_pad_change: Box<dyn FnMut(u8) + Send>,
    pub on_autocomplete_max_visible_change: Box<dyn FnMut(u32) + Send>,
    pub on_quiet_startup_change: Box<dyn FnMut(QuietStartup) + Send>,
    pub on_default_project_trust_change: Box<dyn FnMut(&str) + Send>,
    pub on_clear_on_shrink_change: Box<dyn FnMut(bool) + Send>,
    pub on_show_terminal_progress_change: Box<dyn FnMut(bool) + Send>,
    pub on_tui_mode_change: Box<dyn FnMut(&str) + Send>,
    pub on_fullscreen_exit_output_change: Box<dyn FnMut(&str) + Send>,
    pub on_fullscreen_scrollbar_change: Box<dyn FnMut(&str) + Send>,
    pub on_fullscreen_copy_on_select_change: Box<dyn FnMut(bool) + Send>,
    pub on_fullscreen_wheel_scroll_lines_change: Box<dyn FnMut(&str) + Send>,
    pub on_warnings_change: Box<dyn FnMut(WarningSettings) + Send>,
    pub on_cancel: Box<dyn FnMut() + Send>,
}

fn bool_str(value: bool) -> String {
    if value {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

// ===========================================================================
// SelectorList (settings-list.ts port with the submenu slice)
// ===========================================================================

/// Upstream `SettingsListTheme` shape (painters from the theme).
pub struct SettingsSelectorListTheme {
    pub label: Box<dyn Fn(&str, bool) -> String + Send>,
    pub value: Box<dyn Fn(&str, bool) -> String + Send>,
    pub description: Box<dyn Fn(&str) -> String + Send>,
    pub cursor: String,
    pub hint: Box<dyn Fn(&str) -> String + Send>,
}

/// Upstream `getSettingsListTheme`.
pub fn get_settings_list_theme(theme: &Theme) -> SettingsSelectorListTheme {
    let theme = Arc::new(theme.clone());
    SettingsSelectorListTheme {
        label: {
            let theme = Arc::clone(&theme);
            Box::new(move |text: &str, selected: bool| {
                if selected {
                    theme_fg(&theme, "accent", text)
                } else {
                    text.to_string()
                }
            })
        },
        value: {
            let theme = Arc::clone(&theme);
            Box::new(move |text: &str, selected: bool| {
                if selected {
                    theme_fg(&theme, "accent", text)
                } else {
                    theme_fg(&theme, "muted", text)
                }
            })
        },
        description: {
            let theme = Arc::clone(&theme);
            Box::new(move |text: &str| theme_fg(&theme, "dim", text))
        },
        cursor: theme_fg(&theme, "accent", "→ "),
        hint: {
            let theme = Arc::clone(&theme);
            Box::new(move |text: &str| theme_fg(&theme, "dim", text))
        },
    }
}

/// `done(selectedValue?, {navigateTo?})` payload drained after delegation.
#[derive(Default)]
pub struct SubmenuDone {
    pub selected_value: Option<String>,
    pub navigate_to: Option<String>,
}

/// A component the settings list can delegate to while it is open.
pub trait SubmenuComponent {
    fn render(&mut self, width: usize) -> Vec<String>;
    fn handle_input(&mut self, data: &str);
    fn take_done(&mut self) -> Option<SubmenuDone> {
        None
    }
    fn invalidate(&mut self) {}
}

/// Upstream `SettingItem` plus the submenu opener.
pub struct SelectorItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub current_value: String,
    pub values: Vec<String>,
    pub submenu: Option<Box<dyn Fn(&str) -> Box<dyn SubmenuComponent>>>,
}

/// Upstream settings-list (`SettingsList`) with per-item submenu support.
pub struct SelectorList {
    items: Vec<SelectorItem>,
    filtered_items: Vec<SelectorItem>,
    theme: SettingsSelectorListTheme,
    selected_index: usize,
    max_visible: usize,
    on_change: Box<dyn FnMut(&str, &str)>,
    on_cancel: Box<dyn FnMut()>,
    search_input: Option<Input>,
    search_enabled: bool,
    submenu: Option<Box<dyn SubmenuComponent>>,
    navigate_after_close: Option<String>,
    submenu_item_index: Option<usize>,
}

impl SelectorList {
    pub fn new(
        items: Vec<SelectorItem>,
        max_visible: usize,
        theme: SettingsSelectorListTheme,
        on_change: Box<dyn FnMut(&str, &str)>,
        on_cancel: Box<dyn FnMut()>,
        enable_search: bool,
    ) -> Self {
        let search_enabled = enable_search;
        let filtered_items = items
            .iter()
            .map(|item| SelectorItem {
                id: item.id.clone(),
                label: item.label.clone(),
                description: item.description.clone(),
                current_value: item.current_value.clone(),
                values: item.values.clone(),
                submenu: None,
            })
            .collect();
        Self {
            items,
            filtered_items,
            theme,
            selected_index: 0,
            max_visible,
            on_change,
            on_cancel,
            search_input: search_enabled.then(Input::default),
            search_enabled,
            submenu: None,
            navigate_after_close: None,
            submenu_item_index: None,
        }
    }

    /// Upstream `selectItem`.
    pub fn select_item(&mut self, id: &str) {
        let items = self.get_display_items();
        if let Some(index) = items.iter().position(|item| item.id == id) {
            self.selected_index = index;
        }
    }

    fn get_display_items(&self) -> &Vec<SelectorItem> {
        if self.search_enabled {
            &self.filtered_items
        } else {
            &self.items
        }
    }

    fn get_visible_range(&self, display_count: usize) -> (usize, usize) {
        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(display_count.saturating_sub(self.max_visible));
        (
            start_index,
            (start_index + self.max_visible).min(display_count),
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

        let (start_index, end_index) = self.get_visible_range(display_item_count);

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

        let selected_description = self
            .get_display_items()
            .get(self.selected_index)
            .and_then(|item| item.description.clone());
        if let Some(description) = &selected_description {
            lines.push(String::new());
            for line in wrap_text_with_ansi(description, width.saturating_sub(4)) {
                lines.push((self.theme.description)(&format!("  {line}")));
            }
        }

        self.add_hint_line(&mut lines, width);
        lines
    }

    fn activate_item(&mut self) {
        let index = self.selected_index;
        let entry = self.get_display_items().get(index);
        let Some(entry) = entry else {
            return;
        };
        let current_value = entry.current_value.clone();
        let values = entry.values.clone();
        let id = entry.id.clone();
        let has_submenu = entry.submenu.is_some();

        if has_submenu {
            // Open submenu, passing current value so it can pre-select correctly
            self.submenu_item_index = Some(self.selected_index);
            let opener = self
                .get_display_items()
                .get(index)
                .and_then(|entry| entry.submenu.as_ref())
                .expect("checked above");
            let opened = opener(&current_value);
            self.submenu = Some(opened);
        } else if !values.is_empty() {
            let current_index = values
                .iter()
                .position(|value| *value == current_value)
                .unwrap_or(0);
            let next_index = (current_index + 1) % values.len();
            let new_value = values[next_index].clone();
            if let Some(stored) = self.items.iter_mut().find(|stored| stored.id == id) {
                stored.current_value = new_value.clone();
            }
            (self.on_change)(&id, &new_value);
        }
    }

    fn close_submenu(&mut self) {
        self.submenu = None;
        if let Some(id) = self.navigate_after_close.take() {
            self.submenu_item_index = None;
            self.select_item(&id);
            self.activate_item();
        } else if let Some(index) = self.submenu_item_index.take() {
            self.selected_index = index;
        }
    }

    fn apply_filter(&mut self, query: &str) {
        let paired: Vec<(SelectorItem, String)> = self
            .items
            .iter()
            .map(|item| {
                let label = item.label.clone();
                (
                    SelectorItem {
                        id: item.id.clone(),
                        label: label.clone(),
                        description: item.description.clone(),
                        current_value: item.current_value.clone(),
                        values: item.values.clone(),
                        submenu: None,
                    },
                    label,
                )
            })
            .collect();
        self.filtered_items = fuzzy_filter(paired, query, |pair: &(SelectorItem, String)| {
            pair.1.as_str()
        })
        .into_iter()
        .map(|(item, _)| item)
        .collect();
        self.selected_index = 0;
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, data: &str) {
        // If submenu is active, delegate all input to it, then drain done.
        if self.submenu.is_some() {
            {
                let submenu = self.submenu.as_mut().expect("checked above");
                submenu.handle_input(data);
            }
            let done = self
                .submenu
                .as_mut()
                .and_then(|submenu| submenu.take_done());
            if let Some(done) = done {
                if let Some(selected_value) = &done.selected_value {
                    let id = self
                        .submenu_item_index
                        .and_then(|index| self.get_display_items().get(index))
                        .map(|item| item.id.clone())
                        .unwrap_or_default();
                    if let Some(stored) = self.items.iter_mut().find(|stored| stored.id == id) {
                        stored.current_value = selected_value.clone();
                    }
                    (self.on_change)(&id, selected_value);
                }
                self.navigate_after_close = done.navigate_to;
                self.close_submenu();
            }
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
                        .search_input
                        .as_ref()
                        .map(|input| input.value().is_empty())
                        .unwrap_or(true)))
        {
            self.activate_item();
        } else if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
            (self.on_cancel)();
        } else if self.search_enabled {
            if let Some(input) = &mut self.search_input {
                input.handle_input(data);
                let query = input.value().to_string();
                self.apply_filter(&query);
            }
        }
    }
}

impl Component for SelectorList {
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(submenu) = self.submenu.as_mut() {
            return submenu.render(width);
        }
        self.render_main_list(width)
    }

    fn handle_input(&mut self, data: &str) {
        SelectorList::handle_input(self, data)
    }

    fn invalidate(&mut self) {}
}
// ===========================================================================
// SelectSubmenu (settings-submenu.ts)
// ===========================================================================

enum SubmenuChild {
    Text(Text),
    Spacer(usize),
}

/// Upstream `getSelectListTheme`.
fn get_select_list_theme(theme: &Theme) -> SelectListTheme {
    let theme = Arc::new(theme.clone());
    SelectListTheme {
        selected_prefix: {
            let theme = Arc::clone(&theme);
            Box::new(move |text| theme_fg(&theme, "accent", text))
        },
        selected_text: {
            let theme = Arc::clone(&theme);
            Box::new(move |text| theme_fg(&theme, "accent", text))
        },
        description: {
            let theme = Arc::clone(&theme);
            Box::new(move |text| theme_fg(&theme, "muted", text))
        },
        scroll_info: {
            let theme = Arc::clone(&theme);
            Box::new(move |text| theme_fg(&theme, "muted", text))
        },
        no_match: {
            let theme = Arc::clone(&theme);
            Box::new(move |text| theme_fg(&theme, "muted", text))
        },
    }
}

type SelectionRx = RefCell<std::sync::mpsc::Receiver<String>>;
type CancelFlag = Arc<std::sync::atomic::AtomicBool>;

fn selection_channel() -> (std::sync::mpsc::Sender<String>, SelectionRx) {
    let (tx, rx) = std::sync::mpsc::channel();
    (tx, RefCell::new(rx))
}

fn cancel_flag() -> CancelFlag {
    Arc::new(std::sync::atomic::AtomicBool::new(false))
}

fn build_select_list(
    theme: &Theme,
    options: Vec<SelectItem>,
    layout: (usize, usize),
    preselect: &str,
    selection_tx: std::sync::mpsc::Sender<String>,
    cancel_flag: CancelFlag,
    selection_change_tx: Option<std::sync::mpsc::Sender<String>>,
) -> SelectList {
    let options_count = options.len();
    let preselect_index = options.iter().position(|option| option.value == preselect);
    let mut list = SelectList::new(
        options,
        options_count.min(10),
        get_select_list_theme(theme),
        SelectListLayoutOptions {
            min_primary_column_width: Some(layout.0),
            max_primary_column_width: Some(layout.1),
            truncate_primary: None,
        },
    )
    .with_callbacks(SelectListCallbacks {
        // The select list records the outcome; the owner drains it right
        // after delegating the same input event (see module docs).
        on_select: Some(Box::new(move |item: &SelectItem| {
            let _ = selection_tx.send(item.value.clone());
        })),
        on_cancel: Some(Box::new(move || {
            cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })),
        on_selection_change: selection_change_tx.map(|tx| {
            Box::new(move |item: &SelectItem| {
                let _ = tx.send(item.value.clone());
            }) as Box<dyn FnMut(&SelectItem) + Send>
        }),
    });

    if let Some(index) = preselect_index {
        list.set_selected_index(index);
    }
    list
}

/// Upstream `SelectSubmenu`.
pub struct SelectSubmenu {
    theme: Arc<Theme>,
    children: Vec<SubmenuChild>,
    select_list: SelectList,
    select_options: Vec<SelectItem>,
    list_layout: (usize, usize),
    search_input: Option<Input>,
    on_select: Option<Box<dyn FnMut(&str)>>,
    on_cancel: Option<Box<dyn FnMut()>>,
    on_selection_change: Option<Box<dyn FnMut(&str)>>,
    selection_rx: SelectionRx,
    cancel_flag: CancelFlag,
    selection_change_rx: Option<SelectionRx>,
}

impl SelectSubmenu {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        title: &str,
        description: &str,
        options: Vec<SelectItem>,
        current_value: &str,
        on_select: Box<dyn FnMut(&str)>,
        on_cancel: Box<dyn FnMut()>,
        on_selection_change: Option<Box<dyn FnMut(&str)>>,
        searchable: bool,
        layout: Option<(usize, usize)>,
    ) -> Self {
        let list_layout = layout.unwrap_or((
            SUBMENU_MIN_PRIMARY_COLUMN_WIDTH,
            SUBMENU_MAX_PRIMARY_COLUMN_WIDTH,
        ));
        let mut children: Vec<SubmenuChild> = Vec::new();
        // Title
        children.push(SubmenuChild::Text(Text::with_options(
            &theme.bold(&theme_fg(&theme, "accent", title)),
            0,
            0,
            None,
        )));
        // Description
        if !description.is_empty() {
            children.push(SubmenuChild::Spacer(1));
            children.push(SubmenuChild::Text(Text::with_options(
                &theme_fg(&theme, "muted", description),
                0,
                0,
                None,
            )));
        }
        // Search input
        let mut search_input = None;
        if searchable {
            children.push(SubmenuChild::Spacer(1));
            search_input = Some(Input::new(InputOptions::default()));
        }
        // Spacer
        children.push(SubmenuChild::Spacer(1));

        let (selection_tx, selection_rx) = selection_channel();
        let cancel_flag = cancel_flag();
        let (change_tx, change_rx) = selection_channel();

        let select_list = build_select_list(
            &theme,
            options.clone(),
            list_layout,
            current_value,
            selection_tx,
            Arc::clone(&cancel_flag),
            Some(change_tx),
        );

        // Hint
        children.push(SubmenuChild::Spacer(1));
        let hint = if searchable {
            "  Type to filter \u{b7} Enter to select \u{b7} Esc to go back"
        } else {
            "  Enter to select \u{b7} Esc to go back"
        };
        children.push(SubmenuChild::Text(Text::with_options(
            &theme_fg(&theme, "dim", hint),
            0,
            0,
            None,
        )));

        Self {
            theme,
            children,
            select_list,
            select_options: options,
            list_layout,
            search_input,
            on_select: Some(on_select),
            on_cancel: Some(on_cancel),
            on_selection_change: on_selection_change
                .map(|callback| Box::new(callback) as Box<dyn FnMut(&str)>),
            selection_rx,
            cancel_flag,
            selection_change_rx: Some(change_rx),
        }
    }

    fn rebuild_list(&mut self, options: Vec<SelectItem>) {
        let (selection_tx, selection_rx) = selection_channel();
        let (change_tx, change_rx) = selection_channel();
        self.select_list = build_select_list(
            &self.theme,
            options.clone(),
            self.list_layout,
            "",
            selection_tx,
            Arc::clone(&self.cancel_flag),
            Some(change_tx),
        );
        self.selection_rx = selection_rx;
        self.selection_change_rx = Some(change_rx);
        self.select_options = options;
    }

    fn apply_filter(&mut self, query: &str) {
        let filtered = if !query.is_empty() {
            let paired: Vec<(SelectItem, String)> = self
                .select_options
                .iter()
                .map(|item| {
                    let text = format!(
                        "{} {}",
                        item.label,
                        item.description.as_deref().unwrap_or("")
                    );
                    (item.clone(), text)
                })
                .collect();
            fuzzy_filter(paired, query, |pair: &(SelectItem, String)| pair.1.as_str())
                .into_iter()
                .map(|(item, _)| item)
                .collect()
        } else {
            self.select_options.clone()
        };
        self.rebuild_list(filtered);
    }

    /// Drain the recorded selection (upstream `onSelect`).
    pub fn take_selection(&mut self) -> Option<String> {
        let selection = self.selection_rx.borrow_mut().try_recv().ok();
        if let Some(selection) = &selection {
            if let Some(on_select) = &mut self.on_select {
                (on_select)(selection);
            }
        }
        selection
    }

    /// Drain the recorded cancellation (upstream `onCancel`).
    pub fn take_cancelled(&mut self) -> bool {
        if self
            .cancel_flag
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            if let Some(on_cancel) = &mut self.on_cancel {
                (on_cancel)();
            }
            true
        } else {
            false
        }
    }

    /// Drain a recorded selection-change (upstream `onSelectionChange`).
    pub fn take_selection_change(&mut self) -> Option<String> {
        let change = self
            .selection_change_rx
            .as_ref()?
            .borrow_mut()
            .try_recv()
            .ok();
        if let Some(change) = &change {
            if let Some(on_selection_change) = &mut self.on_selection_change {
                (on_selection_change)(change);
            }
        }
        change
    }
}

impl SubmenuComponent for SelectSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // Title, description, [search], spacer
        let head_count = self.children.len().saturating_sub(2);
        for child in self.children.iter_mut().take(head_count) {
            match child {
                SubmenuChild::Text(text) => lines.extend(text.render(width)),
                SubmenuChild::Spacer(count) => lines.extend(spacer_lines(*count)),
            }
        }
        // Select list
        lines.extend(self.select_list.render(width));
        // Spacer + hint
        for child in self.children.iter_mut().skip(head_count) {
            match child {
                SubmenuChild::Text(text) => lines.extend(text.render(width)),
                SubmenuChild::Spacer(count) => lines.extend(spacer_lines(*count)),
            }
        }
        lines
    }

    fn handle_input(&mut self, data: &str) {
        if self.search_input.is_some() {
            let is_nav = with_keybindings(|kb| {
                kb.matches(data, "tui.select.up")
                    || kb.matches(data, "tui.select.down")
                    || kb.matches(data, "tui.select.confirm")
                    || kb.matches(data, "tui.select.cancel")
            });
            if is_nav {
                self.select_list.handle_input(data);
            } else if let Some(input) = &mut self.search_input {
                input.handle_input(data);
                let query = input.value().to_string();
                self.apply_filter(&query);
            }
        } else {
            self.select_list.handle_input(data);
        }
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        None
    }

    fn invalidate(&mut self) {}
}

// ===========================================================================
// SteppedSubmenu (settings-submenu.ts)
// ===========================================================================

/// Upstream `SteppedSubmenuStep`.
pub struct SteppedSubmenuStep {
    pub key: String,
    pub title: Box<dyn Fn(&[(String, String)]) -> String>,
    pub description: Box<dyn Fn(&[(String, String)]) -> String>,
    pub options: Box<dyn Fn(&[(String, String)]) -> Vec<SelectItem>>,
    pub preselect: Option<Box<dyn Fn(&[(String, String)]) -> Option<String>>>,
    pub searchable: bool,
    pub layout: Option<(usize, usize)>,
}

/// Upstream `SteppedSubmenu`.
pub struct SteppedSubmenu {
    theme: Arc<Theme>,
    steps: Vec<SteppedSubmenuStep>,
    context: Vec<(String, String)>,
    loop_back: bool,
    current_step: usize,
    active: Option<SelectSubmenu>,
    on_complete: Box<dyn FnMut(&[(String, String)])>,
    on_cancel: Box<dyn FnMut()>,
    done: Option<SubmenuDone>,
}

impl SteppedSubmenu {
    pub fn new(
        theme: Arc<Theme>,
        steps: Vec<SteppedSubmenuStep>,
        on_complete: Box<dyn FnMut(&[(String, String)])>,
        on_cancel: Box<dyn FnMut()>,
        loop_back: bool,
    ) -> Self {
        let mut stepped = Self {
            theme,
            steps,
            context: Vec::new(),
            loop_back,
            current_step: 0,
            active: None,
            on_complete,
            on_cancel,
            done: None,
        };
        stepped.active = Some(stepped.build_step(0));
        stepped
    }

    fn build_step(&mut self, step_index: usize) -> SelectSubmenu {
        let step = &self.steps[step_index];
        let total = self.steps.len();
        let step_label = if total > 1 {
            format!("Step {}/{} \u{b7} ", step_index + 1, total)
        } else {
            String::new()
        };
        let title = (step.title)(&self.context);
        let description = format!("{}{}", step_label, (step.description)(&self.context));
        let items = (step.options)(&self.context);
        let preselect = step
            .preselect
            .as_ref()
            .and_then(|preselect| preselect(&self.context))
            .unwrap_or_default();

        // The outcome is drained by `handle_input` (module docs).
        SelectSubmenu::new(
            Arc::clone(&self.theme),
            &title,
            &description,
            items,
            &preselect,
            Box::new(|_| {}),
            Box::new(|| {}),
            None,
            step.searchable,
            step.layout,
        )
    }
}

impl SubmenuComponent for SteppedSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        match self.active.as_mut() {
            Some(active) => active.render(width),
            None => Vec::new(),
        }
    }

    fn handle_input(&mut self, data: &str) {
        // Delegate, then apply the drained outcome (module docs).
        if let Some(active) = self.active.as_mut() {
            active.handle_input(data);
            let _ = active.take_selection_change();
            if active.take_cancelled() {
                // Esc: go back one step, or cancel at step 0.
                if self.current_step > 0 {
                    let key = self.steps[self.current_step].key.clone();
                    self.context.retain(|(entry_key, _)| *entry_key != key);
                    self.current_step -= 1;
                    self.active = Some(self.build_step(self.current_step));
                } else {
                    (self.on_cancel)();
                    self.done = Some(SubmenuDone {
                        selected_value: Some(model_thinking_overrides_summary(&self.context)),
                        navigate_to: None,
                    });
                }
                return;
            }
            if let Some(value) = active.take_selection() {
                let key = self.steps[self.current_step].key.clone();
                if let Some(entry) = self
                    .context
                    .iter_mut()
                    .find(|(entry_key, _)| *entry_key == key)
                {
                    entry.1 = value.clone();
                } else {
                    self.context.push((key, value));
                }
                if self.current_step < self.steps.len() - 1 {
                    self.current_step += 1;
                    self.active = Some(self.build_step(self.current_step));
                } else {
                    (self.on_complete)(&self.context);
                    if self.loop_back {
                        self.context.clear();
                        self.current_step = 0;
                        self.active = Some(self.build_step(0));
                    } else {
                        self.done = Some(SubmenuDone::default());
                    }
                }
            }
        }
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        self.done.take()
    }

    fn invalidate(&mut self) {}
}

// ===========================================================================
// WarningSettingsSubmenu
// ===========================================================================

/// Upstream `WarningSettingsSubmenu`.
pub struct WarningSettingsSubmenu {
    state: Rc<RefCell<WarningSettings>>,
    list: SelectorList,
    cancel_flag: CancelFlag,
    done: Option<SubmenuDone>,
}

impl WarningSettingsSubmenu {
    pub fn new(
        theme: Arc<Theme>,
        warnings: WarningSettings,
        mut on_change: Box<dyn FnMut(WarningSettings)>,
        mut on_done: Box<dyn FnMut()>,
    ) -> Self {
        let state = Rc::new(RefCell::new(warnings.clone()));
        let done_flag = cancel_flag();

        let items = vec![SelectorItem {
            id: "anthropic-extra-usage".to_string(),
            label: "Anthropic extra usage".to_string(),
            description: Some(
                "Warn when Anthropic subscription auth may use paid extra usage".to_string(),
            ),
            current_value: if state.borrow().anthropic_extra_usage.unwrap_or(true) {
                "true".to_string()
            } else {
                "false".to_string()
            },
            values: vec!["true".to_string(), "false".to_string()],
            submenu: None,
        }];

        let change_state = Rc::clone(&state);
        let on_change_callback = Box::new(move |_id: &str, new_value: &str| {
            change_state.borrow_mut().anthropic_extra_usage = Some(new_value == "true");
            on_change(change_state.borrow().clone());
        });

        let done_flag_for_cancel = Arc::clone(&done_flag);
        let on_cancel = Box::new(move || {
            done_flag_for_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            on_done();
        });

        let list = SelectorList::new(
            items,
            1,
            get_settings_list_theme(&theme),
            on_change_callback,
            on_cancel,
            false,
        );

        Self {
            state,
            list,
            cancel_flag: done_flag,
            done: None,
        }
    }

    /// The current warnings state (for test assertions).
    pub fn state(&self) -> WarningSettings {
        self.state.borrow().clone()
    }
}

impl SubmenuComponent for WarningSettingsSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.list.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.handle_input(data);
        if self
            .cancel_flag
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.done = Some(SubmenuDone::default());
        }
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        self.done.take()
    }
}

// ===========================================================================
// ThemeSubmenu
// ===========================================================================

#[derive(Clone, Debug)]
enum ThemeCommand {
    // `Selected`'s payload is written by the r19 shell wiring; only the
    // variant identity is matched today.
    #[allow(dead_code)]
    Selected(String),
    Cancelled,
    Apply,
    SwitchToSingle,
    Pick {
        item_id: &'static str,
        value: String,
    },
}

struct ThemeState {
    mode: &'static str,
    single_theme: String,
    light_theme: String,
    dark_theme: String,
    preview: Option<Box<dyn FnMut(&str) + Send>>,
}

enum ThemeMenu {
    Single(SelectSubmenu),
    Automatic(AutomaticThemeMenu),
}

/// The automatic-mode menu: intro text plus a settings list over
/// light/dark/apply/mode-switch items.
struct AutomaticThemeMenu {
    intro: Vec<Text>,
    list: SelectorList,
}

/// Upstream `ThemeSubmenu`.
pub struct ThemeSubmenu {
    theme: Arc<Theme>,
    state: ThemeState,
    available_themes: Vec<String>,
    terminal_theme: TerminalThemeKind,
    original_theme_setting: String,
    active: Option<ThemeMenu>,
    commands: Rc<RefCell<Vec<ThemeCommand>>>,
    done: Option<SubmenuDone>,
}

impl ThemeSubmenu {
    pub fn new(
        theme: Arc<Theme>,
        current_theme_setting: &str,
        terminal_theme: TerminalThemeKind,
        available_themes: Vec<String>,
        on_done: Box<dyn FnMut(Option<&str>) + Send>,
        on_preview: Option<Box<dyn FnMut(&str) + Send>>,
    ) -> Self {
        let auto_theme = parse_auto_theme_setting(Some(current_theme_setting));
        let automatic_themes = default_automatic_themes(current_theme_setting, &available_themes);
        let fixed_theme = if auto_theme.is_some() || current_theme_setting.contains('/') {
            None
        } else {
            Some(current_theme_setting.to_string())
        };
        let mode = if auto_theme.is_some() {
            "automatic"
        } else {
            "single"
        };
        let active_automatic = if terminal_theme == "light" {
            automatic_themes.0.clone()
        } else {
            automatic_themes.1.clone()
        };
        let single_theme = preferred_theme(
            &available_themes,
            fixed_theme.as_deref().or(if auto_theme.is_some() {
                Some(active_automatic.as_str())
            } else {
                None
            }),
            SYSTEM_THEME_NAME,
        );

        let commands: Rc<RefCell<Vec<ThemeCommand>>> = Rc::new(RefCell::new(Vec::new()));

        let mut submenu = Self {
            theme,
            state: ThemeState {
                mode,
                single_theme,
                light_theme: automatic_themes.0,
                dark_theme: automatic_themes.1,
                preview: on_preview,
            },
            available_themes,
            terminal_theme,
            original_theme_setting: current_theme_setting.to_string(),
            active: None,
            commands,
            done: None,
        };
        let _ = &on_done;
        if mode == "automatic" {
            submenu.show_automatic_menu();
        } else {
            submenu.show_single_menu();
        }
        submenu
    }

    fn preview(&mut self, value: &str) {
        if let Some(preview) = &mut self.state.preview {
            (preview)(value);
        }
    }

    fn get_theme_setting(&self) -> String {
        if self.state.mode == "automatic" {
            format!("{}/{}", self.state.light_theme, self.state.dark_theme)
        } else {
            self.state.single_theme.clone()
        }
    }

    fn get_active_automatic_theme(&self) -> String {
        if self.terminal_theme == "light" {
            self.state.light_theme.clone()
        } else {
            self.state.dark_theme.clone()
        }
    }

    fn apply(&mut self, theme_setting: &str) {
        // Upstream `onDone(themeSetting)`: the host settings list records the
        // value (firing onThemeChange) and closes this submenu.
        self.done = Some(SubmenuDone {
            selected_value: Some(theme_setting.to_string()),
            navigate_to: None,
        });
    }

    fn cancel(&mut self) {
        let original = self.original_theme_setting.clone();
        self.preview(&original);
        self.done = Some(SubmenuDone::default());
    }

    fn show_single_menu(&mut self) {
        self.state.mode = "single";
        let single_theme = self.state.single_theme.clone();
        let items = single_mode_theme_items(&self.available_themes, &single_theme);
        let commands = Rc::clone(&self.commands);
        let cancel_commands = Rc::clone(&self.commands);
        let preview_commands = Rc::clone(&self.commands);
        let select = SelectSubmenu::new(
            Arc::clone(&self.theme),
            "Theme",
            "Select a theme, or choose automatic to follow terminal appearance.",
            items,
            &single_theme,
            Box::new(move |value: &str| {
                commands
                    .borrow_mut()
                    .push(ThemeCommand::Selected(value.to_string()));
            }),
            Box::new(move || {
                cancel_commands.borrow_mut().push(ThemeCommand::Cancelled);
            }),
            Some(Box::new(move |value: &str| {
                preview_commands
                    .borrow_mut()
                    .push(ThemeCommand::Selected(value.to_string()));
            })),
            false,
            None,
        );
        self.active = Some(ThemeMenu::Single(select));
    }

    #[allow(clippy::vec_init_then_push)] // mirrors upstream's incremental intro build
    fn show_automatic_menu(&mut self) {
        self.state.mode = "automatic";
        let mut intro: Vec<Text> = Vec::new();
        intro.push(Text::with_options(
            &self
                .theme
                .bold(&theme_fg(&self.theme, "accent", "Automatic Theme")),
            0,
            0,
            None,
        ));
        intro.push(Text::with_options("", 0, 0, None));
        intro.push(Text::with_options(
            &theme_fg(
                &self.theme,
                "muted",
                "Choose themes for terminal light and dark appearance.",
            ),
            0,
            0,
            None,
        ));
        intro.push(Text::with_options(
            &theme_fg(
                &self.theme,
                "muted",
                "Light/dark detection requires terminal support.",
            ),
            0,
            0,
            None,
        ));
        intro.push(Text::with_options("", 0, 0, None));

        let light_theme = self.state.light_theme.clone();
        let dark_theme = self.state.dark_theme.clone();
        let commands = Rc::clone(&self.commands);
        let cancel_commands = Rc::clone(&self.commands);

        let make_theme_opener =
            |item_id: &'static str, title: &'static str, description: &'static str| {
                let available_themes = self.available_themes.clone();
                let theme_for_opener = Arc::clone(&self.theme);
                let pick_commands = Rc::clone(&self.commands);
                move |current_value: &str| {
                    Box::new(ThemeSelectSubmenu::new(
                        Arc::clone(&theme_for_opener),
                        title,
                        description,
                        available_themes.clone(),
                        current_value,
                        item_id,
                        Rc::clone(&pick_commands),
                    )) as Box<dyn SubmenuComponent>
                }
            };
        let light_opener = make_theme_opener(
            "light-theme",
            "Light Theme",
            "Select the theme to use for light terminal appearance",
        );
        let dark_opener = make_theme_opener(
            "dark-theme",
            "Dark Theme",
            "Select the theme to use for dark terminal appearance",
        );

        let items = vec![
            SelectorItem {
                id: "light-theme".into(),
                label: "Light theme".into(),
                description: Some(
                    "Theme to use in automatic mode when the terminal is light".into(),
                ),
                current_value: light_theme,
                values: Vec::new(),
                submenu: Some(Box::new(light_opener)),
            },
            SelectorItem {
                id: "dark-theme".into(),
                label: "Dark theme".into(),
                description: Some(
                    "Theme to use in automatic mode when the terminal is dark".into(),
                ),
                current_value: dark_theme,
                values: Vec::new(),
                submenu: Some(Box::new(dark_opener)),
            },
            SelectorItem {
                id: "apply".into(),
                label: "Apply".into(),
                description: Some("Save and go back".into()),
                current_value: "save and go back".into(),
                values: vec!["save and go back".into()],
                submenu: None,
            },
            SelectorItem {
                id: "single-mode".into(),
                label: "Change mode".into(),
                description: Some("Switch to one theme for light and dark".into()),
                current_value: "switch to single theme".into(),
                values: vec!["switch to single theme".into()],
                submenu: None,
            },
        ];

        let list = SelectorList::new(
            items,
            4,
            get_settings_list_theme(&self.theme),
            Box::new(move |id: &str, _new_value: &str| match id {
                "single-mode" => commands.borrow_mut().push(ThemeCommand::SwitchToSingle),
                "apply" => commands.borrow_mut().push(ThemeCommand::Apply),
                _ => {}
            }),
            Box::new(move || {
                cancel_commands.borrow_mut().push(ThemeCommand::Cancelled);
            }),
            false,
        );
        self.active = Some(ThemeMenu::Automatic(AutomaticThemeMenu { intro, list }));
    }
}

/// A theme pick submenu opened from the automatic menu (upstream
/// `createThemeSelect`): on select it previews the new setting and reports
/// `done(value)` to the hosting settings list.
struct ThemeSelectSubmenu {
    select: SelectSubmenu,
    item_id: &'static str,
    commands: Rc<RefCell<Vec<ThemeCommand>>>,
    done: Option<SubmenuDone>,
}

impl ThemeSelectSubmenu {
    fn new(
        theme: Arc<Theme>,
        title: &str,
        description: &str,
        available_themes: Vec<String>,
        current_value: &str,
        item_id: &'static str,
        commands: Rc<RefCell<Vec<ThemeCommand>>>,
    ) -> Self {
        let items = theme_items(&available_themes, current_value);
        let pick_commands = Rc::clone(&commands);
        let cancel_commands = Rc::clone(&commands);
        let select = SelectSubmenu::new(
            theme,
            title,
            description,
            items,
            current_value,
            Box::new(move |value: &str| {
                pick_commands.borrow_mut().push(ThemeCommand::Pick {
                    item_id,
                    value: value.to_string(),
                });
            }),
            Box::new(move || {
                cancel_commands.borrow_mut().push(ThemeCommand::Cancelled);
            }),
            None,
            false,
            None,
        );
        Self {
            select,
            item_id,
            commands,
            done: None,
        }
    }
}

impl SubmenuComponent for ThemeSelectSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.select.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.select.handle_input(data);
        if let Some(value) = self.select.take_selection() {
            // Upstream: set the theme, preview, and `done(value)` (the host
            // list then records the value and closes this submenu).
            self.done = Some(SubmenuDone {
                selected_value: Some(value),
                navigate_to: None,
            });
        }
        if self.select.take_cancelled() {
            self.done = Some(SubmenuDone::default());
        }
        let _ = &self.item_id;
        let _ = &mut self.commands;
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        self.done.take()
    }
}

impl SubmenuComponent for ThemeSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        match self.active.as_mut() {
            Some(ThemeMenu::Single(select)) => select.render(width),
            Some(ThemeMenu::Automatic(menu)) => {
                let mut lines: Vec<String> = Vec::new();
                for text in &mut menu.intro {
                    lines.extend(text.render(width));
                }
                lines.extend(menu.list.render(width));
                lines
            }
            None => Vec::new(),
        }
    }

    fn handle_input(&mut self, data: &str) {
        // Drain outcomes from the active menu first (borrows end before the
        // state transitions below rebuild the menu).
        enum ThemeOutcome {
            None,
            SelectionChanged(String),
            Selected(String),
            Cancelled,
            Commands(Vec<ThemeCommand>),
        }
        let outcome = match self.active.as_mut() {
            Some(ThemeMenu::Single(select)) => {
                select.handle_input(data);
                if let Some(value) = select.take_selection_change() {
                    ThemeOutcome::SelectionChanged(value)
                } else if let Some(value) = select.take_selection() {
                    ThemeOutcome::Selected(value)
                } else if select.take_cancelled() {
                    ThemeOutcome::Cancelled
                } else {
                    ThemeOutcome::None
                }
            }
            Some(ThemeMenu::Automatic(menu)) => {
                menu.list.handle_input(data);
                ThemeOutcome::Commands(std::mem::take(&mut *self.commands.borrow_mut()))
            }
            None => ThemeOutcome::None,
        };
        match outcome {
            ThemeOutcome::None => {}
            ThemeOutcome::SelectionChanged(value) => {
                let preview_value = if value == AUTOMATIC_THEME_VALUE {
                    format!("{}/{}", self.state.light_theme, self.state.dark_theme)
                } else {
                    value
                };
                self.preview(&preview_value);
            }
            ThemeOutcome::Selected(value) => {
                if value == AUTOMATIC_THEME_VALUE {
                    self.state.mode = "automatic";
                    let setting = self.get_theme_setting();
                    self.preview(&setting);
                    self.show_automatic_menu();
                } else {
                    self.state.single_theme = value.clone();
                    self.apply(&value);
                }
            }
            ThemeOutcome::Cancelled => self.cancel(),
            ThemeOutcome::Commands(commands) => {
                for command in commands {
                    match command {
                        ThemeCommand::Pick { item_id, value } => {
                            match item_id {
                                "light-theme" => self.state.light_theme = value,
                                "dark-theme" => self.state.dark_theme = value,
                                _ => {}
                            }
                            let setting = self.get_theme_setting();
                            self.preview(&setting);
                        }
                        ThemeCommand::SwitchToSingle => {
                            self.state.mode = "single";
                            let active = self.get_active_automatic_theme();
                            self.state.single_theme = active.clone();
                            self.preview(&active);
                            self.show_single_menu();
                            return;
                        }
                        ThemeCommand::Apply => {
                            let setting = self.get_theme_setting();
                            self.apply(&setting);
                        }
                        ThemeCommand::Cancelled => {
                            self.cancel();
                            return;
                        }
                        ThemeCommand::Selected(_) => {}
                    }
                }
            }
        }
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        self.done.take()
    }
}

// ===========================================================================
// SettingsSelectorComponent
// ===========================================================================

/// Upstream `SettingsSelectorComponent`.
pub struct SettingsSelectorComponent {
    // Read by the r19 interactive-shell wiring (theme preview callbacks).
    #[allow(dead_code)]
    theme: Arc<Theme>,
    settings_list: SelectorList,
}

impl SettingsSelectorComponent {
    pub fn new(
        theme: Arc<Theme>,
        config: SettingsConfig,
        mut callbacks: SettingsCallbacks,
    ) -> Self {
        super::model_selector::set_default_theme(Arc::clone(&theme));
        let supports_images = get_capabilities().images.is_some();
        let follow_up_key = key_display_text("app.message.followUp");
        let cycle_thinking_key = key_display_text("app.thinking.cycle");
        let current_warnings = config.warnings.clone();
        let current_model_thinking_levels = config.model_thinking_levels.clone();
        let current_default_model_key = config
            .available_default_models
            .iter()
            .map(model_setting_key)
            .find(|key| *key == config.default_model);
        let current_model_key = config.current_model.as_ref().map(model_setting_key);

        let mut items: Vec<SelectorItem> = Vec::new();

        items.push(SelectorItem {
            id: "autocompact".into(),
            label: "Auto-compact".into(),
            description: Some("Automatically compact context when it gets too large".into()),
            current_value: bool_str(config.auto_compact),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "steering-mode".into(),
            label: "Steering mode".into(),
            description: Some(
                "Enter while streaming queues steering messages. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once.".into(),
            ),
            current_value: config.steering_mode.clone(),
            values: vec!["one-at-a-time".into(), "all".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "follow-up-mode".into(),
            label: "Follow-up mode".into(),
            description: Some(format!(
                "{follow_up_key} queues follow-up messages until agent stops. 'one-at-a-time': deliver one, wait for response. 'all': deliver all at once."
            )),
            current_value: config.follow_up_mode.clone(),
            values: vec!["one-at-a-time".into(), "all".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "transport".into(),
            label: "Transport".into(),
            description: Some(
                "Preferred transport for providers that support multiple transports".into(),
            ),
            current_value: config.transport.clone(),
            values: vec![
                "sse".into(),
                "websocket".into(),
                "websocket-cached".into(),
                "auto".into(),
            ],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "http-idle-timeout".into(),
            label: "HTTP idle timeout".into(),
            description: Some(
                "Maximum idle gap while waiting for HTTP headers or body chunks. Disable for local models that pause longer than five minutes.".into(),
            ),
            current_value: format_http_idle_timeout_ms(config.http_idle_timeout_ms),
            values: HTTP_IDLE_TIMEOUT_CHOICES.iter().map(|choice| choice.label.to_string()).collect(),
            submenu: None,
        });
        items.push(SelectorItem {
            id: "cache-warming-mode".into(),
            label: "Cache warming".into(),
            description: Some(
                "off; streaming while the agent runs; idle also between runs while continuation stays profitable".into(),
            ),
            current_value: config.cache_warming_mode.clone(),
            values: CACHE_WARMING_MODES.iter().map(|mode| mode.to_string()).collect(),
            submenu: None,
        });
        items.push(SelectorItem {
            id: "hide-thinking".into(),
            label: "Hide thinking".into(),
            description: Some("Hide thinking blocks in assistant responses".into()),
            current_value: bool_str(config.hide_thinking_block),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "mermaid-rendering".into(),
            label: "Mermaid diagrams".into(),
            description: Some("Render Mermaid code blocks as Unicode diagrams".into()),
            current_value: config.mermaid_rendering_mode.clone(),
            values: vec!["off".into(), "final".into(), "streaming".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "cache-miss-notices".into(),
            label: "Cache miss notices".into(),
            description: Some(
                "Show transcript notices for cache costs and provider recovery diagnostics".into(),
            ),
            current_value: bool_str(config.show_cache_miss_notices),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "collapse-changelog".into(),
            label: "Collapse changelog".into(),
            description: Some("Show condensed changelog after updates".into()),
            current_value: bool_str(config.collapse_changelog),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "quiet-startup".into(),
            label: "Quiet startup".into(),
            description: Some(
                "Disable verbose printing at startup (header: keep only the startup header)".into(),
            ),
            // Upstream `String(config.quietStartup)`: "true" | "header" | "false".
            current_value: match config.quiet_startup {
                QuietStartup::Full => "true".to_string(),
                QuietStartup::Header => "header".to_string(),
                QuietStartup::Off => "false".to_string(),
            },
            values: vec!["true".into(), "header".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "install-telemetry".into(),
            label: "Install telemetry".into(),
            description: Some(
                "Send an anonymous version/update ping after changelog-detected updates".into(),
            ),
            current_value: bool_str(config.enable_install_telemetry),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "default-project-trust".into(),
            label: "Default project trust".into(),
            description: Some(
                "Fallback behavior when no extension or saved trust decision decides project trust"
                    .into(),
            ),
            current_value: DEFAULT_PROJECT_TRUST_LABELS
                .iter()
                .find(|(value, _)| *value == config.default_project_trust)
                .map(|(_, label)| label.to_string())
                .unwrap_or_default(),
            values: DEFAULT_PROJECT_TRUST_LABELS
                .iter()
                .map(|(_, label)| label.to_string())
                .collect(),
            submenu: None,
        });
        items.push(SelectorItem {
            id: "double-escape-action".into(),
            label: "Double-escape action".into(),
            description: Some("Action when pressing Escape twice with empty editor".into()),
            current_value: config.double_escape_action.clone(),
            values: vec!["tree".into(), "fork".into(), "none".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "tree-filter-mode".into(),
            label: "Tree filter mode".into(),
            description: Some("Default filter when opening /tree".into()),
            current_value: config.tree_filter_mode.clone(),
            values: vec![
                "default".into(),
                "no-tools".into(),
                "user-only".into(),
                "labeled-only".into(),
                "all".into(),
            ],
            submenu: None,
        });

        // Warnings submenu
        {
            let warnings_cell = Rc::new(RefCell::new(current_warnings.clone()));
            let on_warnings_change = Rc::new(RefCell::new(std::mem::replace(
                &mut callbacks.on_warnings_change,
                Box::new(|_| {}) as Box<dyn FnMut(WarningSettings) + Send>,
            )));
            let theme_for_submenu = Arc::clone(&theme);
            items.push(SelectorItem {
                id: "warnings".into(),
                label: "Warnings".into(),
                description: Some("Enable or disable individual warnings".into()),
                current_value: "configure".into(),
                values: Vec::new(),
                submenu: Some(Box::new(move |_current_value: &str| {
                    let change_sink: Rc<RefCell<Option<Box<dyn FnMut(WarningSettings)>>>> =
                        Rc::new(RefCell::new(None));
                    let done_flag = cancel_flag();
                    let warnings = warnings_cell.borrow().clone();
                    let mut change_callback = on_warnings_change.borrow_mut();
                    let installed = std::mem::replace(
                        &mut *change_callback,
                        Box::new(|_| {}) as Box<dyn FnMut(WarningSettings) + Send>,
                    );
                    let sink_for_cb = Rc::clone(&change_sink);
                    let state_for_cb = Rc::clone(&warnings_cell);
                    let submenu = WarningSettingsSubmenu::new(
                        Arc::clone(&theme_for_submenu),
                        warnings,
                        Box::new(move |warnings| {
                            // Track the latest state (upstream `currentWarnings`)
                            // and forward to the host callback when installed.
                            *state_for_cb.borrow_mut() = warnings.clone();
                            if let Some(change) = &mut *sink_for_cb.borrow_mut() {
                                (change)(warnings);
                            }
                        }),
                        {
                            let done_flag_for_cancel = Arc::clone(&done_flag);
                            Box::new(move || {
                                done_flag_for_cancel
                                    .store(true, std::sync::atomic::Ordering::SeqCst)
                            })
                        },
                    );
                    *change_sink.borrow_mut() = Some(installed);
                    // Wrap so the Esc-cancellation surfaces as `done()`.
                    Box::new(DoneFlagSubmenu {
                        inner: Box::new(submenu),
                        done_flag,
                    })
                })),
            });
        }

        // Model thinking stepped submenu
        {
            let summary = model_thinking_overrides_summary(&current_model_thinking_levels);
            let state = Rc::new(RefCell::new(current_model_thinking_levels.clone()));
            let available = config.available_default_models.clone();
            let thinking_level = config.thinking_level.clone();
            let on_level_change = Rc::new(RefCell::new(std::mem::replace(
                &mut callbacks.on_model_thinking_level_change,
                Box::new(|_, _, _| {}),
            )));
            let on_level_remove = Rc::new(RefCell::new(std::mem::replace(
                &mut callbacks.on_model_thinking_level_remove,
                Box::new(|_, _| {}),
            )));

            let theme_thinking = Arc::clone(&theme);
            items.push(SelectorItem {
                id: "model-thinking".into(),
                label: "Default thinking level per model".into(),
                description: Some(format!(
                    "Override the default thinking level for specific models. {cycle_thinking_key} cycles in-session."
                )),
                current_value: summary,
                values: Vec::new(),
                submenu: Some(Box::new(move |_current_value: &str| {
                    Box::new(build_model_thinking_submenu(
                        Arc::clone(&theme_thinking),
                        state.clone(),
                        available.clone(),
                        current_model_key.clone(),
                        current_default_model_key.clone(),
                        thinking_level.clone(),
                        Rc::clone(&on_level_change),
                        Rc::clone(&on_level_remove),
                    ))
                })),
            });
        }

        items.push(SelectorItem {
            id: "tui-mode".into(),
            label: "TUI mode".into(),
            description: Some(
                "Interface layout; regular mode uses the terminal's normal scrollback".into(),
            ),
            current_value: config.tui_mode.clone(),
            values: vec!["regular".into(), "fullscreen".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "fullscreen-exit-output".into(),
            label: "Fullscreen exit output".into(),
            description: Some(
                "Print the transcript or only a session resume hint when exiting fullscreen mode"
                    .into(),
            ),
            current_value: config.fullscreen_exit_output.clone(),
            values: vec!["transcript".into(), "resume-hint".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "fullscreen-scrollbar".into(),
            label: "Fullscreen scrollbar".into(),
            description: Some(
                "Scrollbar behavior in fullscreen mode; has no effect in regular mode".into(),
            ),
            current_value: config.fullscreen_scrollbar.clone(),
            values: vec!["auto".into(), "always".into(), "hidden".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "fullscreen-copy-on-select".into(),
            label: "Fullscreen copy on select".into(),
            description: Some("Automatically copy selected text in fullscreen mode; disable to copy selections with Ctrl+X".into()),
            current_value: bool_str(config.fullscreen_copy_on_select),
            values: vec!["true".into(), "false".into()],
            submenu: None,
        });
        items.push(SelectorItem {
            id: "fullscreen-wheel-scroll-lines".into(),
            label: "Fullscreen wheel scrolling".into(),
            description: Some(
                "Lines per mouse-wheel event in fullscreen mode; 'auto' speeds up fast wheel spins where the terminal does not".into(),
            ),
            current_value: config.fullscreen_wheel_scroll_lines.clone(),
            values: wheel_scroll_line_values(&config.fullscreen_wheel_scroll_lines),
            submenu: None,
        });
        let on_theme_preview: Rc<RefCell<Option<Box<dyn FnMut(&str) + Send>>>> =
            Rc::new(RefCell::new(callbacks.on_theme_preview.take()));
        let terminal_theme_for_submenu = config.terminal_theme.clone();
        let available_themes_for_submenu = config.available_themes.clone();
        let theme_for_theme_submenu = Arc::clone(&theme);
        items.push(SelectorItem {
            id: "theme".into(),
            label: "Theme".into(),
            description: Some("Color theme for the interface".into()),
            current_value: config.current_theme.clone(),
            values: Vec::new(),
            submenu: Some(Box::new(move |current_value: &str| {
                Box::new(build_theme_submenu(
                    Arc::clone(&theme_for_theme_submenu),
                    current_value.to_string(),
                    terminal_theme_for_submenu.clone(),
                    available_themes_for_submenu.clone(),
                    on_theme_preview.borrow_mut().take(),
                )) as Box<dyn SubmenuComponent>
            })),
        });

        // Only show image toggles if the terminal supports images
        if supports_images {
            items.insert(
                1,
                SelectorItem {
                    id: "show-images".into(),
                    label: "Show images".into(),
                    description: Some("Render images inline in terminal".into()),
                    current_value: bool_str(config.show_images),
                    values: vec!["true".into(), "false".into()],
                    submenu: None,
                },
            );
            items.insert(
                2,
                SelectorItem {
                    id: "image-width-cells".into(),
                    label: "Image width".into(),
                    description: Some("Preferred inline image width in terminal cells".into()),
                    current_value: config.image_width_cells.to_string(),
                    values: vec!["60".into(), "80".into(), "120".into()],
                    submenu: None,
                },
            );
        }

        // Auto-resize images toggle (after the last image item, or autocompact
        // when images are unsupported — upstream `splice(supportsImages ? 3 : 1)`)
        let auto_resize_index = if supports_images { 3 } else { 1 };
        items.insert(
            auto_resize_index,
            SelectorItem {
                id: "auto-resize-images".into(),
                label: "Auto-resize images".into(),
                description: Some(
                    "Resize large images to 2000x2000 max for better model compatibility".into(),
                ),
                current_value: bool_str(config.auto_resize_images),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );

        let insert_after = |items: &mut Vec<SelectorItem>, after_id: &str, item: SelectorItem| {
            if let Some(index) = items.iter().position(|entry| entry.id == after_id) {
                items.insert(index + 1, item);
            }
        };

        insert_after(
            &mut items,
            "auto-resize-images",
            SelectorItem {
                id: "block-images".into(),
                label: "Block images".into(),
                description: Some("Prevent images from being sent to LLM providers".into()),
                current_value: bool_str(config.block_images),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "block-images",
            SelectorItem {
                id: "skill-commands".into(),
                label: "Skill commands".into(),
                description: Some("Register skills as /skill:name commands".into()),
                current_value: bool_str(config.enable_skill_commands),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "skill-commands",
            SelectorItem {
                id: "show-hardware-cursor".into(),
                label: "Show hardware cursor".into(),
                description: Some(
                    "Show the terminal cursor while still positioning it for IME support".into(),
                ),
                current_value: bool_str(config.show_hardware_cursor),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "show-hardware-cursor",
            SelectorItem {
                id: "editor-padding".into(),
                label: "Editor padding".into(),
                description: Some("Horizontal padding for input editor (0-3)".into()),
                current_value: config.editor_padding_x.to_string(),
                values: vec!["0".into(), "1".into(), "2".into(), "3".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "editor-padding",
            SelectorItem {
                id: "output-padding".into(),
                label: "Output padding".into(),
                description: Some(
                    "Horizontal padding for user messages, assistant messages, and thinking".into(),
                ),
                current_value: config.output_pad.to_string(),
                values: vec!["0".into(), "1".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "output-padding",
            SelectorItem {
                id: "autocomplete-max-visible".into(),
                label: "Autocomplete max items".into(),
                description: Some("Max visible items in autocomplete dropdown (3-20)".into()),
                current_value: config.autocomplete_max_visible.to_string(),
                values: vec![
                    "3".into(),
                    "5".into(),
                    "7".into(),
                    "10".into(),
                    "15".into(),
                    "20".into(),
                ],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "autocomplete-max-visible",
            SelectorItem {
                id: "clear-on-shrink".into(),
                label: "Clear on shrink".into(),
                description: Some(
                    "Clear empty rows when content shrinks (may cause flicker)".into(),
                ),
                current_value: bool_str(config.clear_on_shrink),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );
        insert_after(
            &mut items,
            "clear-on-shrink",
            SelectorItem {
                id: "terminal-progress".into(),
                label: "Terminal progress".into(),
                description: Some(
                    "Show OSC 9;4 progress indicators in the terminal tab bar".into(),
                ),
                current_value: bool_str(config.show_terminal_progress),
                values: vec!["true".into(), "false".into()],
                submenu: None,
            },
        );

        let mut callbacks = callbacks;
        let on_cancel = {
            let on_cancel = std::mem::replace(&mut callbacks.on_cancel, Box::new(|| {}));
            Box::new(on_cancel) as Box<dyn FnMut() + Send>
        };
        let on_change = build_dispatch_on_change(callbacks);

        let settings_list = SelectorList::new(
            items,
            10,
            get_settings_list_theme(&theme),
            on_change,
            on_cancel,
            true,
        );

        Self {
            theme,
            settings_list,
        }
    }

    pub fn settings_list(&mut self) -> &mut SelectorList {
        &mut self.settings_list
    }
}

/// Wraps a submenu whose Esc-cancellation must surface as `done()`.
struct DoneFlagSubmenu {
    inner: Box<dyn SubmenuComponent>,
    done_flag: CancelFlag,
}

impl SubmenuComponent for DoneFlagSubmenu {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.inner.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.inner.handle_input(data);
    }

    fn take_done(&mut self) -> Option<SubmenuDone> {
        if self
            .done_flag
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Some(SubmenuDone::default());
        }
        self.inner.take_done()
    }

    fn invalidate(&mut self) {
        self.inner.invalidate();
    }
}

fn build_dispatch_on_change(mut callbacks: SettingsCallbacks) -> Box<dyn FnMut(&str, &str) + Send> {
    Box::new(move |id: &str, new_value: &str| match id {
        "autocompact" => (callbacks.on_auto_compact_change)(new_value == "true"),
        "show-images" => (callbacks.on_show_images_change)(new_value == "true"),
        "image-width-cells" => {
            (callbacks.on_image_width_cells_change)(new_value.parse::<u32>().unwrap_or(0))
        }
        "auto-resize-images" => (callbacks.on_auto_resize_images_change)(new_value == "true"),
        "block-images" => (callbacks.on_block_images_change)(new_value == "true"),
        "skill-commands" => (callbacks.on_enable_skill_commands_change)(new_value == "true"),
        "steering-mode" => (callbacks.on_steering_mode_change)(new_value),
        "follow-up-mode" => (callbacks.on_follow_up_mode_change)(new_value),
        "transport" => (callbacks.on_transport_change)(new_value),
        "http-idle-timeout" => {
            if let Some(choice) = HTTP_IDLE_TIMEOUT_CHOICES
                .iter()
                .find(|choice| choice.label == new_value)
            {
                (callbacks.on_http_idle_timeout_ms_change)(choice.timeout_ms);
            }
        }
        "hide-thinking" => (callbacks.on_hide_thinking_block_change)(new_value == "true"),
        "mermaid-rendering" => (callbacks.on_mermaid_rendering_mode_change)(new_value),
        "cache-miss-notices" => (callbacks.on_show_cache_miss_notices_change)(new_value == "true"),
        "collapse-changelog" => (callbacks.on_collapse_changelog_change)(new_value == "true"),
        "quiet-startup" => (callbacks.on_quiet_startup_change)(match new_value {
            "header" => QuietStartup::Header,
            "true" => QuietStartup::Full,
            _ => QuietStartup::Off,
        }),
        "install-telemetry" => (callbacks.on_enable_install_telemetry_change)(new_value == "true"),
        "default-project-trust" => {
            if let Some((trust, _)) = DEFAULT_PROJECT_TRUST_LABELS
                .iter()
                .find(|(_, label)| *label == new_value)
            {
                (callbacks.on_default_project_trust_change)(trust);
            }
        }
        "double-escape-action" => (callbacks.on_double_escape_action_change)(new_value),
        "tree-filter-mode" => (callbacks.on_tree_filter_mode_change)(new_value),
        "show-hardware-cursor" => (callbacks.on_show_hardware_cursor_change)(new_value == "true"),
        "editor-padding" => {
            (callbacks.on_editor_padding_x_change)(new_value.parse::<u32>().unwrap_or(0))
        }
        "output-padding" => (callbacks.on_output_pad_change)(if new_value == "0" { 0 } else { 1 }),
        "autocomplete-max-visible" => {
            (callbacks.on_autocomplete_max_visible_change)(new_value.parse::<u32>().unwrap_or(0))
        }
        "clear-on-shrink" => (callbacks.on_clear_on_shrink_change)(new_value == "true"),
        "terminal-progress" => (callbacks.on_show_terminal_progress_change)(new_value == "true"),
        "tui-mode" => (callbacks.on_tui_mode_change)(new_value),
        "fullscreen-exit-output" => (callbacks.on_fullscreen_exit_output_change)(new_value),
        "fullscreen-scrollbar" => (callbacks.on_fullscreen_scrollbar_change)(new_value),
        "fullscreen-copy-on-select" => {
            (callbacks.on_fullscreen_copy_on_select_change)(new_value == "true")
        }
        "cache-warming-mode" => (callbacks.on_cache_warming_mode_change)(new_value),
        "fullscreen-wheel-scroll-lines" => {
            (callbacks.on_fullscreen_wheel_scroll_lines_change)(new_value)
        }
        "theme" => (callbacks.on_theme_change)(new_value),
        _ => {}
    })
}

#[allow(clippy::too_many_arguments)] // mirrors the upstream parameter list
fn build_model_thinking_submenu(
    theme: Arc<Theme>,
    state: Rc<RefCell<Vec<(String, String)>>>,
    available: Vec<Model>,
    current_model_key: Option<String>,
    current_default_model_key: Option<String>,
    thinking_level: String,
    on_level_change: Rc<RefCell<Box<dyn FnMut(&str, &str, &str) + Send>>>,
    on_level_remove: Rc<RefCell<Box<dyn FnMut(&str, &str) + Send>>>,
) -> SteppedSubmenu {
    // Step 1: pick a model.
    let model_options = {
        let state = state.clone();
        let available = available.clone();
        let current_model_key = current_model_key.clone();
        let current_default_model_key = current_default_model_key.clone();
        Box::new(move |_context: &[(String, String)]| -> Vec<SelectItem> {
            let mut sorted = available.clone();
            sorted.sort_by(|a, b| {
                let a_key = model_setting_key(a);
                let b_key = model_setting_key(b);
                if Some(a_key.as_str()) == current_model_key.as_deref() {
                    return std::cmp::Ordering::Less;
                }
                if Some(b_key.as_str()) == current_model_key.as_deref() {
                    return std::cmp::Ordering::Greater;
                }
                if Some(a_key.as_str()) == current_default_model_key.as_deref() {
                    return std::cmp::Ordering::Less;
                }
                if Some(b_key.as_str()) == current_default_model_key.as_deref() {
                    return std::cmp::Ordering::Greater;
                }
                a.provider.cmp(&b.provider)
            });
            let mut items: Vec<SelectItem> = sorted
                .iter()
                .map(|model| {
                    let key = model_setting_key(model);
                    let over = state
                        .borrow()
                        .iter()
                        .find(|(stored, _)| *stored == key)
                        .map(|(_, level)| level.clone());
                    SelectItem {
                        value: key,
                        label: model.id.clone(),
                        description: over,
                    }
                })
                .collect();
            if items.is_empty() {
                items.push(SelectItem {
                    value: "__none__".to_string(),
                    label: "No models available".to_string(),
                    description: Some(
                        "Log in to a provider or configure an API key first".to_string(),
                    ),
                });
            }
            items
        })
    };

    // Step 2: pick a level for the selected model.
    let level_options = {
        let state = state.clone();
        let available = available.clone();
        let thinking_level = thinking_level.clone();
        Box::new(move |context: &[(String, String)]| -> Vec<SelectItem> {
            let Some(model_key) = context
                .iter()
                .find(|(key, _)| key == "model")
                .map(|(_, value)| value.clone())
            else {
                return Vec::new();
            };
            let Some(model) = available
                .iter()
                .find(|model| model_setting_key(model) == model_key)
            else {
                return Vec::new();
            };
            let levels: Vec<String> = if model.reasoning {
                get_supported_thinking_levels(model)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            } else {
                vec!["off".to_string()]
            };
            let active_level = state
                .borrow()
                .iter()
                .find(|(key, _)| *key == model_key)
                .map(|(_, level)| level.clone());
            let mut items: Vec<SelectItem> = levels
                .iter()
                .map(|level| SelectItem {
                    value: level.clone(),
                    label: format!(
                        "{}{level}",
                        if Some(level.as_str()) == active_level.as_deref() {
                            "✓ "
                        } else {
                            "  "
                        }
                    ),
                    description: Some(
                        THINKING_DESCRIPTIONS
                            .iter()
                            .find(|(name, _)| *name == level.as_str())
                            .map(|(_, description)| description.to_string())
                            .unwrap_or_default(),
                    ),
                })
                .collect();
            if active_level.is_some() {
                items.push(SelectItem {
                    value: CLEAR_OVERRIDE_VALUE.to_string(),
                    label: "  (clear override)".to_string(),
                    description: Some(format!("Revert to global default ({thinking_level})")),
                });
            }
            items
        })
    };

    let complete_state = Rc::clone(&state);
    let on_complete = Box::new(move |context: &[(String, String)]| {
        let Some(model_key) = context
            .iter()
            .find(|(key, _)| key == "model")
            .map(|(_, value)| value.clone())
        else {
            return;
        };
        let Some(level) = context
            .iter()
            .find(|(key, _)| key == "level")
            .map(|(_, value)| value.clone())
        else {
            return;
        };
        let Some((provider, model_id)) = model_key.split_once('/') else {
            return;
        };
        if level == CLEAR_OVERRIDE_VALUE {
            (on_level_remove.borrow_mut())(provider, model_id);
            complete_state
                .borrow_mut()
                .retain(|(key, _)| *key != model_key);
        } else {
            (on_level_change.borrow_mut())(provider, model_id, &level);
            if let Some(entry) = complete_state
                .borrow_mut()
                .iter_mut()
                .find(|(key, _)| *key == model_key)
            {
                entry.1 = level;
            } else {
                complete_state.borrow_mut().push((model_key, level));
            }
        }
    });

    let title_model_state = Rc::clone(&state);
    let preselect_model_key = current_model_key.clone();
    let preselect_default_key = current_default_model_key.clone();
    let level_title_state = Rc::clone(&state);
    let _preselect_level_state = Rc::clone(&state);

    let steps = vec![
        SteppedSubmenuStep {
            key: "model".to_string(),
            title: Box::new(|_context: &[(String, String)]| "Per-Model Thinking Level".to_string()),
            description: Box::new(|_context: &[(String, String)]| {
                "Select a model to configure".to_string()
            }),
            options: model_options,
            preselect: Some(Box::new(move |_context: &[(String, String)]| {
                preselect_model_key
                    .clone()
                    .or_else(|| preselect_default_key.clone())
            })),
            searchable: true,
            layout: Some((
                MODEL_PICKER_MIN_PRIMARY_COLUMN_WIDTH,
                MODEL_PICKER_MAX_PRIMARY_COLUMN_WIDTH,
            )),
        },
        SteppedSubmenuStep {
            key: "level".to_string(),
            title: Box::new(move |context: &[(String, String)]| {
                let model_key = context
                    .iter()
                    .find(|(key, _)| key == "model")
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default();
                match available
                    .iter()
                    .find(|model| model_setting_key(model) == model_key)
                {
                    Some(model) => format!("Thinking Level for {}", model_display_label(model)),
                    None => format!("Thinking Level for {model_key}"),
                }
            }),
            description: Box::new(|_context: &[(String, String)]| {
                "Select default thinking level for this model".to_string()
            }),
            options: level_options,
            preselect: Some(Box::new(move |context: &[(String, String)]| {
                let model_key = context
                    .iter()
                    .find(|(key, _)| key == "model")
                    .map(|(_, value)| value.clone())?;
                let _ = &title_model_state;
                level_title_state
                    .borrow()
                    .iter()
                    .find(|(key, _)| *key == model_key)
                    .map(|(_, level)| level.clone())
            })),
            searchable: false,
            layout: None,
        },
    ];

    SteppedSubmenu::new(theme, steps, on_complete, Box::new(|| {}), true)
}

fn build_theme_submenu(
    theme: Arc<Theme>,
    current_theme_setting: String,
    terminal_theme: TerminalThemeKind,
    available_themes: Vec<String>,
    on_preview: Option<Box<dyn FnMut(&str) + Send>>,
) -> ThemeSubmenu {
    ThemeSubmenu::new(
        theme,
        &current_theme_setting,
        terminal_theme,
        available_themes,
        // The outer list drains this as done(selectedValue) → onThemeChange.
        Box::new(|_| {}),
        on_preview,
    )
}

impl Component for SettingsSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        lines.extend(DynamicBorder::default().render(width));
        lines.extend(self.settings_list.render(width));
        lines.extend(DynamicBorder::default().render(width));
        lines
    }

    fn handle_input(&mut self, data: &str) {
        self.settings_list.handle_input(data);
    }
}
