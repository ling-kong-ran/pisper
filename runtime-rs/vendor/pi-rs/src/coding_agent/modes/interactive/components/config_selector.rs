//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/config-selector.ts` (942 lines,
//! sha256 `2a1f2594ff2f78aa2305f7128bacbd8c227d5bd4efa94b156f45a7d9268c0805`):
//! TUI component for managing package resources (enable/disable) across the
//! global and project write scopes.
//!
//! Slice conventions: see [`super::model_selector`] (theme seam, inline
//! composite rendering). Further disclosed substitutions:
//! - **SettingsManager surface**: the component talks to a
//!   [`ConfigSettingsStore`] trait (the member surface upstream uses);
//!   implemented by [`crate::coding_agent::core::settings_manager::SettingsManager`]
//!   and by in-memory test doubles. Project setters that can fail upstream
//!   propagate errors through `Result`; the component ignores them exactly
//!   like upstream (`void this.settingsManager.setProject...()`).
//! - **`node:path`**: basename/dirname/join/relative go through the ported
//!   [`crate::coding_agent::utils::node_path`] win32/posix implementations
//!   (host-platform flag, matching upstream's `process.platform` binding).
//! - **`localeCompare`**: mapped to plain lexicographic comparison
//!   (repo-wide convention for the deterministic core).

use std::collections::HashSet;
use std::sync::Arc;

use crate::coding_agent::core::settings_manager::{SettingsManager, SettingsValue};
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::coding_agent::package_manager::{
    PathMetadata, PathMetadataOrigin, ResolvedPaths, ResolvedResource, SourceScope,
};
use crate::coding_agent::utils::node_path;
use crate::coding_agent::utils::paths::{
    canonicalize_path, is_local_path, resolve_path_with, PathInputOptions,
};
use crate::tui::component::Component;
use crate::tui::components::input::{Input, InputOptions};
use crate::tui::keybindings::with_keybindings;
use crate::tui::keys::matches_key;
use crate::tui::utils::{truncate_to_width, visible_width};

use super::model_selector::{key_hint, raw_key_hint, spacer_lines, theme_fg, DynamicBorder};

/// Upstream `BUILTIN_PATH_PREFIX` (core/source-info.ts). The core constant
/// lives in the report-only ripple set; the selector keeps a local copy.
const BUILTIN_PATH_PREFIX: &str = "builtin:";

/// Upstream `ResourceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceType {
    Extensions,
    Skills,
    Prompts,
    Themes,
}

impl ResourceType {
    fn as_str(self) -> &'static str {
        match self {
            ResourceType::Extensions => "extensions",
            ResourceType::Skills => "skills",
            ResourceType::Prompts => "prompts",
            ResourceType::Themes => "themes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ResourceType::Extensions => "Extensions",
            ResourceType::Skills => "Skills",
            ResourceType::Prompts => "Prompts",
            ResourceType::Themes => "Themes",
        }
    }

    #[cfg(test)]
    #[allow(dead_code)] // exercised by the fixture-based assertions below
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "extensions" => Some(ResourceType::Extensions),
            "skills" => Some(ResourceType::Skills),
            "prompts" => Some(ResourceType::Prompts),
            "themes" => Some(ResourceType::Themes),
            _ => None,
        }
    }

    fn all() -> [ResourceType; 4] {
        [
            ResourceType::Extensions,
            ResourceType::Skills,
            ResourceType::Prompts,
            ResourceType::Themes,
        ]
    }
}

/// Upstream `ConfigWriteScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigWriteScope {
    Global,
    Project,
}

/// Upstream `SettingsScope` ("user" | "project").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsScope {
    User,
    Project,
}

/// Upstream `ProjectOverrideState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectOverrideState {
    Inherit,
    Load,
    Unload,
}

/// Upstream `ScopedResolvedPaths` (`Record<ConfigWriteScope, ResolvedPaths>`).
#[derive(Debug, Clone, Default)]
pub struct ScopedResolvedPaths {
    pub global: ResolvedPaths,
    pub project: ResolvedPaths,
}

/// The settings-manager surface config-selector uses. Implemented by
/// [`SettingsManager`] (delegating, ignoring project-setter errors like
/// upstream's fire-and-forget calls) and by test doubles.
pub trait ConfigSettingsStore {
    fn get_global_settings(&self) -> SettingsValue;
    fn get_project_settings(&self) -> SettingsValue;
    fn set_extension_paths(&self, paths: Vec<String>);
    fn set_skill_paths(&self, paths: Vec<String>);
    fn set_prompt_template_paths(&self, paths: Vec<String>);
    fn set_theme_paths(&self, paths: Vec<String>);
    fn set_project_extension_paths(&self, paths: Vec<String>);
    fn set_project_skill_paths(&self, paths: Vec<String>);
    fn set_project_prompt_template_paths(&self, paths: Vec<String>);
    fn set_project_theme_paths(&self, paths: Vec<String>);
    fn set_packages(&self, packages: Vec<SettingsValue>);
    fn set_project_packages(&self, packages: Vec<SettingsValue>);
}

impl ConfigSettingsStore for SettingsManager {
    fn get_global_settings(&self) -> SettingsValue {
        SettingsManager::get_global_settings(self)
    }
    fn get_project_settings(&self) -> SettingsValue {
        SettingsManager::get_project_settings(self)
    }
    fn set_extension_paths(&self, paths: Vec<String>) {
        SettingsManager::set_extension_paths(self, paths);
    }
    fn set_skill_paths(&self, paths: Vec<String>) {
        SettingsManager::set_skill_paths(self, paths);
    }
    fn set_prompt_template_paths(&self, paths: Vec<String>) {
        SettingsManager::set_prompt_template_paths(self, paths);
    }
    fn set_theme_paths(&self, paths: Vec<String>) {
        SettingsManager::set_theme_paths(self, paths);
    }
    fn set_project_extension_paths(&self, paths: Vec<String>) {
        let _ = SettingsManager::set_project_extension_paths(self, paths);
    }
    fn set_project_skill_paths(&self, paths: Vec<String>) {
        let _ = SettingsManager::set_project_skill_paths(self, paths);
    }
    fn set_project_prompt_template_paths(&self, paths: Vec<String>) {
        let _ = SettingsManager::set_project_prompt_template_paths(self, paths);
    }
    fn set_project_theme_paths(&self, paths: Vec<String>) {
        let _ = SettingsManager::set_project_theme_paths(self, paths);
    }
    fn set_packages(&self, packages: Vec<SettingsValue>) {
        SettingsManager::set_packages(self, packages);
    }
    fn set_project_packages(&self, packages: Vec<SettingsValue>) {
        let _ = SettingsManager::set_project_packages(self, packages);
    }
}

// ===========================================================================
// node:path helpers on the host platform
// ===========================================================================

fn windows_paths() -> bool {
    cfg!(windows)
}

fn host_relative(from: &str, to: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default();
    if windows_paths() {
        node_path::win32_relative(from, to, &cwd)
    } else {
        node_path::posix_relative(from, to, &cwd)
    }
}

fn host_join(parts: &[&str]) -> String {
    if windows_paths() {
        node_path::win32_join(parts)
    } else {
        node_path::posix_join(parts)
    }
}

fn host_basename(path: &str) -> String {
    let normalized = if windows_paths() {
        node_path::win32_normalize(path)
    } else {
        node_path::posix_normalize(path)
    };
    let separator = if windows_paths() { '\\' } else { '/' };
    normalized
        .rsplit(separator)
        .next()
        .unwrap_or(&normalized)
        .to_string()
}

fn host_dirname(path: &str) -> String {
    let normalized = if windows_paths() {
        node_path::win32_normalize(path)
    } else {
        node_path::posix_normalize(path)
    };
    let separator = if windows_paths() { '\\' } else { '/' };
    match normalized.rfind(separator) {
        Some(index) if index > 0 => normalized[..index].to_string(),
        Some(_) => {
            if normalized.starts_with(separator) {
                separator.to_string()
            } else {
                ".".to_string()
            }
        }
        None => ".".to_string(),
    }
}

fn homedir() -> String {
    if windows_paths() {
        std::env::var("USERPROFILE").unwrap_or_default()
    } else {
        std::env::var("HOME").unwrap_or_default()
    }
}

fn resolve_trimmed(input: &str, base_dir: &str) -> String {
    let options = PathInputOptions {
        trim: true,
        ..PathInputOptions::default()
    };
    resolve_path_with(input, base_dir, &options, windows_paths())
        .unwrap_or_else(|_| input.to_string())
}

// ===========================================================================
// Group building
// ===========================================================================

/// Upstream `formatBaseDir`.
fn format_base_dir(base_dir: &str) -> String {
    let home_dir = homedir();
    let display_path = if base_dir == home_dir {
        "~".to_string()
    } else if base_dir.starts_with(&home_dir) && !home_dir.is_empty() {
        let rest = &base_dir[home_dir.len()..];
        format!("~{}", rest.replace('\\', "/"))
    } else {
        base_dir.replace('\\', "/")
    };
    if display_path.ends_with('/') {
        display_path
    } else {
        format!("{display_path}/")
    }
}

fn scope_str(scope: SourceScope) -> &'static str {
    match scope {
        SourceScope::User => "user",
        SourceScope::Project => "project",
        SourceScope::Temporary => "temporary",
    }
}

/// Upstream `getGroupLabel`.
fn get_group_label(metadata: &PathMetadata, agent_dir: &str) -> String {
    if metadata.origin == PathMetadataOrigin::Package {
        return format!("{} ({})", metadata.source, scope_str(metadata.scope));
    }
    if metadata.source == "builtin" {
        return if metadata.scope == SourceScope::User {
            "Built-in".to_string()
        } else {
            "Built-in (project override)".to_string()
        };
    }
    if metadata.source == "auto" {
        if let Some(base_dir) = &metadata.base_dir {
            return if metadata.scope == SourceScope::User {
                format!("User ({})", format_base_dir(base_dir))
            } else {
                format!("Project ({})", format_base_dir(base_dir))
            };
        }
        return if metadata.scope == SourceScope::User {
            format!("User ({})", format_base_dir(agent_dir))
        } else {
            format!("Project ({}/)", crate::coding_agent::core::CONFIG_DIR_NAME)
        };
    }
    if metadata.scope == SourceScope::User {
        "User settings".to_string()
    } else {
        "Project settings".to_string()
    }
}

/// Upstream `ResourceItem`.
#[derive(Clone, Debug)]
pub struct ResourceItem {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
    pub resource_type: ResourceType,
    pub display_name: String,
    pub group_key: String,
    pub subgroup_key: String,
}

/// Upstream `ResourceSubgroup`.
#[derive(Clone, Debug)]
pub struct ResourceSubgroup {
    pub resource_type: ResourceType,
    pub label: String,
    pub items: Vec<ResourceItem>,
}

/// Upstream `ResourceGroup`.
#[derive(Clone, Debug)]
pub struct ResourceGroup {
    pub key: String,
    pub label: String,
    pub scope: SourceScope,
    pub origin: PathMetadataOrigin,
    pub source: String,
    pub subgroups: Vec<ResourceSubgroup>,
}

/// Upstream `buildGroups`.
pub fn build_groups(resolved: &ResolvedPaths, agent_dir: &str) -> Vec<ResourceGroup> {
    let mut groups: Vec<ResourceGroup> = Vec::new();

    let add_to_group = |resources: &[ResolvedResource],
                        resource_type: ResourceType,
                        groups: &mut Vec<ResourceGroup>| {
        for res in resources {
            let metadata = &res.metadata;
            let group_key = format!(
                "{}:{}:{}:{}",
                match metadata.origin {
                    PathMetadataOrigin::Package => "package",
                    PathMetadataOrigin::TopLevel => "top-level",
                },
                scope_str(metadata.scope),
                metadata.source,
                metadata.base_dir.as_deref().unwrap_or("")
            );

            if !groups.iter().any(|group| group.key == group_key) {
                groups.push(ResourceGroup {
                    key: group_key.clone(),
                    label: get_group_label(metadata, agent_dir),
                    scope: metadata.scope,
                    origin: metadata.origin,
                    source: metadata.source.clone(),
                    subgroups: Vec::new(),
                });
            }

            let group = groups
                .iter_mut()
                .find(|group| group.key == group_key)
                .expect("inserted above");
            let subgroup_key = format!("{group_key}:{}", resource_type.as_str());

            let subgroup = match group
                .subgroups
                .iter_mut()
                .find(|subgroup| subgroup.resource_type == resource_type)
            {
                Some(subgroup) => subgroup,
                None => {
                    group.subgroups.push(ResourceSubgroup {
                        resource_type,
                        label: resource_type.label().to_string(),
                        items: Vec::new(),
                    });
                    group.subgroups.last_mut().expect("just pushed")
                }
            };

            let file_name = host_basename(&res.path);
            let parent_folder = host_basename(&host_dirname(&res.path));
            let display_name = if metadata.source == "builtin" {
                // Upstream `path.slice(BUILTIN_PATH_PREFIX.length)`.
                res.path
                    .strip_prefix(BUILTIN_PATH_PREFIX)
                    .unwrap_or(&res.path)
                    .to_string()
            } else if resource_type == ResourceType::Extensions && parent_folder != "extensions" {
                format!("{parent_folder}/{file_name}")
            } else if resource_type == ResourceType::Skills && file_name == "SKILL.md" {
                parent_folder
            } else {
                file_name
            };
            subgroup.items.push(ResourceItem {
                path: res.path.clone(),
                enabled: res.enabled,
                metadata: metadata.clone(),
                resource_type,
                display_name,
                group_key,
                subgroup_key,
            });
        }
    };

    add_to_group(&resolved.extensions, ResourceType::Extensions, &mut groups);
    add_to_group(&resolved.skills, ResourceType::Skills, &mut groups);
    add_to_group(&resolved.prompts, ResourceType::Prompts, &mut groups);
    add_to_group(&resolved.themes, ResourceType::Themes, &mut groups);

    // Sort groups: packages first, then top-level; user before project.
    groups.sort_by(|a, b| {
        if a.origin != b.origin {
            return if a.origin == PathMetadataOrigin::Package {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if a.scope != b.scope {
            return if a.scope == SourceScope::User {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.source.cmp(&b.source)
    });

    // Sort subgroups within each group by type order, and items by name.
    let type_order = |resource_type: ResourceType| match resource_type {
        ResourceType::Extensions => 0,
        ResourceType::Skills => 1,
        ResourceType::Prompts => 2,
        ResourceType::Themes => 3,
    };
    for group in &mut groups {
        group
            .subgroups
            .sort_by_key(|subgroup| type_order(subgroup.resource_type));
        for subgroup in &mut group.subgroups {
            subgroup
                .items
                .sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }
    }

    groups
}

/// Upstream `FlatEntry`.
#[derive(Clone, Debug)]
enum FlatEntry {
    Group(usize),
    Subgroup {
        group_index: usize,
        subgroup_index: usize,
    },
    Item {
        group_index: usize,
        subgroup_index: usize,
        item_index: usize,
    },
}

/// Upstream `ConfigSelectorHeader`.
struct ConfigSelectorHeader {
    theme: Arc<Theme>,
    write_scope: ConfigWriteScope,
    project_mode_available: bool,
}

impl ConfigSelectorHeader {
    fn render(&self, width: usize) -> Vec<String> {
        let title = self
            .theme
            .bold(if self.write_scope == ConfigWriteScope::Project {
                "Project Local Resources"
            } else {
                "Global Resources"
            });
        let separator = theme_fg(&self.theme, "muted", " · ");
        let switch_hint = if self.project_mode_available {
            key_hint(&self.theme, "tui.input.tab", "switch mode") + &separator
        } else {
            String::new()
        };
        let action_hint = if self.write_scope == ConfigWriteScope::Project {
            raw_key_hint(&self.theme, "space", "cycle inherit/+/-")
        } else {
            raw_key_hint(&self.theme, "space", "toggle")
        };
        let hint = format!(
            "{switch_hint}{action_hint}{separator}{}",
            raw_key_hint(&self.theme, "esc", "close")
        );
        let spacing = 1.max(
            width
                .saturating_sub(visible_width(&title))
                .saturating_sub(visible_width(&hint)),
        );
        let scope_hint = if self.write_scope == ConfigWriteScope::Project {
            theme_fg(
                &self.theme,
                "muted",
                &format!(
                    "{}/settings.json · inherited global resources are dimmed",
                    crate::coding_agent::core::CONFIG_DIR_NAME
                ),
            )
        } else {
            theme_fg(
                &self.theme,
                "muted",
                &format!(
                    "~/{}/agent/settings.json",
                    crate::coding_agent::core::CONFIG_DIR_NAME
                ),
            )
        };

        vec![
            truncate_to_width(
                &format!("{title}{}{hint}", " ".repeat(spacing)),
                width,
                "",
                false,
            ),
            truncate_to_width(&scope_hint, width, "", false),
        ]
    }
}

/// Shared request-render callback (upstream closes over `requestRender` from
/// several event sinks).
pub type RenderFn = Arc<std::sync::Mutex<Box<dyn FnMut() + Send>>>;

/// Upstream `ResourceList`.
pub struct ResourceList {
    theme: Arc<Theme>,
    groups_by_scope: [Vec<ResourceGroup>; 2],
    flat_items: Vec<FlatEntry>,
    filtered_items: Vec<FlatEntry>,
    selected_index: usize,
    search_input: Input,
    max_visible: usize,
    settings: Box<dyn ConfigSettingsStore + Send>,
    cwd: String,
    agent_dir: String,
    write_scope: ConfigWriteScope,
    project_mode_available: bool,
    inherited_enabled_by_key: Vec<(String, bool)>,
    request_render: RenderFn,

    pub on_cancel: Option<Box<dyn FnMut() + Send>>,
    pub on_exit: Option<Box<dyn FnMut() + Send>>,
    pub on_toggle: Option<Box<dyn FnMut(&ResourceItem, bool) + Send>>,
    focused: bool,
}

impl ResourceList {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        groups_by_scope: [Vec<ResourceGroup>; 2],
        settings: Box<dyn ConfigSettingsStore + Send>,
        cwd: &str,
        agent_dir: &str,
        terminal_height: Option<usize>,
        write_scope: ConfigWriteScope,
        project_mode_available: bool,
        request_render: RenderFn,
    ) -> Self {
        let inherited_enabled_by_key = Self::build_inherited_enabled_map(&groups_by_scope[0]);
        let mut list = Self {
            theme,
            groups_by_scope,
            flat_items: Vec::new(),
            filtered_items: Vec::new(),
            selected_index: 0,
            search_input: Input::new(InputOptions::default()),
            // 8 lines of chrome: top spacer + top border + spacer + header
            // (2 lines) + spacer + bottom spacer + bottom border
            max_visible: 5.max(terminal_height.unwrap_or(24).saturating_sub(8)),
            settings,
            cwd: cwd.to_string(),
            agent_dir: agent_dir.to_string(),
            write_scope,
            project_mode_available,
            inherited_enabled_by_key,
            request_render,
            on_cancel: None,
            on_exit: None,
            on_toggle: None,
            focused: false,
        };
        list.build_flat_list();
        list.filtered_items = list.flat_items.clone();
        list
    }

    /// The list's current write scope (mirrors the header's displayed scope).
    pub fn write_scope(&self) -> ConfigWriteScope {
        self.write_scope
    }

    pub fn set_write_scope(&mut self, write_scope: ConfigWriteScope) {
        self.write_scope = write_scope;
        self.build_flat_list();
        let query = self.search_input.value().to_string();
        self.filter_items(&query);
    }

    fn groups(&self) -> &Vec<ResourceGroup> {
        match self.write_scope {
            ConfigWriteScope::Global => &self.groups_by_scope[0],
            ConfigWriteScope::Project => &self.groups_by_scope[1],
        }
    }

    fn groups_mut(&mut self) -> &mut Vec<ResourceGroup> {
        match self.write_scope {
            ConfigWriteScope::Global => &mut self.groups_by_scope[0],
            ConfigWriteScope::Project => &mut self.groups_by_scope[1],
        }
    }

    fn resource_item_key(item: &ResourceItem) -> String {
        format!(
            "{}:{}",
            item.resource_type.as_str(),
            canonicalize_path(&item.path)
        )
    }

    fn build_inherited_enabled_map(groups: &[ResourceGroup]) -> Vec<(String, bool)> {
        let mut result = Vec::new();
        for group in groups {
            for subgroup in &group.subgroups {
                for item in &subgroup.items {
                    result.push((Self::resource_item_key(item), item.enabled));
                }
            }
        }
        result
    }

    fn build_flat_list(&mut self) {
        self.flat_items.clear();
        let group_count = self.groups().len();
        for group_index in 0..group_count {
            self.flat_items.push(FlatEntry::Group(group_index));
            let subgroup_count = self.groups()[group_index].subgroups.len();
            for subgroup_index in 0..subgroup_count {
                self.flat_items.push(FlatEntry::Subgroup {
                    group_index,
                    subgroup_index,
                });
                let item_count = self.groups()[group_index].subgroups[subgroup_index]
                    .items
                    .len();
                for item_index in 0..item_count {
                    self.flat_items.push(FlatEntry::Item {
                        group_index,
                        subgroup_index,
                        item_index,
                    });
                }
            }
        }
        // Start selection on first item (not header)
        self.selected_index = self
            .flat_items
            .iter()
            .position(|entry| matches!(entry, FlatEntry::Item { .. }))
            .unwrap_or(0);
    }

    fn entry_is_item(&self, index: usize) -> bool {
        matches!(self.filtered_items.get(index), Some(FlatEntry::Item { .. }))
    }

    fn find_next_item(&self, from_index: usize, direction: i64) -> usize {
        let mut index = from_index as i64 + direction;
        while index >= 0 && (index as usize) < self.filtered_items.len() {
            if self.entry_is_item(index as usize) {
                return index as usize;
            }
            index += direction;
        }
        from_index
    }

    fn filter_items(&mut self, query: &str) {
        if query.trim().is_empty() {
            self.filtered_items = self.flat_items.clone();
            self.select_first_item();
            return;
        }

        let lower_query = query.to_lowercase();
        let mut matching_items: HashSet<String> = HashSet::new();
        for entry in &self.flat_items {
            if let FlatEntry::Item {
                group_index,
                subgroup_index,
                item_index,
            } = entry
            {
                let item = &self.groups_by_scope_lookup(*group_index).subgroups[*subgroup_index]
                    .items[*item_index];
                if item.display_name.to_lowercase().contains(&lower_query)
                    || item
                        .resource_type
                        .as_str()
                        .to_lowercase()
                        .contains(&lower_query)
                    || item.path.to_lowercase().contains(&lower_query)
                {
                    matching_items.insert(Self::resource_item_key(item));
                }
            }
        }

        // Find which subgroups and groups contain matching items
        let mut matching_subgroups: HashSet<String> = HashSet::new();
        let mut matching_groups: HashSet<String> = HashSet::new();
        for group in self.groups() {
            for subgroup in &group.subgroups {
                for item in &subgroup.items {
                    if matching_items.contains(&Self::resource_item_key(item)) {
                        matching_subgroups.insert(subgroup_key(group, subgroup));
                        matching_groups.insert(group.key.clone());
                    }
                }
            }
        }

        self.filtered_items = self
            .flat_items
            .iter()
            .filter(|entry| match entry {
                FlatEntry::Group(group_index) => {
                    matching_groups.contains(&self.groups_by_scope_lookup(*group_index).key)
                }
                FlatEntry::Subgroup {
                    group_index,
                    subgroup_index,
                } => matching_subgroups.contains(&subgroup_key(
                    self.groups_by_scope_lookup(*group_index),
                    &self.groups_by_scope_lookup(*group_index).subgroups[*subgroup_index],
                )),
                FlatEntry::Item {
                    group_index,
                    subgroup_index,
                    item_index,
                } => matching_items.contains(&Self::resource_item_key(
                    &self.groups_by_scope_lookup(*group_index).subgroups[*subgroup_index].items
                        [*item_index],
                )),
            })
            .cloned()
            .collect();

        self.select_first_item();
    }

    fn groups_by_scope_lookup(&self, group_index: usize) -> &ResourceGroup {
        // Flat entries always reference the ACTIVE scope's groups; but search
        // runs over the active scope only, so the lookup is the same vector
        // `groups()` returns. Kept as a method to make index use explicit.
        self.groups()
            .get(group_index)
            .or_else(|| self.groups().last())
            .expect("group index in range")
    }

    fn select_first_item(&mut self) {
        let first_item_index = self
            .filtered_items
            .iter()
            .position(|entry| matches!(entry, FlatEntry::Item { .. }));
        self.selected_index = first_item_index.unwrap_or(0);
    }

    /// Upstream `updateItem`.
    pub fn update_item(&mut self, item: &ResourceItem, enabled: bool) {
        if let Some(FlatEntry::Item {
            group_index,
            subgroup_index,
            item_index,
        }) = self.filtered_items.get(self.selected_index).cloned()
        {
            let target =
                &mut self.groups_mut()[group_index].subgroups[subgroup_index].items[item_index];
            if target.path == item.path && target.resource_type == item.resource_type {
                target.enabled = enabled;
                return;
            }
        }
        // Fall back to the upstream group scan.
        for group in self.groups_mut() {
            for subgroup in &mut group.subgroups {
                if let Some(found) = subgroup.items.iter_mut().find(|candidate| {
                    candidate.path == item.path && candidate.resource_type == item.resource_type
                }) {
                    found.enabled = enabled;
                    return;
                }
            }
        }
    }

    fn item_at(&self, entry: &FlatEntry) -> Option<&ResourceItem> {
        match entry {
            FlatEntry::Item {
                group_index,
                subgroup_index,
                item_index,
            } => Some(&self.groups()[*group_index].subgroups[*subgroup_index].items[*item_index]),
            _ => None,
        }
    }

    /// Upstream `render`.
    pub fn render_list(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        // Search input
        lines.extend(self.search_input.render(width));
        lines.push(String::new());

        if self.filtered_items.is_empty() {
            lines.push(theme_fg(&self.theme, "muted", "  No resources found"));
            return lines;
        }

        let start_index = self
            .selected_index
            .saturating_sub(self.max_visible / 2)
            .min(self.filtered_items.len().saturating_sub(self.max_visible));
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());

        for i in start_index..end_index {
            let Some(entry) = self.filtered_items.get(i).cloned() else {
                continue;
            };
            let is_selected = i == self.selected_index;

            match &entry {
                FlatEntry::Group(group_index) => {
                    let group = &self.groups()[*group_index];
                    let inherited = self.write_scope == ConfigWriteScope::Project
                        && group.scope == SourceScope::User;
                    let label = self.theme.bold(&format!(
                        "{}{}",
                        group.label,
                        if inherited {
                            " · inherited global"
                        } else {
                            ""
                        }
                    ));
                    let group_line = theme_fg(
                        &self.theme,
                        if inherited { "dim" } else { "accent" },
                        &label,
                    );
                    lines.push(truncate_to_width(
                        &format!("  {group_line}"),
                        width,
                        "",
                        false,
                    ));
                }
                FlatEntry::Subgroup {
                    group_index,
                    subgroup_index,
                } => {
                    let color = if self.write_scope == ConfigWriteScope::Project
                        && self.groups()[*group_index].scope == SourceScope::User
                    {
                        "dim"
                    } else {
                        "muted"
                    };
                    let subgroup_line = theme_fg(
                        &self.theme,
                        color,
                        &self.groups()[*group_index].subgroups[*subgroup_index].label,
                    );
                    lines.push(truncate_to_width(
                        &format!("    {subgroup_line}"),
                        width,
                        "",
                        false,
                    ));
                }
                FlatEntry::Item { .. } => {
                    let item = self.item_at(&entry).expect("item entry").clone();
                    let cursor = if is_selected { "> " } else { "  " };
                    let dimmed = self.is_dimmed_item(&item);
                    let name_text = if is_selected && !dimmed {
                        self.theme.bold(&item.display_name)
                    } else {
                        item.display_name.clone()
                    };
                    let name = if dimmed {
                        theme_fg(&self.theme, "dim", &name_text)
                    } else {
                        name_text
                    };
                    lines.push(truncate_to_width(
                        &format!(
                            "{cursor}    {} {name}{}",
                            self.render_checkbox(&item),
                            self.get_item_suffix(&item)
                        ),
                        width,
                        "...",
                        false,
                    ));
                }
            }
        }

        // Scroll indicator
        if start_index > 0 || end_index < self.filtered_items.len() {
            let item_count = self
                .filtered_items
                .iter()
                .filter(|entry| matches!(entry, FlatEntry::Item { .. }))
                .count();
            let current_item_index = self
                .filtered_items
                .iter()
                .take(self.selected_index)
                .filter(|entry| matches!(entry, FlatEntry::Item { .. }))
                .count()
                + 1;
            lines.push(theme_fg(
                &self.theme,
                "dim",
                &format!("  ({current_item_index}/{item_count})"),
            ));
        }

        lines
    }

    /// Upstream `handleInput`.
    pub fn handle_input(&mut self, data: &str) {
        if with_keybindings(|kb| kb.matches(data, "tui.select.up")) {
            self.selected_index = self.find_next_item(self.selected_index, -1);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.select.down")) {
            self.selected_index = self.find_next_item(self.selected_index, 1);
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.select.pageUp")) {
            let mut target = self.selected_index.saturating_sub(self.max_visible);
            while target < self.filtered_items.len() && !self.entry_is_item(target) {
                target += 1;
            }
            if target < self.filtered_items.len() {
                self.selected_index = target;
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.select.pageDown")) {
            let mut target =
                (self.selected_index + self.max_visible).min(self.filtered_items.len() - 1);
            while target > 0 && !self.entry_is_item(target) {
                target -= 1;
            }
            if self.entry_is_item(target) {
                self.selected_index = target;
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.select.cancel")) {
            if let Some(on_cancel) = &mut self.on_cancel {
                (on_cancel)();
            }
            return;
        }
        if matches_key(data, "ctrl+c") {
            if let Some(on_exit) = &mut self.on_exit {
                (on_exit)();
            }
            return;
        }
        if with_keybindings(|kb| kb.matches(data, "tui.input.tab")) {
            // Upstream: `onSwitchMode = () => { this.switchWriteScope();
            // requestRender(); }` — the port performs the scope switch here
            // (the list owns the groups) and fires the shared render hook.
            if self.project_mode_available {
                self.set_write_scope(if self.write_scope == ConfigWriteScope::Global {
                    ConfigWriteScope::Project
                } else {
                    ConfigWriteScope::Global
                });
                let mut render = self.request_render.lock().expect("render fn");
                (render)();
            }
            return;
        }
        if data == " " || with_keybindings(|kb| kb.matches(data, "tui.select.confirm")) {
            let entry = self.filtered_items.get(self.selected_index).cloned();
            if let Some(entry) = entry {
                if let Some(item) = self.item_at(&entry).cloned() {
                    let allowed = self.write_scope == ConfigWriteScope::Project
                        || self.get_item_scope(&item) == SettingsScope::User;
                    if allowed {
                        if let Some(new_enabled) = self.toggle_resource(&item) {
                            self.update_item(&item, new_enabled);
                            if let Some(on_toggle) = &mut self.on_toggle {
                                (on_toggle)(&item, new_enabled);
                            }
                        }
                    }
                }
            }
            return;
        }

        // Pass to search input
        self.search_input.handle_input(data);
        let query = self.search_input.value().to_string();
        self.filter_items(&query);
    }

    fn toggle_resource(&mut self, item: &ResourceItem) -> Option<bool> {
        if self.write_scope == ConfigWriteScope::Project {
            let state = self.get_next_override_state(item);
            if !self.set_project_resource_override(item, state) {
                return None;
            }
            return Some(match state {
                ProjectOverrideState::Inherit => self.get_inherited_enabled(item),
                ProjectOverrideState::Load => true,
                ProjectOverrideState::Unload => false,
            });
        }

        let enabled = !item.enabled;
        if item.metadata.origin == PathMetadataOrigin::TopLevel {
            self.toggle_top_level_resource(item, enabled);
        } else {
            self.toggle_package_resource(item, enabled);
        }
        Some(enabled)
    }

    fn toggle_top_level_resource(&mut self, item: &ResourceItem, enabled: bool) {
        let scope = self.get_item_scope(item);
        let settings = match scope {
            SettingsScope::Project => self.settings.get_project_settings(),
            SettingsScope::User => self.settings.get_global_settings(),
        };

        let current = settings
            .get(item.resource_type.as_str())
            .and_then(SettingsValue::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(SettingsValue::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let pattern = self.get_resource_pattern(item);
        let disable_pattern = format!("-{pattern}");
        let enable_pattern = format!("+{pattern}");

        let updated: Vec<String> = current
            .into_iter()
            .filter(|p| {
                let stripped = p
                    .strip_prefix('!')
                    .or_else(|| p.strip_prefix('+'))
                    .or_else(|| p.strip_prefix('-'))
                    .unwrap_or(p);
                stripped != pattern
            })
            .collect();

        let mut updated = updated;
        if enabled {
            updated.push(enable_pattern);
        } else {
            updated.push(disable_pattern);
        }

        self.set_top_level_paths(scope, item.resource_type, updated);
    }

    fn set_top_level_paths(&mut self, scope: SettingsScope, key: ResourceType, paths: Vec<String>) {
        match (scope, key) {
            (SettingsScope::Project, ResourceType::Extensions) => {
                self.settings.set_project_extension_paths(paths)
            }
            (SettingsScope::Project, ResourceType::Skills) => {
                self.settings.set_project_skill_paths(paths)
            }
            (SettingsScope::Project, ResourceType::Prompts) => {
                self.settings.set_project_prompt_template_paths(paths)
            }
            (SettingsScope::Project, ResourceType::Themes) => {
                self.settings.set_project_theme_paths(paths)
            }
            (SettingsScope::User, ResourceType::Extensions) => {
                self.settings.set_extension_paths(paths)
            }
            (SettingsScope::User, ResourceType::Skills) => self.settings.set_skill_paths(paths),
            (SettingsScope::User, ResourceType::Prompts) => {
                self.settings.set_prompt_template_paths(paths)
            }
            (SettingsScope::User, ResourceType::Themes) => self.settings.set_theme_paths(paths),
        }
    }

    fn toggle_package_resource(&mut self, item: &ResourceItem, enabled: bool) {
        let scope = self.get_item_scope(item);
        let settings = match scope {
            SettingsScope::Project => self.settings.get_project_settings(),
            SettingsScope::User => self.settings.get_global_settings(),
        };

        let mut packages = settings
            .get("packages")
            .and_then(SettingsValue::as_array)
            .map(|array| array.to_vec())
            .unwrap_or_default();

        let pkg_index = packages
            .iter()
            .position(|pkg| package_source_string(pkg) == item.metadata.source);

        let Some(pkg_index) = pkg_index else {
            return;
        };

        // Convert string to object form if needed
        let mut pkg = match packages[pkg_index].clone() {
            SettingsValue::Str(source) => package_object(&source, None),
            object @ SettingsValue::Obj(_) => object,
            other => other,
        };

        let current = pkg
            .get(item.resource_type.as_str())
            .and_then(SettingsValue::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(SettingsValue::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let pattern = self.get_package_resource_pattern(item);
        let disable_pattern = format!("-{pattern}");
        let enable_pattern = format!("+{pattern}");

        let updated: Vec<String> = current
            .into_iter()
            .filter(|p| {
                let stripped = p
                    .strip_prefix('!')
                    .or_else(|| p.strip_prefix('+'))
                    .or_else(|| p.strip_prefix('-'))
                    .unwrap_or(p);
                stripped != pattern
            })
            .collect();

        let mut updated = updated;
        if enabled {
            updated.push(enable_pattern);
        } else {
            updated.push(disable_pattern);
        }

        if updated.is_empty() {
            pkg.remove(item.resource_type.as_str());
        } else {
            pkg.set(
                item.resource_type.as_str(),
                SettingsValue::Arr(
                    updated
                        .iter()
                        .map(|entry| SettingsValue::Str(entry.clone()))
                        .collect(),
                ),
            );
        }

        // Clean up empty filter object
        let has_filters = ResourceType::all()
            .iter()
            .any(|key| pkg.get(key.as_str()).is_some());
        if !has_filters {
            let source = pkg
                .get("source")
                .and_then(SettingsValue::as_str)
                .unwrap_or_default()
                .to_string();
            packages[pkg_index] = SettingsValue::Str(source);
        } else {
            packages[pkg_index] = pkg;
        }

        match scope {
            SettingsScope::Project => self.settings.set_project_packages(packages),
            SettingsScope::User => self.settings.set_packages(packages),
        }
    }

    fn render_checkbox(&self, item: &ResourceItem) -> String {
        if self.write_scope == ConfigWriteScope::Project {
            let state = self.get_project_override_state(item);
            if state == ProjectOverrideState::Load {
                return theme_fg(&self.theme, "success", "[+]");
            }
            if state == ProjectOverrideState::Unload {
                return theme_fg(&self.theme, "warning", "[-]");
            }
            return theme_fg(&self.theme, "dim", if item.enabled { "[x]" } else { "[ ]" });
        }
        if item.enabled {
            theme_fg(&self.theme, "success", "[x]")
        } else {
            theme_fg(&self.theme, "dim", "[ ]")
        }
    }

    fn get_item_suffix(&self, item: &ResourceItem) -> String {
        if self.write_scope != ConfigWriteScope::Project {
            return String::new();
        }
        let state = self.get_project_override_state(item);
        if state == ProjectOverrideState::Load {
            return theme_fg(&self.theme, "muted", "  project load");
        }
        if state == ProjectOverrideState::Unload {
            return theme_fg(&self.theme, "muted", "  project unload");
        }
        if self.is_inherited_global_item(item) {
            theme_fg(&self.theme, "dim", "  inherited global")
        } else {
            String::new()
        }
    }

    fn is_dimmed_item(&self, item: &ResourceItem) -> bool {
        self.write_scope == ConfigWriteScope::Project
            && self.is_inherited_global_item(item)
            && self.get_project_override_state(item) == ProjectOverrideState::Inherit
    }

    fn set_project_resource_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        if item.metadata.origin == PathMetadataOrigin::TopLevel {
            self.set_project_top_level_override(item, state)
        } else {
            self.set_project_package_override(item, state)
        }
    }

    fn set_project_top_level_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        let current = self
            .settings
            .get_project_settings()
            .get(item.resource_type.as_str())
            .and_then(SettingsValue::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(SettingsValue::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let pattern = if self.is_inherited_global_item(item) {
            item.path.clone()
        } else {
            self.get_resource_pattern_for_scope(item, SettingsScope::Project)
        };
        let patterns = self.get_top_level_override_patterns(item, SettingsScope::Project);
        let updated: Vec<String> = current
            .into_iter()
            .filter(|entry| {
                let target = get_pattern_entry_target(entry);
                if (entry.starts_with('!') || entry.starts_with('+') || entry.starts_with('-'))
                    && patterns.contains(target)
                {
                    return false;
                }
                !(state == ProjectOverrideState::Inherit
                    && self.is_inherited_global_item(item)
                    && target == pattern)
            })
            .collect();
        let mut updated = updated;
        if state != ProjectOverrideState::Inherit {
            // Project entries name inherited files to override them. Built-in
            // paths need no entry.
            if self.is_inherited_global_item(item)
                && item.metadata.source != "builtin"
                && !updated.contains(&pattern)
            {
                updated.push(pattern.clone());
            }
            updated.push(format!(
                "{}{pattern}",
                if state == ProjectOverrideState::Load {
                    "+"
                } else {
                    "-"
                }
            ));
        }
        self.set_top_level_paths(SettingsScope::Project, item.resource_type, updated);
        true
    }

    fn set_project_package_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        let mut packages = self
            .settings
            .get_project_settings()
            .get("packages")
            .and_then(SettingsValue::as_array)
            .map(|array| array.to_vec())
            .unwrap_or_default();
        let mut pkg_index = packages.iter().position(|pkg| {
            let right = package_source_string(pkg);
            self.package_source_string_matches(
                &item.metadata.source,
                self.get_item_scope(item),
                &right,
                SettingsScope::Project,
            )
        });
        if pkg_index.is_none() {
            if state == ProjectOverrideState::Inherit {
                return false;
            }
            packages.push(self.create_package_override_source(item));
            pkg_index = Some(packages.len() - 1);
        }
        let pkg_index = pkg_index.expect("set above");
        let mut pkg = match packages[pkg_index].clone() {
            SettingsValue::Str(source) => package_object(&source, None),
            object @ SettingsValue::Obj(_) => object,
            other => other,
        };
        let pattern = self.get_package_resource_pattern(item);
        let current = pkg
            .get(item.resource_type.as_str())
            .and_then(SettingsValue::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(SettingsValue::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated: Vec<String> = current
            .into_iter()
            .filter(|entry| get_pattern_entry_target(entry) != pattern)
            .collect();
        let mut updated = updated;
        if state != ProjectOverrideState::Inherit {
            updated.push(format!(
                "{}{pattern}",
                if state == ProjectOverrideState::Load {
                    "+"
                } else {
                    "-"
                }
            ));
        }
        if updated.is_empty() {
            pkg.remove(item.resource_type.as_str());
        } else {
            pkg.set(
                item.resource_type.as_str(),
                SettingsValue::Arr(
                    updated
                        .iter()
                        .map(|entry| SettingsValue::Str(entry.clone()))
                        .collect(),
                ),
            );
        }
        if !ResourceType::all()
            .iter()
            .any(|key| pkg.get(key.as_str()).is_some())
        {
            let autoload_false =
                pkg.get("autoload").and_then(SettingsValue::as_bool) == Some(false);
            if autoload_false {
                packages.remove(pkg_index);
            } else {
                let source = pkg
                    .get("source")
                    .and_then(SettingsValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                packages[pkg_index] = SettingsValue::Str(source);
            }
        } else {
            packages[pkg_index] = pkg;
        }
        self.settings.set_project_packages(packages);
        true
    }

    fn get_next_override_state(&self, item: &ResourceItem) -> ProjectOverrideState {
        let state = self.get_project_override_state(item);
        let inherited_enabled = self.get_inherited_enabled(item);
        if state == ProjectOverrideState::Inherit {
            return if inherited_enabled {
                ProjectOverrideState::Unload
            } else {
                ProjectOverrideState::Load
            };
        }
        if state == ProjectOverrideState::Unload {
            return if inherited_enabled {
                ProjectOverrideState::Load
            } else {
                ProjectOverrideState::Inherit
            };
        }
        if inherited_enabled {
            ProjectOverrideState::Inherit
        } else {
            ProjectOverrideState::Unload
        }
    }

    fn get_project_override_state(&self, item: &ResourceItem) -> ProjectOverrideState {
        if self.write_scope != ConfigWriteScope::Project {
            return ProjectOverrideState::Inherit;
        }
        if item.metadata.origin == PathMetadataOrigin::TopLevel {
            let entries = self
                .settings
                .get_project_settings()
                .get(item.resource_type.as_str())
                .and_then(SettingsValue::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(SettingsValue::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return self.get_override_state_from_entries(
                &entries,
                &self.get_top_level_override_patterns(item, SettingsScope::Project),
                false,
            );
        }
        let pkg = self.find_matching_package_source(item, SettingsScope::Project);
        let Some(pkg) = pkg else {
            return ProjectOverrideState::Inherit;
        };
        let entries = pkg
            .get(item.resource_type.as_str())
            .and_then(SettingsValue::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(SettingsValue::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
        let Some(entries) = entries else {
            return ProjectOverrideState::Inherit;
        };
        let autoload_false = pkg.get("autoload").and_then(SettingsValue::as_bool) == Some(false);
        self.get_override_state_from_entries(
            &entries,
            &HashSet::from([self.get_package_resource_pattern(item)]),
            !autoload_false,
        )
    }

    fn get_override_state_from_entries(
        &self,
        entries: &[String],
        patterns: &HashSet<String>,
        empty_array_is_unload: bool,
    ) -> ProjectOverrideState {
        if entries.is_empty() && empty_array_is_unload {
            return ProjectOverrideState::Unload;
        }
        let mut state = ProjectOverrideState::Inherit;
        for entry in entries {
            if !patterns.contains(get_pattern_entry_target(entry)) {
                continue;
            }
            if entry.starts_with('!') || entry.starts_with('-') {
                state = ProjectOverrideState::Unload;
            } else {
                state = ProjectOverrideState::Load;
            }
        }
        state
    }

    fn get_inherited_enabled(&self, item: &ResourceItem) -> bool {
        let key = Self::resource_item_key(item);
        match self
            .inherited_enabled_by_key
            .iter()
            .find(|(stored_key, _)| *stored_key == key)
        {
            Some((_, enabled)) => *enabled,
            None => {
                if self.get_item_scope(item) == SettingsScope::User {
                    item.enabled
                } else {
                    true
                }
            }
        }
    }

    fn is_inherited_global_item(&self, item: &ResourceItem) -> bool {
        self.get_item_scope(item) == SettingsScope::User
            || self
                .inherited_enabled_by_key
                .iter()
                .any(|(key, _)| *key == Self::resource_item_key(item))
    }

    fn get_top_level_override_patterns(
        &self,
        item: &ResourceItem,
        scope: SettingsScope,
    ) -> HashSet<String> {
        let base_dir = self.get_top_level_base_dir(scope);
        let mut patterns = HashSet::new();
        patterns.insert(self.get_resource_pattern_for_scope(item, scope));
        patterns.insert(item.path.clone());
        patterns.insert(host_relative(&base_dir, &item.path));
        if let Some(metadata_base_dir) = &item.metadata.base_dir {
            patterns.insert(host_relative(metadata_base_dir, &item.path));
        }
        patterns
    }

    fn get_resource_pattern_for_scope(&self, item: &ResourceItem, scope: SettingsScope) -> String {
        let source_scope = self.get_item_scope(item);
        if scope != source_scope || item.metadata.source == "builtin" {
            return item.path.clone();
        }
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| self.get_top_level_base_dir(source_scope));
        host_relative(&base_dir, &item.path)
    }

    fn create_package_override_source(&self, item: &ResourceItem) -> SettingsValue {
        let source = &item.metadata.source;
        if !is_local_path(source) {
            return package_object(source, Some(false));
        }
        let source_path = resolve_trimmed(
            source,
            &self.get_top_level_base_dir(self.get_item_scope(item)),
        );
        let relative = host_relative(
            &self.get_top_level_base_dir(SettingsScope::Project),
            &source_path,
        );
        package_object(
            if relative.is_empty() { "." } else { &relative },
            Some(false),
        )
    }

    fn package_source_string_matches(
        &self,
        left_source: &str,
        left_scope: SettingsScope,
        right_source: &str,
        right_scope: SettingsScope,
    ) -> bool {
        if left_source == right_source {
            return true;
        }
        if !is_local_path(left_source) || !is_local_path(right_source) {
            return false;
        }
        let left = resolve_trimmed(left_source, &self.get_top_level_base_dir(left_scope));
        let right = resolve_trimmed(right_source, &self.get_top_level_base_dir(right_scope));
        left == right
    }

    fn find_matching_package_source(
        &self,
        item: &ResourceItem,
        target_scope: SettingsScope,
    ) -> Option<SettingsValue> {
        let settings = match target_scope {
            SettingsScope::Project => self.settings.get_project_settings(),
            SettingsScope::User => self.settings.get_global_settings(),
        };
        settings
            .get("packages")
            .and_then(SettingsValue::as_array)
            .and_then(|packages| {
                packages.iter().find(|pkg| {
                    let right = package_source_string(pkg);
                    self.package_source_string_matches(
                        &item.metadata.source,
                        self.get_item_scope(item),
                        &right,
                        target_scope,
                    )
                })
            })
            .cloned()
    }

    fn get_item_scope(&self, item: &ResourceItem) -> SettingsScope {
        if item.metadata.scope == SourceScope::Project {
            SettingsScope::Project
        } else {
            SettingsScope::User
        }
    }

    fn get_top_level_base_dir(&self, scope: SettingsScope) -> String {
        match scope {
            SettingsScope::Project => {
                host_join(&[&self.cwd, crate::coding_agent::core::CONFIG_DIR_NAME])
            }
            SettingsScope::User => self.agent_dir.clone(),
        }
    }

    fn get_resource_pattern(&self, item: &ResourceItem) -> String {
        if item.metadata.source == "builtin" {
            return item.path.clone();
        }
        let scope = self.get_item_scope(item);
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| self.get_top_level_base_dir(scope));
        host_relative(&base_dir, &item.path)
    }

    fn get_package_resource_pattern(&self, item: &ResourceItem) -> String {
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| host_dirname(&item.path));
        host_relative(&base_dir, &item.path)
    }

    /// Test seam: the currently selected flat index.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn selected_index(&self) -> usize {
        self.selected_index
    }
}

fn subgroup_key(group: &ResourceGroup, subgroup: &ResourceSubgroup) -> String {
    format!("{}:{}", group.key, subgroup.resource_type.as_str())
}

fn package_source_string(pkg: &SettingsValue) -> String {
    match pkg {
        SettingsValue::Str(source) => source.clone(),
        SettingsValue::Obj(_) => pkg
            .get("source")
            .and_then(SettingsValue::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn package_object(source: &str, autoload: Option<bool>) -> SettingsValue {
    let mut entries: Vec<(String, SettingsValue)> =
        vec![("source".to_string(), SettingsValue::Str(source.to_string()))];
    if let Some(autoload) = autoload {
        entries.push(("autoload".to_string(), SettingsValue::Bool(autoload)));
    }
    SettingsValue::Obj(entries)
}

fn get_pattern_entry_target(entry: &str) -> &str {
    entry
        .strip_prefix('!')
        .or_else(|| entry.strip_prefix('+'))
        .or_else(|| entry.strip_prefix('-'))
        .unwrap_or(entry)
}

impl Component for ResourceList {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_list(width)
    }

    fn handle_input(&mut self, data: &str) {
        ResourceList::handle_input(self, data)
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
        self.search_input.set_focused(focused);
    }
}

/// Upstream `ConfigSelectorComponent`.
pub struct ConfigSelectorComponent {
    focused: bool,
    header: ConfigSelectorHeader,
    resource_list: ResourceList,
}

impl ConfigSelectorComponent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        theme: Arc<Theme>,
        resolved_paths: &ScopedResolvedPaths,
        settings: Box<dyn ConfigSettingsStore + Send>,
        cwd: &str,
        agent_dir: &str,
        on_close: Box<dyn FnMut() + Send>,
        on_exit: Box<dyn FnMut() + Send>,
        request_render: Box<dyn FnMut() + Send>,
        terminal_height: Option<usize>,
        write_scope: ConfigWriteScope,
        project_mode_available: bool,
    ) -> Self {
        super::model_selector::set_default_theme(Arc::clone(&theme));
        let groups_by_scope = [
            build_groups(&resolved_paths.global, agent_dir),
            build_groups(&resolved_paths.project, agent_dir),
        ];

        let request_render: RenderFn = Arc::new(std::sync::Mutex::new(Box::new(request_render)));
        let mut resource_list = ResourceList::new(
            Arc::clone(&theme),
            groups_by_scope,
            settings,
            cwd,
            agent_dir,
            terminal_height,
            write_scope,
            project_mode_available,
            Arc::clone(&request_render),
        );
        resource_list.on_cancel = Some(on_close);
        resource_list.on_exit = Some(on_exit);
        resource_list.on_toggle = Some(Box::new(move |_item, _enabled| {
            (request_render.lock().expect("render fn"))();
        }));
        let header = ConfigSelectorHeader {
            theme: Arc::clone(&theme),
            write_scope,
            project_mode_available,
        };
        drop(theme);
        Self {
            focused: false,
            header,
            resource_list,
        }
    }

    /// Upstream `switchWriteScope` (upstream reaches this through the Tab
    /// binding's `onSwitchMode` closure; the list performs the same switch).
    pub fn switch_write_scope(&mut self) {
        self.resource_list.set_write_scope(
            if self.resource_list.write_scope() == ConfigWriteScope::Global {
                ConfigWriteScope::Project
            } else {
                ConfigWriteScope::Global
            },
        );
    }

    pub fn resource_list(&mut self) -> &mut ResourceList {
        &mut self.resource_list
    }
}

impl Component for ConfigSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // DynamicBorder
        lines.extend(DynamicBorder::default().render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Header (2 lines; scope mirrors the list's active scope)
        self.header.write_scope = self.resource_list.write_scope();
        lines.extend(self.header.render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
        // Resource list
        lines.extend(self.resource_list.render(width));
        // Spacer(1)
        lines.extend(spacer_lines(1));
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
        self.resource_list.set_focused(focused);
    }
}
