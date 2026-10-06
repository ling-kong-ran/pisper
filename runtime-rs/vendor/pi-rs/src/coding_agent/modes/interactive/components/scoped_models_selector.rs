//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/scoped-models-selector.ts` (401
//! lines, sha256
//! `a8524d7ccfaf6fd6044a268a06d43f6eee7325b047057c73bb43fa0b1286c612`).
//!
//! Slice conventions: see the module docs of [`super::model_selector`] (theme
//! seam, inline composite rendering, focused-input propagation). The
//! `EnabledIds` algebra is ported verbatim: `null` = all enabled.

use std::sync::Arc;

use crate::ai::types::Model;
use crate::coding_agent::modes::interactive::model_search::{
    get_model_search_text, ModelSearchItem,
};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::components::text::Text;
use crate::tui::fuzzy::fuzzy_filter;
use crate::tui::keybindings::with_keybindings;
use crate::tui::keys::matches_key;

use super::model_selector::{spacer_lines, theme_fg, DynamicBorder};

/// `EnabledIds`: `None` = all enabled (no filter), `Some(list)` = explicit
/// ordered list.
pub type EnabledIds = Option<Vec<String>>;

/// Upstream `isEnabled`.
fn is_enabled(enabled_ids: &EnabledIds, id: &str) -> bool {
    match enabled_ids {
        None => true,
        Some(list) => list.iter().any(|enabled| enabled == id),
    }
}

/// Upstream `normalizeEnabled`: collapse an explicit list back to `None`
/// (= all enabled) when it covers every available model.
fn normalize_enabled(result: Vec<String>, all_ids: &[String]) -> EnabledIds {
    if result.len() == all_ids.len() && result.iter().all(|id| all_ids.contains(id)) {
        None
    } else {
        Some(result)
    }
}

/// Upstream `toggle`.
fn toggle(enabled_ids: &EnabledIds, all_ids: &[String], id: &str) -> EnabledIds {
    match enabled_ids {
        None => Some(
            all_ids
                .iter()
                .filter(|model_id| model_id.as_str() != id)
                .cloned()
                .collect(),
        ),
        Some(list) => match list.iter().position(|enabled| enabled == id) {
            Some(index) => {
                let mut updated = list.clone();
                updated.remove(index);
                Some(updated)
            }
            None => {
                let mut updated = list.clone();
                updated.push(id.to_string());
                normalize_enabled(updated, all_ids)
            }
        },
    }
}

/// Upstream `enableAll`.
fn enable_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    if enabled_ids.is_none() {
        return None;
    }
    let mut result = enabled_ids.as_ref().expect("checked above").clone();
    for id in target_ids.unwrap_or(all_ids) {
        if !result.contains(id) {
            result.push(id.clone());
        }
    }
    normalize_enabled(result, all_ids)
}

/// Upstream `clearAll`.
fn clear_all(
    enabled_ids: &EnabledIds,
    all_ids: &[String],
    target_ids: Option<&[String]>,
) -> EnabledIds {
    match enabled_ids {
        None => match target_ids {
            Some(targets) => Some(
                all_ids
                    .iter()
                    .filter(|id| !targets.contains(id))
                    .cloned()
                    .collect(),
            ),
            None => Some(Vec::new()),
        },
        Some(list) => {
            let targets: std::collections::HashSet<&String> = target_ids
                .map(|targets| targets.iter().collect())
                .unwrap_or_else(|| list.iter().collect());
            Some(
                list.iter()
                    .filter(|id| !targets.contains(id))
                    .cloned()
                    .collect(),
            )
        }
    }
}

/// Upstream `move`: swap `id` with its neighbour `delta` positions away.
fn move_item(enabled_ids: &EnabledIds, id: &str, delta: i64) -> EnabledIds {
    let Some(list) = enabled_ids else {
        return None;
    };
    let list = list.clone();
    let Some(index) = list.iter().position(|enabled| enabled == id) else {
        return Some(list);
    };
    let new_index = index as i64 + delta;
    if new_index < 0 || new_index >= list.len() as i64 {
        return Some(list);
    }
    let mut result = list;
    result.swap(index, new_index as usize);
    Some(result)
}

/// Upstream `getSortedIds`.
fn get_sorted_ids(enabled_ids: &EnabledIds, all_ids: &[String]) -> Vec<String> {
    match enabled_ids {
        None => all_ids.to_vec(),
        Some(list) => {
            let mut sorted = list.clone();
            for id in all_ids {
                if !list.contains(id) {
                    sorted.push(id.clone());
                }
            }
            sorted
        }
    }
}

#[derive(Clone, Debug)]
struct ModelItem {
    full_id: String,
    model: Option<Model>,
    enabled: bool,
}

/// Upstream `ModelsConfig`.
pub struct ModelsConfig {
    pub all_models: Vec<Model>,
    pub enabled_model_ids: EnabledIds,
    pub refresh_status: Option<String>,
}

/// Upstream `ModelsCallbacks`.
pub struct ModelsCallbacks {
    /// Called whenever the enabled model set or order changes (session-only).
    pub on_change: Box<dyn FnMut(&EnabledIds) + Send>,
    /// Called when the user persists the current selection to settings.
    pub on_persist: Box<dyn FnMut(&EnabledIds) + Send>,
    pub on_cancel: Box<dyn FnMut() + Send>,
}

/// Upstream `ScopedModelsSelectorComponent`: enable/disable models for
/// Ctrl+P cycling; session-only until persisted with Ctrl+S.
pub struct ScopedModelsSelectorComponent {
    theme: Arc<Theme>,
    focused: bool,
    models_by_id: Vec<(String, Model)>,
    all_ids: Vec<String>,
    enabled_ids: EnabledIds,
    filtered_items: Vec<ModelItem>,
    selected_index: usize,
    search_input: Input,
    list_children: Vec<ListChild>,
    footer_text: Text,
    refresh_status_text: Option<Text>,
    callbacks: ModelsCallbacks,
    max_visible: usize,
    is_dirty: bool,
}

enum ListChild {
    Text(Text),
    Spacer(usize),
}

impl ScopedModelsSelectorComponent {
    pub fn new(theme: Arc<Theme>, config: ModelsConfig, callbacks: ModelsCallbacks) -> Self {
        let mut component = Self {
            theme,
            focused: false,
            models_by_id: Vec::new(),
            all_ids: Vec::new(),
            enabled_ids: config.enabled_model_ids.clone(),
            filtered_items: Vec::new(),
            selected_index: 0,
            search_input: Input::new(InputOptions::default()),
            list_children: Vec::new(),
            footer_text: Text::with_options("", 0, 0, None),
            refresh_status_text: None,
            callbacks,
            max_visible: 8,
            is_dirty: false,
        };
        for model in &config.all_models {
            let full_id = format!("{}/{}", model.provider, model.id);
            component
                .models_by_id
                .push((full_id.clone(), model.clone()));
            component.all_ids.push(full_id);
        }
        component.filtered_items = component.build_items();
        if let Some(refresh_status) = &config.refresh_status {
            let text = theme_fg(&component.theme, "muted", &format!("  {refresh_status}"));
            component.refresh_status_text = Some(Text::with_options(&text, 0, 0, None));
        }
        let footer = component.get_footer_text();
        component.footer_text = Text::with_options(&footer, 0, 0, None);
        component.update_list();
        component
    }

    /// Upstream `updateModels`.
    pub fn update_models(&mut self, models: &[Model], enabled_model_ids: Option<&[String]>) {
        let selected_id = self
            .filtered_items
            .get(self.selected_index)
            .map(|item| item.full_id.clone());
        if let Some(enabled) = enabled_model_ids {
            self.enabled_ids = Some(enabled.to_vec());
        }
        self.models_by_id.clear();
        self.all_ids.clear();
        for model in models {
            let full_id = format!("{}/{}", model.provider, model.id);
            self.models_by_id.push((full_id.clone(), model.clone()));
            self.all_ids.push(full_id);
        }
        self.refresh();
        let refreshed_index = selected_id.and_then(|selected| {
            self.filtered_items
                .iter()
                .position(|item| item.full_id == selected)
        });
        if let Some(index) = refreshed_index {
            self.selected_index = index;
            self.update_list();
        }
    }

    /// Upstream `setRefreshStatus`.
    pub fn set_refresh_status(&mut self, message: &str, kind: &str) {
        let text = theme_fg(&self.theme, kind, &format!("  {message}"));
        if let Some(refresh_status_text) = &mut self.refresh_status_text {
            refresh_status_text.set_text(&text);
        }
    }

    fn model(&self, full_id: &str) -> Option<&Model> {
        self.models_by_id
            .iter()
            .find(|(id, _)| id == full_id)
            .map(|(_, model)| model)
    }

    fn build_items(&self) -> Vec<ModelItem> {
        get_sorted_ids(&self.enabled_ids, &self.all_ids)
            .into_iter()
            .map(|id| ModelItem {
                enabled: is_enabled(&self.enabled_ids, &id),
                model: self.model(&id).cloned(),
                full_id: id,
            })
            .collect()
    }

    fn get_footer_text(&self) -> String {
        let enabled_count = match &self.enabled_ids {
            Some(list) => list.iter().filter(|id| self.model(id).is_some()).count(),
            None => self.all_ids.len(),
        };
        let unavailable_count = match &self.enabled_ids {
            Some(list) => list.iter().filter(|id| self.model(id).is_none()).count(),
            None => 0,
        };
        let all_enabled = self.enabled_ids.is_none();
        let count_text = if all_enabled {
            "all enabled".to_string()
        } else if unavailable_count > 0 {
            format!(
                "{enabled_count}/{} enabled · {unavailable_count} unavailable",
                self.all_ids.len()
            )
        } else {
            format!("{enabled_count}/{} enabled", self.all_ids.len())
        };
        let parts = [
            format!(
                "{} toggle",
                super::model_selector::key_display_text("tui.select.confirm")
            ),
            format!(
                "{} all",
                super::model_selector::key_display_text("app.models.enableAll")
            ),
            format!(
                "{} clear",
                super::model_selector::key_display_text("app.models.clearAll")
            ),
            format!(
                "{} provider",
                super::model_selector::key_display_text("app.models.toggleProvider")
            ),
            format!(
                "{}/{} reorder",
                super::model_selector::key_display_text("app.models.reorderUp"),
                super::model_selector::key_display_text("app.models.reorderDown")
            ),
            format!(
                "{} save",
                super::model_selector::key_display_text("app.models.save")
            ),
            count_text,
        ];
        if self.is_dirty {
            theme_fg(&self.theme, "dim", &format!("  {} ", parts.join(" · ")))
                + &theme_fg(&self.theme, "warning", "(unsaved)")
        } else {
            theme_fg(&self.theme, "dim", &format!("  {}", parts.join(" · ")))
        }
    }

    fn refresh(&mut self) {
        let query = self.search_input.value().to_string();
        let items = self.build_items();
        self.filtered_items = if !query.is_empty() {
            let paired: Vec<(ModelItem, String)> = items
                .into_iter()
                .map(|item| {
                    let text = match &item.model {
                        Some(model) => get_model_search_text(&ModelSearchItem {
                            id: model.id.clone(),
                            provider: model.provider.clone(),
                            name: (!model.name.is_empty()).then(|| model.name.clone()),
                        }),
                        None => item.full_id.clone(),
                    };
                    (item, text)
                })
                .collect();
            fuzzy_filter(paired, &query, |pair: &(ModelItem, String)| pair.1.as_str())
                .into_iter()
                .map(|(item, _)| item)
                .collect()
        } else {
            items
        };
        self.selected_index = self
            .selected_index
            .min(self.filtered_items.len().saturating_sub(1));
        self.update_list();
        let footer = self.get_footer_text();
        self.footer_text.set_text(&footer);
    }

    fn notify_change(&mut self) {
        let enabled_ids = self.enabled_ids.clone();
        (self.callbacks.on_change)(&enabled_ids);
    }

    fn update_list(&mut self) {
        self.list_children.clear();

        if self.filtered_items.is_empty() {
            let text = theme_fg(&self.theme, "muted", "  No matching models");
            self.list_children
                .push(ListChild::Text(Text::with_options(&text, 0, 0, None)));
            return;
        }

        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(self.filtered_items.len().saturating_sub(self.max_visible));
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());
        for i in start_index..end_index {
            let Some(item) = self.filtered_items.get(i) else {
                continue;
            };
            let is_selected = i == self.selected_index;
            let prefix = if is_selected {
                theme_fg(&self.theme, "accent", "→ ")
            } else {
                "  ".to_string()
            };
            let id = item
                .model
                .as_ref()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| item.full_id.clone());
            let styled_id = if item.model.is_some() {
                id
            } else {
                // Upstream `theme.strikethrough`.
                self.theme.strikethrough(&id)
            };
            let model_text = if is_selected {
                theme_fg(&self.theme, "accent", &styled_id)
            } else {
                styled_id
            };
            let provider_badge = match &item.model {
                Some(model) => theme_fg(&self.theme, "muted", &format!(" [{}]", model.provider)),
                None => theme_fg(&self.theme, "muted", " [unavailable]"),
            };
            let status = if item.model.is_some() && item.enabled {
                theme_fg(&self.theme, "accent", "✓ ")
            } else {
                "  ".to_string()
            };
            let line = format!("{prefix}{status}{model_text}{provider_badge}");
            self.list_children
                .push(ListChild::Text(Text::with_options(&line, 0, 0, None)));
        }

        if start_index > 0 || end_index < self.filtered_items.len() {
            let scroll = theme_fg(
                &self.theme,
                "muted",
                &format!(
                    "  ({}/{})",
                    self.selected_index + 1,
                    self.filtered_items.len()
                ),
            );
            self.list_children
                .push(ListChild::Text(Text::with_options(&scroll, 0, 0, None)));
        }

        if !self.filtered_items.is_empty() {
            let selected = &self.filtered_items[self.selected_index];
            self.list_children.push(ListChild::Spacer(1));
            let detail = match &selected.model {
                Some(model) => format!("  Model Name: {}", model.name),
                None => "  Model unavailable".to_string(),
            };
            self.list_children.push(ListChild::Text(Text::with_options(
                &theme_fg(&self.theme, "muted", &detail),
                0,
                0,
                None,
            )));
        }
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, data: &str) {
        // Navigation
        if with_keybindings(|kb| kb.matches(data, "tui.select.up")) {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_items.len() - 1
            } else {
                self.selected_index - 1
            };
            self.update_list();
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.select.down")) {
            if self.filtered_items.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == self.filtered_items.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.update_list();
            return;
        }

        // Reorder enabled models
        let reorder_up = with_keybindings(|kb| kb.matches(data, "app.models.reorderUp"));
        let reorder_down = with_keybindings(|kb| kb.matches(data, "app.models.reorderDown"));
        if reorder_up || reorder_down {
            if self.enabled_ids.is_none() {
                return;
            }
            let item = self.filtered_items.get(self.selected_index).cloned();
            if let Some(item) = item {
                if is_enabled(&self.enabled_ids, &item.full_id) {
                    let delta: i64 = if reorder_up { -1 } else { 1 };
                    let current_index = self
                        .enabled_ids
                        .as_ref()
                        .expect("checked above")
                        .iter()
                        .position(|id| *id == item.full_id)
                        .unwrap_or(usize::MAX);
                    let new_index = current_index as i64 + delta;
                    if new_index >= 0
                        && new_index
                            < self.enabled_ids.as_ref().expect("checked above").len() as i64
                    {
                        self.enabled_ids = move_item(&self.enabled_ids, &item.full_id, delta);
                        self.is_dirty = true;
                        self.selected_index = (self.selected_index as i64 + delta).max(0) as usize;
                        self.refresh();
                        self.notify_change();
                    }
                }
            }
            return;
        }

        // Toggle on Enter
        if with_keybindings(|kb| kb.matches(data, "tui.select.confirm")) {
            if let Some(item) = self.filtered_items.get(self.selected_index).cloned() {
                self.enabled_ids = toggle(&self.enabled_ids, &self.all_ids, &item.full_id);
                self.is_dirty = true;
                self.refresh();
                self.notify_change();
            }
            return;
        }

        // Enable all (filtered if search active, otherwise all)
        if with_keybindings(|kb| kb.matches(data, "app.models.enableAll")) {
            let target_ids = if !self.search_input.value().is_empty() {
                Some(
                    self.filtered_items
                        .iter()
                        .map(|item| item.full_id.clone())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            };
            self.enabled_ids = enable_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return;
        }

        // Clear all (filtered if search active, otherwise all)
        if with_keybindings(|kb| kb.matches(data, "app.models.clearAll")) {
            let target_ids = if !self.search_input.value().is_empty() {
                Some(
                    self.filtered_items
                        .iter()
                        .map(|item| item.full_id.clone())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            };
            self.enabled_ids = clear_all(&self.enabled_ids, &self.all_ids, target_ids.as_deref());
            self.is_dirty = true;
            self.refresh();
            self.notify_change();
            return;
        }

        // Toggle provider of current item
        if with_keybindings(|kb| kb.matches(data, "app.models.toggleProvider")) {
            let item = self.filtered_items.get(self.selected_index).cloned();
            if let Some(item) = item {
                if let Some(model) = &item.model {
                    let provider = model.provider.clone();
                    let provider_ids: Vec<String> = self
                        .all_ids
                        .iter()
                        .filter(|id| {
                            self.model(id)
                                .map(|candidate| candidate.provider == provider)
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    let all_enabled = provider_ids
                        .iter()
                        .all(|id| is_enabled(&self.enabled_ids, id));
                    self.enabled_ids = if all_enabled {
                        clear_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                    } else {
                        enable_all(&self.enabled_ids, &self.all_ids, Some(&provider_ids))
                    };
                    self.is_dirty = true;
                    self.refresh();
                    self.notify_change();
                }
            }
            return;
        }

        // Save/persist to settings
        if with_keybindings(|kb| kb.matches(data, "app.models.save")) {
            let enabled_ids = self.enabled_ids.clone();
            (self.callbacks.on_persist)(&enabled_ids);
            self.is_dirty = false;
            let footer = self.get_footer_text();
            self.footer_text.set_text(&footer);
            return;
        }

        // Ctrl+C - clear search or cancel if empty
        if matches_key(data, "ctrl+c") {
            if !self.search_input.value().is_empty() {
                self.search_input.set_value("");
                self.refresh();
            } else {
                (self.callbacks.on_cancel)();
            }
            return;
        }

        // Escape - cancel
        if matches_key(data, "escape") {
            (self.callbacks.on_cancel)();
            return;
        }

        // Pass everything else to search input
        self.search_input.handle_input(data);
        self.refresh();
    }

    /// Upstream `getSearchInput`.
    pub fn search_input(&mut self) -> &mut Input {
        &mut self.search_input
    }

    /// Test seam: selected item full id.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn selected_full_id(&self) -> Option<String> {
        self.filtered_items
            .get(self.selected_index)
            .map(|item| item.full_id.clone())
    }
}

impl Component for ScopedModelsSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Header
        let title = theme_fg(
            &self.theme,
            "accent",
            &self.theme.bold("Model Configuration"),
        );
        lines.extend(Text::with_options(&title, 0, 0, None).render(width));
        let subtitle = theme_fg(
            &self.theme,
            "muted",
            &format!(
                "Session-only. {} to save to settings.",
                super::model_selector::key_display_text("app.models.save")
            ),
        );
        lines.extend(Text::with_options(&subtitle, 0, 0, None).render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Search input
        lines.extend(self.search_input.render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // List container
        for child in &mut self.list_children {
            match child {
                ListChild::Text(text) => lines.extend(text.render(width)),
                ListChild::Spacer(count) => lines.extend(spacer_lines(*count)),
            }
        }
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Refresh status
        if let Some(refresh_status_text) = &mut self.refresh_status_text {
            lines.extend(refresh_status_text.render(width));
        }
        // Footer
        lines.extend(self.footer_text.render(width));
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        lines
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.set_focused(focused);
    }
}
