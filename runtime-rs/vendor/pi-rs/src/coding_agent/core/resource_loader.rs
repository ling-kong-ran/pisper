//! Port of upstream `coding-agent/src/core/resource-loader.ts` (upstream
//! SHA256 at migration time:
//! `1877e9535820cb8b45e5a84598ac8ae581058bb0fbee6f0c64ec672f8ab986dd`).
//!
//! Deterministic outputs (context-file discovery order across linked-worktree
//! skeletons, skills/prompts/themes discovery + sourceInfo assignment,
//! system-prompt discovery, collision diagnostics, extension load order and
//! conflict error texts) were captured from the real upstream sources under
//! node (type stripping) into
//! `tests/fixtures/core_oracle_w38/resource_loader.oracle.json` (generator
//! `oracle_resource_loader.mjs`) and are pinned byte-for-byte by the tests.
//!
//! Shared seams (upstream imports modules outside this slice; disclosed):
//!
//! - **SettingsManager ↔ package-manager**: the ported
//!   [`DefaultPackageManager`] consumes settings through the
//!   [`SettingsManagerHandle`] seam; the real bridge lives here as
//!   [`SettingsManagerAdapter`] (field snapshots only — the `setPackages`
//!   mutators are resource-loader-irrelevant and are no-ops).
//! - **jiti module loading** (extensions): the [`ExtensionModuleLoader`]
//!   seam from the extensions port is an injected option
//!   ([`DefaultResourceLoaderOptions::extension_module_loader`], defaulting
//!   to [`NullModuleLoader`]) — the Rust port has no runtime TS module
//!   system. Tests inject registry-backed loaders exactly like the oracle
//!   drives the jiti stub.
//! - **`loadPromptTemplates` / `PromptTemplate`** (upstream
//!   `core/prompt-templates.ts`, sha256
//!   `e94b8504b97fe668b04577891b7029abc7d11ac795e728982d2615a13ec1528a`, not
//!   yet ported as its own slice): the exact consumed subset is vendored in
//!   [`prompt_templates`] (discovery + parsing; the argument expansion
//!   helpers are not part of this surface).
//! - **`loadThemeFromPath` / `Theme`** (upstream
//!   `modes/interactive/theme/theme.ts`, sha256
//!   `c3bf2e3b72f6bb782f34de0535fcc1758b9b6ea7a0d2e7d6f17244fa55c3f31a`,
//!   not yet ported): the consumed subset is vendored in [`theme`] (read →
//!   JSON parse → default shape validation → name/sourcePath record). Color
//!   resolution, modes, watchers and highlighters are not observable through
//!   the resource loader.
//! - `resetTimings("extensions")` (timings.ts) is telemetry-only and dropped.
//! - `findGitPaths` comes from the ported [`footer_data_provider`].
//! - `getAgentDir`/`CONFIG_DIR_NAME` from [`crate::coding_agent::core`].
//!
//! Divergences (disclosed):
//!
//! 1. `reload` awaits the trust resolver (including extension/UI futures).
//!    Package resolution and module loading remain synchronous;
//!    `reload_without_trust` preserves the current verdict without prompting.
//! 2. **Constructor settings failure**: upstream `SettingsManager.create`
//!    throws synchronously out of the constructor; the port panics with the
//!    same effect. Explicitly provided managers bypass this.
//! 3. **`console.error` warnings**: the two `chalk.yellow` warnings print
//!    without color to stderr (chalk level 0 on a non-TTY renders the same
//!    text).
//! 4. **Theme/JSON parse error prose**: V8 `JSON.parse` and errno messages
//!    surfaced in theme diagnostics are platform-owned; the authored
//!    `Invalid theme "…": expected an object with a "colors" map.` shape
//!    check is pinned verbatim.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::coding_agent::core::diagnostics::{
    ResourceCollision, ResourceCollisionType, ResourceDiagnostic, ResourceDiagnosticType,
};
use crate::coding_agent::core::event_bus::{EventBus, EventBusController};
use crate::coding_agent::core::footer_data_provider::find_git_paths;
use crate::coding_agent::core::settings_manager::{
    SettingsManager, SettingsManagerCreateOptions, SettingsValue,
};
use crate::coding_agent::core::skills::{load_skills, LoadSkillsOptions, LoadSkillsResult, Skill};
use crate::coding_agent::extensions::loader::{
    clear_extension_cache, load_extension_from_factory, load_extensions_cached, ExtensionFactory,
    ExtensionModuleLoader, ExtensionRuntime, NullModuleLoader,
};
use crate::coding_agent::extensions::types::{
    Extension, ExtensionLoadError, LoadExtensionsResult, SourceInfo, SourceOrigin, SourceScope,
};
use crate::coding_agent::package_manager::{
    DefaultPackageManager, PackageFilterSpec, PackageManagerOptions, PackageSourceEntry,
    PathMetadata, PathMetadataOrigin, ResolvedResource, SettingsData, SettingsManagerHandle,
    SourceScope as PmSourceScope,
};
use crate::coding_agent::utils::paths::{
    canonicalize_path, is_local_path, resolve_path_auto_base, resolve_path_with, PathInputOptions,
};
use crate::coding_agent::utils::text::strip_bom;

use super::CONFIG_DIR_NAME;

// ===========================================================================
// Small shared path helpers (host platform = node semantics)
// ===========================================================================

fn node_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn node_join(base: &str, segments: &[&str]) -> String {
    let mut parts = vec![base];
    parts.extend_from_slice(segments);
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&parts)
    } else {
        crate::coding_agent::utils::node_path::posix_join(&parts)
    }
}

fn node_resolve(args: &[&str]) -> String {
    let cwd = node_cwd();
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_resolve(args, &cwd)
    } else {
        crate::coding_agent::utils::node_path::posix_resolve(args, &cwd)
    }
}

fn node_basename(path: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    match path.trim_end_matches(separators).rfind(separators) {
        Some(index) => path[index + 1..].to_string(),
        None => path.trim_end_matches(separators).to_string(),
    }
}

fn node_dirname(path: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    if !cfg!(windows) {
        return match path.rfind(separators) {
            Some(index) if index > 0 => path[..index].to_string(),
            Some(0) => path[..1].to_string(),
            _ => ".".to_string(),
        };
    }
    // node path.win32.dirname
    let chars: Vec<char> = path.chars().collect();
    let len = chars.len();
    if len == 0 {
        return ".".to_string();
    }
    let is_sep = |c: char| c == '\\' || c == '/';
    // Root prefix length (node win32): UNC \\server\share, drive "C:",
    // leading separator.
    let mut root_end;
    if len >= 2 && chars[0] == '\\' && chars[1] == '\\' {
        let mut idx = 2;
        let mut seps = 0;
        while idx < len {
            if is_sep(chars[idx]) {
                seps += 1;
                if seps == 2 {
                    break;
                }
            }
            idx += 1;
        }
        root_end = idx;
        if root_end < len {
            root_end += 1; // include the separator that ends the share
        }
    } else if chars[0].is_ascii_alphabetic() && len >= 2 && chars[1] == ':' {
        root_end = 2;
        if len >= 3 && is_sep(chars[2]) {
            root_end = 3;
        }
    } else if is_sep(chars[0]) {
        root_end = 1;
    } else {
        root_end = 0;
    }
    // Strip trailing separators beyond the root.
    let mut end = len;
    while end > root_end && is_sep(chars[end - 1]) {
        end -= 1;
    }
    if end <= root_end {
        // The whole path is a root (e.g. "C:\", "C:", "\\a\b"): node returns
        // it unchanged.
        return path.to_string();
    }
    let mut last_sep = None;
    for idx in (root_end..end).rev() {
        if is_sep(chars[idx]) {
            last_sep = Some(idx);
            break;
        }
    }
    match last_sep {
        Some(index) => {
            let cut = usize::max(index, root_end);
            chars[..cut].iter().collect()
        }
        None => {
            if root_end > 0 {
                chars[..root_end].iter().collect()
            } else {
                ".".to_string()
            }
        }
    }
}

const PATH_SEP: char = if cfg!(windows) { '\\' } else { '/' };

fn resolve_auto(input: &str) -> String {
    resolve_path_auto_base(input).unwrap_or_else(|_| input.to_string())
}

fn is_under_path(target: &str, root: &str) -> bool {
    let normalized_root = resolve_auto(root);
    if target == normalized_root {
        return true;
    }
    let prefix = if normalized_root.ends_with(PATH_SEP) {
        normalized_root.clone()
    } else {
        format!("{normalized_root}{PATH_SEP}")
    };
    target.starts_with(&prefix)
}

/// Insertion-ordered map stand-in (upstream `Map` iteration order is
/// observable through `findSourceInfoForPath`).
fn ordered_insert<V>(map: &mut Vec<(String, V)>, key: String, value: V) {
    match map.iter_mut().find(|(existing, _)| *existing == key) {
        Some(slot) => slot.1 = value,
        None => map.push((key, value)),
    }
}

fn ordered_get<'a, V>(map: &'a [(String, V)], key: &str) -> Option<&'a V> {
    map.iter()
        .find(|(existing, _)| existing == key)
        .map(|(_, v)| v)
}

// ===========================================================================
// source-info seam (upstream core/source-info.ts)
// ===========================================================================

fn pm_scope_to_source_scope(scope: PmSourceScope) -> SourceScope {
    match scope {
        PmSourceScope::User => SourceScope::User,
        PmSourceScope::Project => SourceScope::Project,
        PmSourceScope::Temporary => SourceScope::Temporary,
    }
}

fn pm_origin_to_source_origin(origin: PathMetadataOrigin) -> SourceOrigin {
    match origin {
        PathMetadataOrigin::Package => SourceOrigin::Package,
        PathMetadataOrigin::TopLevel => SourceOrigin::TopLevel,
    }
}

/// Upstream `createSourceInfo(path, metadata)` (core/source-info.ts).
fn create_source_info(path: &str, metadata: &PathMetadata) -> SourceInfo {
    SourceInfo {
        path: path.to_string(),
        source: metadata.source.clone(),
        scope: pm_scope_to_source_scope(metadata.scope),
        origin: pm_origin_to_source_origin(metadata.origin),
        base_dir: metadata.base_dir.clone(),
    }
}

// ===========================================================================
// prompt-templates seam (vendored subset of core/prompt-templates.ts)
// ===========================================================================

pub mod prompt_templates {
    //! Vendored from upstream `coding-agent/src/core/prompt-templates.ts`
    //! (sha256
    //! `e94b8504b97fe668b04577891b7029abc7d11ac795e728982d2615a13ec1528a`):
    //! the `PromptTemplate` record and `loadPromptTemplates` discovery used
    //! by the resource loader, pending that file's own migration slice. The
    //! argument-expansion helpers (`parseCommandArgs`, `substituteArgs`,
    //! `expandPromptTemplate`) are not part of this surface.

    use std::fs;

    use crate::coding_agent::extensions::types::{
        create_synthetic_source_info, SourceInfo, SourceScope,
    };
    use crate::coding_agent::utils::frontmatter::{parse_frontmatter, FrontmatterValue};
    use crate::coding_agent::utils::paths::{
        resolve_path_auto_base, resolve_path_with, PathInputOptions,
    };

    use super::{is_under_path, node_basename, node_dirname, node_join, CONFIG_DIR_NAME};

    /// Upstream `PromptTemplate`.
    #[derive(Debug, Clone, PartialEq)]
    pub struct PromptTemplate {
        pub name: String,
        pub description: String,
        pub argument_hint: Option<String>,
        pub content: String,
        pub source_info: SourceInfo,
        pub file_path: String,
    }

    /// JS string length (UTF-16 code units).
    fn js_length(value: &str) -> usize {
        value.encode_utf16().count()
    }

    /// JS `firstLine.slice(0, 60)`.
    fn js_slice_60(value: &str) -> String {
        let units: Vec<u16> = value.encode_utf16().take(60).collect();
        String::from_utf16_lossy(&units)
    }

    fn load_template_from_file(file_path: &str, source_info: SourceInfo) -> Option<PromptTemplate> {
        let raw_content = fs::read(file_path).ok()?;
        let raw_content = String::from_utf8_lossy(&raw_content).into_owned();
        let parsed = parse_frontmatter(&raw_content).ok()?;

        let basename = node_basename(file_path);
        let name = basename
            .strip_suffix(".md")
            .unwrap_or(&basename)
            .to_string();

        // Get description from frontmatter or first non-empty line
        let mut description = match parsed.frontmatter.get("description") {
            Some(FrontmatterValue::Str(s)) => s.clone(),
            _ => String::new(),
        };
        if description.is_empty() {
            if let Some(first_line) = parsed.body.split('\n').find(|line| !line.trim().is_empty()) {
                // Truncate if too long
                description = js_slice_60(first_line);
                if js_length(first_line) > 60 {
                    description += "...";
                }
            }
        }

        let argument_hint = match parsed.frontmatter.get("argument-hint") {
            Some(FrontmatterValue::Str(s)) => Some(s.clone()),
            _ => None,
        };

        Some(PromptTemplate {
            name,
            description,
            argument_hint,
            content: parsed.body,
            source_info,
            file_path: file_path.to_string(),
        })
    }

    /// Scan a directory for .md files (non-recursive) and load them as prompt
    /// templates.
    fn load_templates_from_dir(
        dir: &str,
        get_source_info: &dyn Fn(&str) -> SourceInfo,
    ) -> Vec<PromptTemplate> {
        let mut templates = Vec::new();

        if !std::path::Path::new(dir).exists() {
            return templates;
        }

        let Ok(entries) = fs::read_dir(dir) else {
            return templates;
        };
        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            let full_path = node_join(dir, &[&entry_name]);

            // For symlinks, check if they point to a file
            let mut is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                match fs::metadata(&full_path) {
                    Ok(stats) => is_file = stats.is_file(),
                    // Broken symlink, skip it
                    Err(_) => continue,
                }
            }

            if is_file && entry_name.ends_with(".md") {
                if let Some(template) =
                    load_template_from_file(&full_path, get_source_info(&full_path))
                {
                    templates.push(template);
                }
            }
        }

        templates
    }

    pub struct LoadPromptTemplatesOptions {
        /// Working directory for project-local templates.
        pub cwd: String,
        /// Agent config directory for global templates.
        pub agent_dir: String,
        /// Explicit prompt template paths (files or directories).
        pub prompt_paths: Vec<String>,
        /// Include default prompt directories.
        pub include_defaults: bool,
    }

    /// Load all prompt templates from:
    /// 1. Global: agentDir/prompts/
    /// 2. Project: cwd/{CONFIG_DIR_NAME}/prompts/
    /// 3. Explicit prompt paths
    pub fn load_prompt_templates(options: LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
        let LoadPromptTemplatesOptions {
            cwd,
            agent_dir,
            prompt_paths,
            include_defaults,
        } = options;

        let resolved_cwd = resolve_path_auto_base(&cwd).unwrap_or_else(|_| cwd.clone());
        let resolved_agent_dir =
            resolve_path_auto_base(&agent_dir).unwrap_or_else(|_| agent_dir.clone());

        let mut templates: Vec<PromptTemplate> = Vec::new();

        let global_prompts_dir = node_join(&resolved_agent_dir, &["prompts"]);
        let project_prompts_dir = node_join(&resolved_cwd, &[CONFIG_DIR_NAME, "prompts"]);

        let get_source_info = |resolved_path: &str| -> SourceInfo {
            if is_under_path(resolved_path, &global_prompts_dir) {
                return create_synthetic_source_info(
                    resolved_path,
                    "local",
                    Some(SourceScope::User),
                    None,
                    Some(global_prompts_dir.clone()),
                );
            }
            if is_under_path(resolved_path, &project_prompts_dir) {
                return create_synthetic_source_info(
                    resolved_path,
                    "local",
                    Some(SourceScope::Project),
                    None,
                    Some(project_prompts_dir.clone()),
                );
            }
            let base_dir = if std::fs::metadata(resolved_path)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                resolved_path.to_string()
            } else {
                node_dirname(resolved_path)
            };
            create_synthetic_source_info(resolved_path, "local", None, None, Some(base_dir))
        };

        if include_defaults {
            templates.extend(load_templates_from_dir(
                &global_prompts_dir,
                &get_source_info,
            ));
            templates.extend(load_templates_from_dir(
                &project_prompts_dir,
                &get_source_info,
            ));
        }

        // 3. Load explicit prompt paths
        for raw_path in &prompt_paths {
            let resolved_path = resolve_path_with(
                raw_path,
                &resolved_cwd,
                &PathInputOptions {
                    trim: true,
                    ..PathInputOptions::default()
                },
                cfg!(windows),
            )
            .unwrap_or_else(|_| raw_path.clone());
            if !std::path::Path::new(&resolved_path).exists() {
                continue;
            }

            let Ok(stats) = fs::metadata(&resolved_path) else {
                // Ignore read failures
                continue;
            };
            if stats.is_dir() {
                templates.extend(load_templates_from_dir(&resolved_path, &get_source_info));
            } else if stats.is_file() && resolved_path.ends_with(".md") {
                if let Some(template) =
                    load_template_from_file(&resolved_path, get_source_info(&resolved_path))
                {
                    templates.push(template);
                }
            }
        }

        templates
    }
}

use prompt_templates::{load_prompt_templates, PromptTemplate};

// ===========================================================================
// theme seam (vendored subset of modes/interactive/theme/theme.ts)
// ===========================================================================

pub mod theme {
    //! Vendored subset of upstream
    //! `coding-agent/src/modes/interactive/theme/theme.ts` (sha256
    //! `c3bf2e3b72f6bb782f34de0535fcc1758b9b6ea7a0d2e7d6f17244fa55c3f31a`):
    //! only what the resource loader observes — `loadThemeFromPath` reading,
    //! JSON-parsing and default (validator-less) shape-checking a theme file,
    //! and the authored error strings. Color resolution, modes, watchers and
    //! highlighters are presentation-side and not modeled.

    use std::fs;

    use serde_json::Value;

    use crate::coding_agent::extensions::types::SourceInfo;
    use crate::coding_agent::utils::text::strip_bom;

    /// The resource-loader-observable subset of the upstream `Theme` class.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Theme {
        pub name: Option<Value>,
        pub source_path: Option<String>,
        pub source_info: Option<SourceInfo>,
    }

    fn parse_theme_json(label: &str, json: &Value) -> Result<(), String> {
        if !json.is_object() || json.get("colors").is_none() {
            return Err(format!(
                "Invalid theme \"{label}\": expected an object with a \"colors\" map."
            ));
        }
        Ok(())
    }

    fn parse_theme_json_content(label: &str, content: &str) -> Result<Value, String> {
        let json: Value = serde_json::from_str(strip_bom(content))
            .map_err(|error| format!("Failed to parse theme {label}: {error}"))?;
        parse_theme_json(label, &json)?;
        Ok(json)
    }

    /// Upstream `loadThemeFromPath(themePath)`. `Err` carries the thrown
    /// error message (`readFileSync` failures render the io error text; the
    /// `JSON.parse` prose is V8-owned — both disclosed in the port docs).
    pub fn load_theme_from_path(theme_path: &str) -> Result<Theme, String> {
        let bytes = fs::read(theme_path).map_err(|error| error.to_string())?;
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let theme_json = parse_theme_json_content(theme_path, &content)?;
        Ok(Theme {
            name: theme_json.get("name").cloned(),
            source_path: Some(theme_path.to_string()),
            source_info: None,
        })
    }
}

use theme::{load_theme_from_path, Theme};

// ===========================================================================
// SettingsManager ↔ SettingsManagerHandle bridge
// ===========================================================================

/// Adapt the ported [`SettingsManager`] onto the package-manager slice's
/// [`SettingsManagerHandle`] seam (the snapshot surface `resolve()` reads).
struct SettingsManagerAdapter {
    inner: Arc<SettingsManager>,
}

fn settings_value_to_json(value: &SettingsValue) -> serde_json::Value {
    match value {
        SettingsValue::Null => serde_json::Value::Null,
        SettingsValue::Bool(b) => serde_json::Value::Bool(*b),
        SettingsValue::Num(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        SettingsValue::Str(s) => serde_json::Value::String(s.clone()),
        SettingsValue::Arr(items) => {
            serde_json::Value::Array(items.iter().map(settings_value_to_json).collect())
        }
        SettingsValue::Obj(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), settings_value_to_json(value)))
                .collect(),
        ),
    }
}

fn string_array_setting(items: Option<&Vec<SettingsValue>>) -> Vec<String> {
    items
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn settings_data_from_value(settings: &SettingsValue) -> SettingsData {
    let list = |key: &str| -> Option<&Vec<SettingsValue>> {
        match settings.get(key) {
            Some(SettingsValue::Arr(items)) => Some(items),
            _ => None,
        }
    };
    let packages = match list("packages") {
        Some(items) => items
            .iter()
            .filter_map(|item| match item {
                SettingsValue::Str(source) => Some(PackageSourceEntry::Plain(source.clone())),
                SettingsValue::Obj(entries) => {
                    let source = match entries.iter().find(|(key, _)| key == "source") {
                        Some((_, SettingsValue::Str(source))) => source.clone(),
                        _ => return None,
                    };
                    let field = |name: &str| -> Option<Vec<String>> {
                        match entries.iter().find(|(key, _)| key == name) {
                            Some((_, SettingsValue::Arr(items))) => {
                                Some(string_array_setting(Some(items)))
                            }
                            _ => None,
                        }
                    };
                    let autoload = match entries.iter().find(|(key, _)| key == "autoload") {
                        Some((_, SettingsValue::Bool(value))) => Some(*value),
                        _ => None,
                    };
                    let mut extra = serde_json::Map::new();
                    for (key, value) in entries {
                        if !matches!(
                            key.as_str(),
                            "source" | "autoload" | "extensions" | "skills" | "prompts" | "themes"
                        ) {
                            extra.insert(key.clone(), settings_value_to_json(value));
                        }
                    }
                    Some(PackageSourceEntry::Object(PackageFilterSpec {
                        source,
                        autoload,
                        extensions: field("extensions"),
                        skills: field("skills"),
                        prompts: field("prompts"),
                        themes: field("themes"),
                        extra,
                    }))
                }
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    SettingsData {
        packages,
        extensions: string_array_setting(list("extensions")),
        skills: string_array_setting(list("skills")),
        prompts: string_array_setting(list("prompts")),
        themes: string_array_setting(list("themes")),
        npm_command: None,
    }
}

impl SettingsManagerHandle for SettingsManagerAdapter {
    fn global_settings(&self) -> SettingsData {
        let mut data = settings_data_from_value(&self.inner.get_global_settings());
        data.npm_command = self.inner.get_npm_command();
        data
    }

    fn project_settings(&self) -> SettingsData {
        settings_data_from_value(&self.inner.get_project_settings())
    }

    fn is_project_trusted(&self) -> bool {
        self.inner.is_project_trusted()
    }

    fn set_project_trusted(&self, trusted: bool) {
        self.inner.set_project_trusted(trusted);
    }

    fn npm_command(&self) -> Option<Vec<String>> {
        self.inner.get_npm_command()
    }

    // Install-flow mutators; the resource loader never drives installs.
    fn set_packages(&self, _packages: Vec<PackageSourceEntry>) {}
    fn set_project_packages(&self, _packages: Vec<PackageSourceEntry>) {}
}

// ===========================================================================
// resource-loader records
// ===========================================================================

/// Upstream `{ path, metadata }` resource entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourcePathEntry {
    pub path: String,
    pub metadata: PathMetadata,
}

/// Upstream `ResourceExtensionPaths`.
#[derive(Debug, Clone, Default)]
pub struct ResourceExtensionPaths {
    pub skill_paths: Vec<ResourcePathEntry>,
    pub prompt_paths: Vec<ResourcePathEntry>,
    pub theme_paths: Vec<ResourcePathEntry>,
}

/// Upstream resolveProjectTrust may await UI/extension handlers and reject.
/// Borrow the bootstrap set across await without cloning or holding a mutex.
pub type ResolveProjectTrust = Arc<
    dyn for<'a> Fn(&'a LoadExtensionsResult) -> futures::future::BoxFuture<'a, Result<bool, String>>
        + Send
        + Sync,
>;

/// Upstream ResourceLoaderReloadOptions.
#[derive(Default)]
pub struct ResourceLoaderReloadOptions {
    pub resolve_project_trust: Option<ResolveProjectTrust>,
}

/// Upstream `{ path, content }` context-file record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

/// Upstream `{ prompts, diagnostics }`.
#[derive(Debug, Clone, Default)]
pub struct PromptsResult {
    pub prompts: Vec<PromptTemplate>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Upstream `{ themes, diagnostics }`.
#[derive(Debug, Clone, Default)]
pub struct ThemesResult {
    pub themes: Vec<Theme>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Upstream `{ agentsFiles }`.
#[derive(Debug, Clone, Default)]
pub struct AgentsFilesResult {
    pub agents_files: Vec<ContextFile>,
}

/// Upstream `InlineExtension` (factory or `{ factory, name, hidden }`).
#[derive(Clone)]
pub enum InlineExtension {
    Factory(ExtensionFactory),
    Named {
        factory: ExtensionFactory,
        name: String,
        hidden: bool,
    },
}

pub type ExtensionsOverride =
    Arc<dyn Fn(LoadExtensionsResult) -> LoadExtensionsResult + Send + Sync>;
pub type SkillsOverride = Arc<dyn Fn(LoadSkillsResult) -> LoadSkillsResult + Send + Sync>;
pub type PromptsOverride = Arc<dyn Fn(PromptsResult) -> PromptsResult + Send + Sync>;
pub type ThemesOverride = Arc<dyn Fn(ThemesResult) -> ThemesResult + Send + Sync>;
pub type AgentsFilesOverride = Arc<dyn Fn(AgentsFilesResult) -> AgentsFilesResult + Send + Sync>;
pub type SystemPromptOverride = Arc<dyn Fn(Option<String>) -> Option<String> + Send + Sync>;
pub type AppendSystemPromptOverride = Arc<dyn Fn(Vec<String>) -> Vec<String> + Send + Sync>;

/// Upstream `DefaultResourceLoaderOptions` plus the port-only
/// `extension_module_loader` jiti seam.
#[derive(Clone, Default)]
pub struct DefaultResourceLoaderOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub settings_manager: Option<Arc<SettingsManager>>,
    pub event_bus: Option<EventBus>,
    pub additional_extension_paths: Vec<String>,
    pub additional_skill_paths: Vec<String>,
    pub additional_prompt_template_paths: Vec<String>,
    pub additional_theme_paths: Vec<String>,
    pub extension_factories: Vec<InlineExtension>,
    pub no_extensions: bool,
    pub no_skills: bool,
    pub no_prompt_templates: bool,
    pub no_themes: bool,
    pub no_context_files: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<Vec<String>>,
    pub extensions_override: Option<ExtensionsOverride>,
    pub skills_override: Option<SkillsOverride>,
    pub prompts_override: Option<PromptsOverride>,
    pub themes_override: Option<ThemesOverride>,
    pub agents_files_override: Option<AgentsFilesOverride>,
    pub system_prompt_override: Option<SystemPromptOverride>,
    pub append_system_prompt_override: Option<AppendSystemPromptOverride>,
    /// Port-only: the jiti seam (see module docs).
    pub extension_module_loader: Option<Arc<dyn ExtensionModuleLoader>>,
}

// ===========================================================================
// Context files
// ===========================================================================

fn resolve_prompt_input(input: Option<&str>, description: &str) -> Option<String> {
    let input = input?;
    if input.is_empty() {
        return None;
    }

    if Path::new(input).exists() {
        match fs::read(input) {
            Ok(bytes) => return Some(strip_bom(&String::from_utf8_lossy(&bytes)).to_string()),
            Err(error) => {
                eprintln!("Warning: Could not read {description} file {input}: {error}");
                return Some(input.to_string());
            }
        }
    }

    Some(input.to_string())
}

fn load_context_file_from_dir(dir: &str) -> Option<ContextFile> {
    let candidates = [
        "AGENTS.override.md",
        "AGENTS.md",
        "AGENTS.MD",
        "CLAUDE.md",
        "CLAUDE.MD",
    ];
    for filename in candidates {
        let file_path = node_join(dir, &[filename]);
        if Path::new(&file_path).exists() {
            match fs::metadata(&file_path) {
                Ok(stats) if stats.is_file() => match fs::read(&file_path) {
                    Ok(bytes) => {
                        return Some(ContextFile {
                            path: file_path,
                            content: strip_bom(&String::from_utf8_lossy(&bytes)).to_string(),
                        });
                    }
                    Err(error) => {
                        eprintln!("Warning: Could not read {file_path}: {error}");
                    }
                },
                Ok(_) => continue,
                Err(error) => {
                    eprintln!("Warning: Could not read {file_path}: {error}");
                }
            }
        }
    }
    None
}

/// The main repo's context file that a nested linked worktree's own copy
/// shadows: both occupy the same logical repository scope, so loading both
/// applies that context twice. Returns None when nothing is shadowed, leaving
/// normal ancestor inheritance alone.
///
/// Returned canonicalized (realpath), because `git worktree add` writes the
/// `.git` file's `gitdir:` target in realpath form while cwd may still be
/// symlinked (macOS `/tmp` -> `/private/tmp`).
fn find_shadowed_context_file(cwd: &str) -> Option<String> {
    let git_paths = find_git_paths(cwd)?;
    let common_git_dir = canonicalize_path(&git_paths.common_git_dir);
    let worktree_root = canonicalize_path(&git_paths.repo_dir);
    let main_repo_root = node_dirname(&common_git_dir);
    // False for an ordinary repo, where the two are the same dir, and for a
    // sibling worktree (`git worktree add ../feat`), whose main repo is not an
    // ancestor.
    let main_prefix = format!("{main_repo_root}{PATH_SEP}");
    if !worktree_root.starts_with(&main_prefix) {
        return None;
    }
    // dirname of the common git dir is the main worktree root only when that
    // dir is itself checked out from the same repo. In a bare layout
    // (`proj/.bare` + `proj/main`) it is just the directory holding `.bare`,
    // which tracks nothing; a submodule's gitdir has no `commondir`, so it
    // lands under `.git/modules`.
    if canonicalize_path(&node_join(&main_repo_root, &[".git"])) != common_git_dir {
        return None;
    }
    let worktree_context_file = load_context_file_from_dir(&worktree_root)?;
    Some(node_join(
        &main_repo_root,
        &[&node_basename(&worktree_context_file.path)],
    ))
}

/// Upstream `loadProjectContextFiles`.
pub fn load_project_context_files(cwd: &str, agent_dir: &str) -> Vec<ContextFile> {
    let resolved_cwd = resolve_path_auto_base(cwd).unwrap_or_else(|_| cwd.to_string());
    let resolved_agent_dir =
        resolve_path_auto_base(agent_dir).unwrap_or_else(|_| agent_dir.to_string());

    let mut context_files: Vec<ContextFile> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    if let Some(global_context) = load_context_file_from_dir(&resolved_agent_dir) {
        seen_paths.insert(global_context.path.clone());
        context_files.push(global_context);
    }

    let mut ancestor_context_files: Vec<ContextFile> = Vec::new();

    let shadowed_context_file = find_shadowed_context_file(&resolved_cwd);
    let mut current_dir = resolved_cwd;

    loop {
        let context_file = load_context_file_from_dir(&current_dir);
        let is_shadowed = match (&shadowed_context_file, &context_file) {
            (Some(shadowed), Some(file)) => canonicalize_path(&file.path) == *shadowed,
            _ => false,
        };
        if let Some(file) = context_file {
            if !is_shadowed && !seen_paths.contains(&file.path) {
                seen_paths.insert(file.path.clone());
                ancestor_context_files.insert(0, file);
            }
        }

        let parent_dir = node_dirname(&current_dir);
        if parent_dir == current_dir {
            break;
        }
        current_dir = parent_dir;
    }

    context_files.extend(ancestor_context_files);

    context_files
}

// ===========================================================================
// DefaultResourceLoader
// ===========================================================================

fn warning(message: impl Into<String>, path: &str) -> ResourceDiagnostic {
    ResourceDiagnostic {
        r#type: ResourceDiagnosticType::Warning,
        message: message.into(),
        path: Some(path.to_string()),
        collision: None,
    }
}

fn error_diagnostic(message: impl Into<String>, path: &str) -> ResourceDiagnostic {
    ResourceDiagnostic {
        r#type: ResourceDiagnosticType::Error,
        message: message.into(),
        path: Some(path.to_string()),
        collision: None,
    }
}

/// Upstream `DefaultResourceLoader` (the `ResourceLoader` interface methods
/// become inherent methods here).
pub struct DefaultResourceLoader {
    cwd: String,
    agent_dir: String,
    settings_manager: Arc<SettingsManager>,
    event_bus: EventBus,
    package_manager: DefaultPackageManager,
    additional_extension_paths: Vec<String>,
    additional_skill_paths: Vec<String>,
    additional_prompt_template_paths: Vec<String>,
    additional_theme_paths: Vec<String>,
    extension_factories: Vec<InlineExtension>,
    no_extensions: bool,
    no_skills: bool,
    no_prompt_templates: bool,
    no_themes: bool,
    no_context_files: bool,
    system_prompt_source: Option<String>,
    append_system_prompt_source: Option<Vec<String>>,
    extensions_override: Option<ExtensionsOverride>,
    skills_override: Option<SkillsOverride>,
    prompts_override: Option<PromptsOverride>,
    themes_override: Option<ThemesOverride>,
    agents_files_override: Option<AgentsFilesOverride>,
    system_prompt_override: Option<SystemPromptOverride>,
    append_system_prompt_override: Option<AppendSystemPromptOverride>,

    extensions_result: LoadExtensionsResult,
    skills: Vec<Skill>,
    skill_diagnostics: Vec<ResourceDiagnostic>,
    prompts: Vec<PromptTemplate>,
    prompt_diagnostics: Vec<ResourceDiagnostic>,
    themes: Vec<Theme>,
    theme_diagnostics: Vec<ResourceDiagnostic>,
    agents_files: Vec<ContextFile>,
    system_prompt: Option<String>,
    system_prompt_source_path: Option<String>,
    append_system_prompt: Vec<String>,
    append_system_prompt_source_paths: Vec<String>,
    last_skill_paths: Vec<String>,
    extension_skill_source_infos: Vec<(String, SourceInfo)>,
    extension_prompt_source_infos: Vec<(String, SourceInfo)>,
    extension_theme_source_infos: Vec<(String, SourceInfo)>,
    resource_metadata_by_path: Vec<(String, PathMetadata)>,
    last_prompt_paths: Vec<String>,
    last_theme_paths: Vec<String>,
    loaded: bool,
    extension_module_loader: Arc<dyn ExtensionModuleLoader>,
}

impl DefaultResourceLoader {
    pub fn new(options: DefaultResourceLoaderOptions) -> DefaultResourceLoader {
        let DefaultResourceLoaderOptions {
            cwd,
            agent_dir,
            settings_manager,
            event_bus,
            additional_extension_paths,
            additional_skill_paths,
            additional_prompt_template_paths,
            additional_theme_paths,
            extension_factories,
            no_extensions,
            no_skills,
            no_prompt_templates,
            no_themes,
            no_context_files,
            system_prompt,
            append_system_prompt,
            extensions_override,
            skills_override,
            prompts_override,
            themes_override,
            agents_files_override,
            system_prompt_override,
            append_system_prompt_override,
            extension_module_loader,
        } = options;

        let cwd = resolve_path_auto_base(&cwd).unwrap_or(cwd);
        let agent_dir = resolve_path_auto_base(&agent_dir).unwrap_or(agent_dir);
        let settings_manager = settings_manager.unwrap_or_else(|| {
            Arc::new(
                SettingsManager::create_with(
                    &cwd,
                    &agent_dir,
                    SettingsManagerCreateOptions::default(),
                )
                .expect("SettingsManager.create failed"),
            )
        });
        let event_bus = event_bus.unwrap_or_else(|| EventBusController::new().bus().clone());
        let package_manager = DefaultPackageManager::new(PackageManagerOptions {
            cwd: cwd.clone(),
            agent_dir: agent_dir.clone(),
            settings_manager: Arc::new(SettingsManagerAdapter {
                inner: settings_manager.clone(),
            }),
            command_runner: None,
        });

        DefaultResourceLoader {
            cwd,
            agent_dir,
            settings_manager,
            event_bus,
            package_manager,
            additional_extension_paths,
            additional_skill_paths,
            additional_prompt_template_paths,
            additional_theme_paths,
            extension_factories,
            no_extensions,
            no_skills,
            no_prompt_templates,
            no_themes,
            no_context_files,
            system_prompt_source: system_prompt,
            append_system_prompt_source: append_system_prompt,
            extensions_override,
            skills_override,
            prompts_override,
            themes_override,
            agents_files_override,
            system_prompt_override,
            append_system_prompt_override,
            extensions_result: LoadExtensionsResult {
                extensions: Vec::new(),
                errors: Vec::new(),
                warnings: Vec::new(),
                runtime: ExtensionRuntime::new(),
            },
            skills: Vec::new(),
            skill_diagnostics: Vec::new(),
            prompts: Vec::new(),
            prompt_diagnostics: Vec::new(),
            themes: Vec::new(),
            theme_diagnostics: Vec::new(),
            agents_files: Vec::new(),
            system_prompt: None,
            system_prompt_source_path: None,
            append_system_prompt: Vec::new(),
            append_system_prompt_source_paths: Vec::new(),
            last_skill_paths: Vec::new(),
            extension_skill_source_infos: Vec::new(),
            extension_prompt_source_infos: Vec::new(),
            extension_theme_source_infos: Vec::new(),
            resource_metadata_by_path: Vec::new(),
            last_prompt_paths: Vec::new(),
            last_theme_paths: Vec::new(),
            loaded: false,
            extension_module_loader: extension_module_loader
                .unwrap_or_else(|| Arc::new(NullModuleLoader)),
        }
    }

    // -----------------------------------------------------------------
    // Getters
    // -----------------------------------------------------------------

    pub fn get_extensions(&self) -> LoadExtensionsResult {
        self.extensions_result.clone()
    }

    pub fn get_skills(&self) -> LoadSkillsResult {
        LoadSkillsResult {
            skills: self.skills.clone(),
            diagnostics: self.skill_diagnostics.clone(),
        }
    }

    pub fn get_prompts(&self) -> PromptsResult {
        PromptsResult {
            prompts: self.prompts.clone(),
            diagnostics: self.prompt_diagnostics.clone(),
        }
    }

    pub fn get_themes(&self) -> ThemesResult {
        ThemesResult {
            themes: self.themes.clone(),
            diagnostics: self.theme_diagnostics.clone(),
        }
    }

    pub fn get_agents_files(&self) -> Vec<ContextFile> {
        self.agents_files.clone()
    }

    pub fn get_system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }

    pub fn get_system_prompt_source(&self) -> Option<String> {
        self.system_prompt_source_path.clone()
    }

    pub fn get_append_system_prompt(&self) -> Vec<String> {
        self.append_system_prompt.clone()
    }

    pub fn get_append_system_prompt_sources(&self) -> Vec<String> {
        self.append_system_prompt_source_paths.clone()
    }

    // -----------------------------------------------------------------
    // extendResources
    // -----------------------------------------------------------------

    pub fn extend_resources(&mut self, paths: ResourceExtensionPaths) {
        let skill_paths = self.normalize_extension_paths(paths.skill_paths);
        let prompt_paths = self.normalize_extension_paths(paths.prompt_paths);
        let theme_paths = self.normalize_extension_paths(paths.theme_paths);

        for entry in &skill_paths {
            ordered_insert(
                &mut self.extension_skill_source_infos,
                entry.path.clone(),
                create_source_info(&entry.path, &entry.metadata),
            );
        }
        for entry in &prompt_paths {
            ordered_insert(
                &mut self.extension_prompt_source_infos,
                entry.path.clone(),
                create_source_info(&entry.path, &entry.metadata),
            );
        }
        for entry in &theme_paths {
            ordered_insert(
                &mut self.extension_theme_source_infos,
                entry.path.clone(),
                create_source_info(&entry.path, &entry.metadata),
            );
        }

        if !skill_paths.is_empty() {
            let entry_paths: Vec<String> = skill_paths.iter().map(|e| e.path.clone()).collect();
            self.last_skill_paths = self.merge_paths(&self.last_skill_paths.clone(), &entry_paths);
            let (paths, metadata) = (
                self.last_skill_paths.clone(),
                self.resource_metadata_by_path.clone(),
            );
            self.update_skills_from_paths(&paths, &metadata);
        }

        if !prompt_paths.is_empty() {
            let entry_paths: Vec<String> = prompt_paths.iter().map(|e| e.path.clone()).collect();
            self.last_prompt_paths =
                self.merge_paths(&self.last_prompt_paths.clone(), &entry_paths);
            let (paths, metadata) = (
                self.last_prompt_paths.clone(),
                self.resource_metadata_by_path.clone(),
            );
            self.update_prompts_from_paths(&paths, &metadata);
        }

        if !theme_paths.is_empty() {
            let entry_paths: Vec<String> = theme_paths.iter().map(|e| e.path.clone()).collect();
            self.last_theme_paths = self.merge_paths(&self.last_theme_paths.clone(), &entry_paths);
            let (paths, metadata) = (
                self.last_theme_paths.clone(),
                self.resource_metadata_by_path.clone(),
            );
            self.update_themes_from_paths(&paths, &metadata);
        }
    }

    // -----------------------------------------------------------------
    // Trust bootstrap + reload
    // -----------------------------------------------------------------

    /// Upstream `loadProjectTrustExtensions`: forces untrusted project
    /// settings for the bootstrap pass so project-local extensions/packages
    /// stay out while user/global and temporary CLI extensions load.
    pub fn load_project_trust_extensions(&mut self) -> Result<LoadExtensionsResult, String> {
        self.settings_manager.set_project_trusted(false);
        self.settings_manager.reload();
        self.load_current_extension_set(true)
    }

    fn begin_reload(&self) {
        // resetTimings("extensions") is telemetry-only.
        if self.loaded {
            clear_extension_cache();
        }
    }

    /// Reload using the already established trust verdict. This is the
    /// synchronous subset for runtime reloads which never re-prompt for trust.
    pub fn reload_without_trust(&mut self) -> Result<(), String> {
        self.begin_reload();
        self.reload_resources(None)
    }

    /// Upstream reload: untrusted bootstrap, await trust, then load project
    /// resources. Rejection/cancellation leaves project settings untrusted.
    pub async fn reload(
        &mut self,
        options: Option<ResourceLoaderReloadOptions>,
    ) -> Result<(), String> {
        self.begin_reload();
        let mut pre_trust_extensions = None;
        if let Some(resolve) = options.and_then(|o| o.resolve_project_trust) {
            let pre = self.load_project_trust_extensions()?;
            let trusted = resolve(&pre).await?;
            self.settings_manager.set_project_trusted(trusted);
            pre_trust_extensions = Some(pre);
        }
        self.reload_resources(pre_trust_extensions)
    }

    fn reload_resources(
        &mut self,
        pre_trust_extensions: Option<LoadExtensionsResult>,
    ) -> Result<(), String> {
        // reload() preserves SettingsManager.projectTrusted and reloads
        // settings for that trust state.
        self.settings_manager.reload();
        let resolved_paths = self
            .package_manager
            .resolve(None)
            .map_err(|e| e.to_string())?;
        let cli_extension_paths = self
            .package_manager
            .resolve_extension_sources(&self.additional_extension_paths, false, true)
            .map_err(|e| e.to_string())?;
        // Kept on the instance so post-reload passes (extendResources) can
        // still resolve package metadata.
        let mut metadata_by_path: Vec<(String, PathMetadata)> = Vec::new();

        self.extension_skill_source_infos = Vec::new();
        self.extension_prompt_source_infos = Vec::new();
        self.extension_theme_source_infos = Vec::new();

        // Helper to extract enabled resources/paths and store metadata.
        fn get_enabled_resources(
            resources: &[ResolvedResource],
            metadata_by_path: &mut Vec<(String, PathMetadata)>,
        ) -> Vec<ResolvedResource> {
            for r in resources {
                if ordered_get(metadata_by_path, &r.path).is_none() {
                    ordered_insert(metadata_by_path, r.path.clone(), r.metadata.clone());
                }
            }
            resources.iter().filter(|r| r.enabled).cloned().collect()
        }

        fn get_enabled_paths(
            resources: &[ResolvedResource],
            metadata_by_path: &mut Vec<(String, PathMetadata)>,
        ) -> Vec<String> {
            get_enabled_resources(resources, metadata_by_path)
                .into_iter()
                .map(|r| r.path)
                .collect()
        }

        let enabled_extensions =
            get_enabled_paths(&resolved_paths.extensions, &mut metadata_by_path);
        let enabled_skill_resources =
            get_enabled_resources(&resolved_paths.skills, &mut metadata_by_path);
        let enabled_prompts = get_enabled_paths(&resolved_paths.prompts, &mut metadata_by_path);
        let enabled_themes = get_enabled_paths(&resolved_paths.themes, &mut metadata_by_path);

        let enabled_skills: Vec<String> = enabled_skill_resources
            .iter()
            .map(|resource| self.map_skill_path(resource, &mut metadata_by_path))
            .collect();

        // Add CLI paths metadata
        for r in &cli_extension_paths.extensions {
            if ordered_get(&metadata_by_path, &r.path).is_none() {
                ordered_insert(
                    &mut metadata_by_path,
                    r.path.clone(),
                    PathMetadata {
                        source: "cli".to_string(),
                        scope: PmSourceScope::Temporary,
                        origin: PathMetadataOrigin::TopLevel,
                        base_dir: None,
                    },
                );
            }
        }
        for r in &cli_extension_paths.skills {
            if ordered_get(&metadata_by_path, &r.path).is_none() {
                ordered_insert(
                    &mut metadata_by_path,
                    r.path.clone(),
                    PathMetadata {
                        source: "cli".to_string(),
                        scope: PmSourceScope::Temporary,
                        origin: PathMetadataOrigin::TopLevel,
                        base_dir: None,
                    },
                );
            }
        }

        let cli_enabled_extensions =
            get_enabled_paths(&cli_extension_paths.extensions, &mut metadata_by_path);
        let cli_enabled_skills =
            get_enabled_paths(&cli_extension_paths.skills, &mut metadata_by_path);
        let cli_enabled_prompts =
            get_enabled_paths(&cli_extension_paths.prompts, &mut metadata_by_path);
        let cli_enabled_themes =
            get_enabled_paths(&cli_extension_paths.themes, &mut metadata_by_path);

        let extension_paths = if self.no_extensions {
            cli_enabled_extensions
        } else {
            self.merge_paths(&cli_enabled_extensions, &enabled_extensions)
        };

        let mut extensions_result =
            self.load_final_extension_set(extension_paths, pre_trust_extensions)?;
        for p in &self.additional_extension_paths {
            if is_local_path(p) {
                let resolved = self.resolve_resource_path(p);
                if !Path::new(&resolved).exists() {
                    extensions_result.errors.push(ExtensionLoadError {
                        path: resolved.clone(),
                        error: format!("Extension path does not exist: {resolved}"),
                    });
                }
            }
        }
        let extensions_result = match &self.extensions_override {
            Some(apply) => apply(extensions_result),
            None => extensions_result,
        };
        let mut extensions_result = extensions_result;
        self.apply_extension_source_info(&mut extensions_result, &metadata_by_path);
        self.extensions_result = extensions_result;

        let skill_paths = if self.no_skills {
            self.merge_paths(&cli_enabled_skills, &self.additional_skill_paths)
        } else {
            let combined = cli_enabled_skills
                .iter()
                .chain(enabled_skills.iter())
                .cloned()
                .collect::<Vec<String>>();
            self.merge_paths(&combined, &self.additional_skill_paths)
        };

        self.last_skill_paths = skill_paths.clone();
        self.update_skills_from_paths(&skill_paths, &metadata_by_path);
        for p in &self.additional_skill_paths {
            if is_local_path(p) {
                let resolved = self.resolve_resource_path(p);
                if !Path::new(&resolved).exists()
                    && !self
                        .skill_diagnostics
                        .iter()
                        .any(|d| d.path.as_deref() == Some(resolved.as_str()))
                {
                    self.skill_diagnostics
                        .push(error_diagnostic("Skill path does not exist", &resolved));
                }
            }
        }

        let prompt_paths = if self.no_prompt_templates {
            self.merge_paths(&cli_enabled_prompts, &self.additional_prompt_template_paths)
        } else {
            let combined = cli_enabled_prompts
                .iter()
                .chain(enabled_prompts.iter())
                .cloned()
                .collect::<Vec<String>>();
            self.merge_paths(&combined, &self.additional_prompt_template_paths)
        };

        self.last_prompt_paths = prompt_paths.clone();
        self.update_prompts_from_paths(&prompt_paths, &metadata_by_path);
        for p in &self.additional_prompt_template_paths {
            if is_local_path(p) {
                let resolved = self.resolve_resource_path(p);
                if !Path::new(&resolved).exists()
                    && !self
                        .prompt_diagnostics
                        .iter()
                        .any(|d| d.path.as_deref() == Some(resolved.as_str()))
                {
                    self.prompt_diagnostics.push(error_diagnostic(
                        "Prompt template path does not exist",
                        &resolved,
                    ));
                }
            }
        }

        let theme_paths = if self.no_themes {
            self.merge_paths(&cli_enabled_themes, &self.additional_theme_paths)
        } else {
            let combined = cli_enabled_themes
                .iter()
                .chain(enabled_themes.iter())
                .cloned()
                .collect::<Vec<String>>();
            self.merge_paths(&combined, &self.additional_theme_paths)
        };

        self.last_theme_paths = theme_paths.clone();
        self.update_themes_from_paths(&theme_paths, &metadata_by_path);
        for p in &self.additional_theme_paths {
            let resolved = self.resolve_resource_path(p);
            if !Path::new(&resolved).exists()
                && !self
                    .theme_diagnostics
                    .iter()
                    .any(|d| d.path.as_deref() == Some(resolved.as_str()))
            {
                self.theme_diagnostics
                    .push(error_diagnostic("Theme path does not exist", &resolved));
            }
        }

        let agents_files = AgentsFilesResult {
            agents_files: if self.no_context_files {
                Vec::new()
            } else {
                load_project_context_files(&self.cwd, &self.agent_dir)
            },
        };
        let resolved_agents_files = match &self.agents_files_override {
            Some(apply) => apply(agents_files),
            None => agents_files,
        };
        self.agents_files = resolved_agents_files.agents_files;

        let system_prompt_source = match &self.system_prompt_source {
            Some(source) => Some(source.clone()),
            None => self.discover_system_prompt_file(),
        };
        let base_system_prompt =
            resolve_prompt_input(system_prompt_source.as_deref(), "system prompt");
        self.system_prompt = match &self.system_prompt_override {
            Some(apply) => apply(base_system_prompt),
            None => base_system_prompt,
        };
        self.system_prompt_source_path = match &system_prompt_source {
            Some(source) if !source.is_empty() && Path::new(source.as_str()).exists() => {
                Some(resolve_auto(source))
            }
            _ => None,
        };

        let append_sources = match &self.append_system_prompt_source {
            Some(sources) => sources.clone(),
            None => match self.discover_append_system_prompt_file() {
                Some(file) => vec![file],
                None => Vec::new(),
            },
        };
        let base_append: Vec<String> = append_sources
            .iter()
            .filter_map(|s| resolve_prompt_input(Some(s), "append system prompt"))
            .collect();
        self.append_system_prompt = match &self.append_system_prompt_override {
            Some(apply) => apply(base_append),
            None => base_append,
        };
        self.append_system_prompt_source_paths = append_sources
            .iter()
            .filter(|source| !source.is_empty() && Path::new(source.as_str()).exists())
            .map(|source| resolve_auto(source))
            .collect();
        self.resource_metadata_by_path = metadata_by_path;
        self.loaded = true;
        Ok(())
    }

    fn load_current_extension_set(
        &self,
        include_inline_factories: bool,
    ) -> Result<LoadExtensionsResult, String> {
        let resolved_paths = self
            .package_manager
            .resolve(None)
            .map_err(|e| e.to_string())?;
        let cli_extension_paths = self
            .package_manager
            .resolve_extension_sources(&self.additional_extension_paths, false, true)
            .map_err(|e| e.to_string())?;
        let enabled_extensions: Vec<String> = resolved_paths
            .extensions
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.path.clone())
            .collect();
        let cli_enabled_extensions: Vec<String> = cli_extension_paths
            .extensions
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.path.clone())
            .collect();
        let extension_paths = if self.no_extensions {
            cli_enabled_extensions
        } else {
            self.merge_paths(&cli_enabled_extensions, &enabled_extensions)
        };
        let extensions_result = load_extensions_cached(
            &extension_paths,
            &self.cwd,
            Some(self.event_bus.clone()),
            None,
            &*self.extension_module_loader,
        );
        if !include_inline_factories {
            return Ok(extensions_result);
        }

        let mut extensions_result = extensions_result;
        let inline_extensions = self.load_extension_factories(&extensions_result.runtime);
        extensions_result
            .extensions
            .extend(inline_extensions.extensions);
        extensions_result.errors.extend(inline_extensions.errors);
        Ok(extensions_result)
    }

    fn resolve_extension_load_path(&self, path: &str) -> String {
        resolve_path_with(
            path,
            &self.cwd,
            &PathInputOptions {
                normalize_unicode_spaces: true,
                ..PathInputOptions::default()
            },
            cfg!(windows),
        )
        .unwrap_or_else(|_| path.to_string())
    }

    fn load_final_extension_set(
        &self,
        extension_paths: Vec<String>,
        pre_trust_extensions: Option<LoadExtensionsResult>,
    ) -> Result<LoadExtensionsResult, String> {
        let Some(pre_trust_extensions) = pre_trust_extensions else {
            let mut extensions_result = load_extensions_cached(
                &extension_paths,
                &self.cwd,
                Some(self.event_bus.clone()),
                None,
                &*self.extension_module_loader,
            );
            let inline_extensions = self.load_extension_factories(&extensions_result.runtime);
            extensions_result
                .extensions
                .extend(inline_extensions.extensions);
            extensions_result.errors.extend(inline_extensions.errors);
            add_extension_conflict_diagnostics(&mut extensions_result);
            return Ok(extensions_result);
        };

        let mut preloaded_by_path: Vec<(String, Extension)> = pre_trust_extensions
            .extensions
            .iter()
            .filter(|extension| !extension.path.starts_with("<inline:"))
            .map(|extension| (extension.resolved_path.clone(), extension.clone()))
            .collect();
        let failed_preload_paths: HashSet<String> = pre_trust_extensions
            .errors
            .iter()
            .map(|error| self.resolve_extension_load_path(&error.path))
            .collect();
        let remaining_paths: Vec<String> = extension_paths
            .iter()
            .filter(|path| {
                let resolved_path = self.resolve_extension_load_path(path);
                ordered_get(&preloaded_by_path, &resolved_path).is_none()
                    && !failed_preload_paths.contains(&resolved_path)
            })
            .cloned()
            .collect();
        let remaining_extensions = load_extensions_cached(
            &remaining_paths,
            &self.cwd,
            Some(self.event_bus.clone()),
            Some(pre_trust_extensions.runtime.clone()),
            &*self.extension_module_loader,
        );
        for extension in &remaining_extensions.extensions {
            ordered_insert(
                &mut preloaded_by_path,
                extension.resolved_path.clone(),
                extension.clone(),
            );
        }

        let inline_extensions: Vec<Extension> = pre_trust_extensions
            .extensions
            .iter()
            .filter(|extension| extension.path.starts_with("<inline:"))
            .cloned()
            .collect();
        let mut ordered_extensions: Vec<Extension> = extension_paths
            .iter()
            .filter_map(|path| {
                let resolved = self.resolve_extension_load_path(path);
                ordered_get(&preloaded_by_path, &resolved).cloned()
            })
            .collect();
        ordered_extensions.extend(inline_extensions);

        let mut extensions_result = LoadExtensionsResult {
            extensions: ordered_extensions,
            errors: pre_trust_extensions
                .errors
                .iter()
                .chain(remaining_extensions.errors.iter())
                .cloned()
                .collect(),
            warnings: pre_trust_extensions
                .warnings
                .iter()
                .chain(remaining_extensions.warnings.iter())
                .cloned()
                .collect(),
            runtime: pre_trust_extensions.runtime.clone(),
        };
        add_extension_conflict_diagnostics(&mut extensions_result);
        Ok(extensions_result)
    }

    fn map_skill_path(
        &self,
        resource: &ResolvedResource,
        metadata_by_path: &mut Vec<(String, PathMetadata)>,
    ) -> String {
        if resource.metadata.source != "auto"
            && resource.metadata.origin != PathMetadataOrigin::Package
        {
            return resource.path.clone();
        }
        let Ok(stats) = fs::metadata(&resource.path) else {
            return resource.path.clone();
        };
        if !stats.is_dir() {
            return resource.path.clone();
        }
        let skill_file = node_join(&resource.path, &["SKILL.md"]);
        if Path::new(&skill_file).exists() {
            if ordered_get(metadata_by_path, &skill_file).is_none() {
                ordered_insert(
                    metadata_by_path,
                    skill_file.clone(),
                    resource.metadata.clone(),
                );
            }
            return skill_file;
        }
        resource.path.clone()
    }

    fn normalize_extension_paths(&self, entries: Vec<ResourcePathEntry>) -> Vec<ResourcePathEntry> {
        entries
            .into_iter()
            .map(|entry| {
                let metadata = match &entry.metadata.base_dir {
                    Some(base_dir) => PathMetadata {
                        base_dir: Some(self.resolve_resource_path(base_dir)),
                        ..entry.metadata.clone()
                    },
                    None => entry.metadata.clone(),
                };
                ResourcePathEntry {
                    path: self.resolve_resource_path(&entry.path),
                    metadata,
                }
            })
            .collect()
    }

    fn update_skills_from_paths(
        &mut self,
        skill_paths: &[String],
        metadata_by_path: &[(String, PathMetadata)],
    ) {
        let skills_result: LoadSkillsResult = if self.no_skills && skill_paths.is_empty() {
            LoadSkillsResult::default()
        } else {
            load_skills(LoadSkillsOptions {
                cwd: self.cwd.clone(),
                agent_dir: self.agent_dir.clone(),
                skill_paths: skill_paths.to_vec(),
                include_defaults: false,
            })
        };
        let resolved_skills = match &self.skills_override {
            Some(apply) => apply(skills_result),
            None => skills_result,
        };
        let mut skills = Vec::new();
        for mut skill in resolved_skills.skills {
            skill.source_info = self
                .find_source_info_for_path(
                    &skill.file_path,
                    Some(&self.extension_skill_source_infos),
                    Some(metadata_by_path),
                )
                .or_else(|| Some(skill.source_info.clone()))
                .unwrap_or_else(|| self.get_default_source_info_for_path(&skill.file_path));
            skills.push(skill);
        }
        self.skills = skills;
        self.skill_diagnostics = resolved_skills.diagnostics;
    }

    fn update_prompts_from_paths(
        &mut self,
        prompt_paths: &[String],
        metadata_by_path: &[(String, PathMetadata)],
    ) {
        let prompts_result: PromptsResult = if self.no_prompt_templates && prompt_paths.is_empty() {
            PromptsResult::default()
        } else {
            let all_prompts = load_prompt_templates(prompt_templates::LoadPromptTemplatesOptions {
                cwd: self.cwd.clone(),
                agent_dir: self.agent_dir.clone(),
                prompt_paths: prompt_paths.to_vec(),
                include_defaults: false,
            });
            dedupe_prompts(all_prompts)
        };
        let resolved_prompts = match &self.prompts_override {
            Some(apply) => apply(prompts_result),
            None => prompts_result,
        };
        let mut prompts = Vec::new();
        for mut prompt in resolved_prompts.prompts {
            prompt.source_info = self
                .find_source_info_for_path(
                    &prompt.file_path,
                    Some(&self.extension_prompt_source_infos),
                    Some(metadata_by_path),
                )
                .or_else(|| Some(prompt.source_info.clone()))
                .unwrap_or_else(|| self.get_default_source_info_for_path(&prompt.file_path));
            prompts.push(prompt);
        }
        self.prompts = prompts;
        self.prompt_diagnostics = resolved_prompts.diagnostics;
    }

    fn update_themes_from_paths(
        &mut self,
        theme_paths: &[String],
        metadata_by_path: &[(String, PathMetadata)],
    ) {
        let themes_result: ThemesResult = if self.no_themes && theme_paths.is_empty() {
            ThemesResult::default()
        } else {
            let loaded = self.load_themes(theme_paths, false);
            let deduped = dedupe_themes(loaded.themes);
            ThemesResult {
                themes: deduped.themes,
                diagnostics: loaded
                    .diagnostics
                    .into_iter()
                    .chain(deduped.diagnostics)
                    .collect(),
            }
        };
        let resolved_themes = match &self.themes_override {
            Some(apply) => apply(themes_result),
            None => themes_result,
        };
        let mut themes = Vec::new();
        for mut theme in resolved_themes.themes {
            if let Some(source_path) = theme.source_path.clone() {
                theme.source_info = self
                    .find_source_info_for_path(
                        &source_path,
                        Some(&self.extension_theme_source_infos),
                        Some(metadata_by_path),
                    )
                    .or_else(|| theme.source_info.clone())
                    .or_else(|| Some(self.get_default_source_info_for_path(&source_path)));
            }
            themes.push(theme);
        }
        self.themes = themes;
        self.theme_diagnostics = resolved_themes.diagnostics;
    }

    /// Upstream `applyExtensionSourceInfo`.
    fn apply_extension_source_info(
        &self,
        extensions_result: &mut LoadExtensionsResult,
        metadata_by_path: &[(String, PathMetadata)],
    ) {
        for extension in &mut extensions_result.extensions {
            extension.source_info = self
                .find_source_info_for_path(&extension.path, None, Some(metadata_by_path))
                .unwrap_or_else(|| self.get_default_source_info_for_path(&extension.path));
            for command in extension.commands.values_mut() {
                command.source_info = extension.source_info.clone();
            }
            for tool in extension.tools.values_mut() {
                tool.source_info = extension.source_info.clone();
            }
        }
    }

    fn find_source_info_for_path(
        &self,
        resource_path: &str,
        extra_source_infos: Option<&[(String, SourceInfo)]>,
        metadata_by_path: Option<&[(String, PathMetadata)]>,
    ) -> Option<SourceInfo> {
        if resource_path.is_empty() {
            return None;
        }

        if resource_path.starts_with('<') {
            return Some(self.get_default_source_info_for_path(resource_path));
        }

        let normalized_resource_path = resolve_auto(resource_path);
        if let Some(extra_source_infos) = extra_source_infos {
            for (source_path, source_info) in extra_source_infos {
                let normalized_source_path = resolve_auto(source_path);
                if normalized_resource_path == normalized_source_path
                    || normalized_resource_path
                        .starts_with(&format!("{normalized_source_path}{PATH_SEP}"))
                {
                    return Some(SourceInfo {
                        path: resource_path.to_string(),
                        ..source_info.clone()
                    });
                }
            }
        }

        if let Some(metadata_by_path) = metadata_by_path {
            let exact = ordered_get(metadata_by_path, &normalized_resource_path)
                .or_else(|| ordered_get(metadata_by_path, resource_path));
            if let Some(metadata) = exact {
                return Some(create_source_info(resource_path, metadata));
            }

            for (source_path, metadata) in metadata_by_path {
                let normalized_source_path = resolve_auto(source_path);
                if normalized_resource_path == normalized_source_path
                    || normalized_resource_path
                        .starts_with(&format!("{normalized_source_path}{PATH_SEP}"))
                {
                    return Some(create_source_info(resource_path, metadata));
                }
            }
        }

        None
    }

    fn get_default_source_info_for_path(&self, file_path: &str) -> SourceInfo {
        default_source_info_for_path(file_path, &self.agent_dir, &self.cwd)
    }

    fn merge_paths(&self, primary: &[String], additional: &[String]) -> Vec<String> {
        let mut merged: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for p in primary.iter().chain(additional.iter()) {
            let resolved = self.resolve_resource_path(p);
            let canonical_path = canonicalize_path(&resolved);
            if seen.contains(&canonical_path) {
                continue;
            }
            seen.insert(canonical_path);
            merged.push(resolved);
        }

        merged
    }

    fn resolve_resource_path(&self, p: &str) -> String {
        resolve_path_with(
            p,
            &self.cwd,
            &PathInputOptions {
                trim: true,
                ..PathInputOptions::default()
            },
            cfg!(windows),
        )
        .unwrap_or_else(|_| p.to_string())
    }

    fn load_themes(&self, paths: &[String], include_defaults: bool) -> ThemesResult {
        let mut themes: Vec<Theme> = Vec::new();
        let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
        if include_defaults {
            let default_dirs = [
                node_join(&self.agent_dir, &["themes"]),
                node_join(&self.cwd, &[CONFIG_DIR_NAME, "themes"]),
            ];

            for dir in &default_dirs {
                self.load_themes_from_dir(dir, &mut themes, &mut diagnostics);
            }
        }

        for p in paths {
            let resolved = self.resolve_resource_path(p);
            if !Path::new(&resolved).exists() {
                diagnostics.push(warning("theme path does not exist", &resolved));
                continue;
            }

            match fs::metadata(&resolved) {
                Ok(stats) => {
                    if stats.is_dir() {
                        self.load_themes_from_dir(&resolved, &mut themes, &mut diagnostics);
                    } else if stats.is_file() && resolved.ends_with(".json") {
                        self.load_theme_from_file(&resolved, &mut themes, &mut diagnostics);
                    } else {
                        diagnostics.push(warning("theme path is not a json file", &resolved));
                    }
                }
                Err(error) => {
                    diagnostics.push(warning(error.to_string(), &resolved));
                }
            }
        }

        ThemesResult {
            themes,
            diagnostics,
        }
    }

    fn load_themes_from_dir(
        &self,
        dir: &str,
        themes: &mut Vec<Theme>,
        diagnostics: &mut Vec<ResourceDiagnostic>,
    ) {
        if !Path::new(dir).exists() {
            return;
        }

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(warning(error.to_string(), dir));
                return;
            }
        };
        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            let full_path = node_join(dir, &[&entry_name]);
            let mut is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                match fs::metadata(&full_path) {
                    Ok(stats) => is_file = stats.is_file(),
                    Err(_) => continue,
                }
            }
            if !is_file {
                continue;
            }
            if !entry_name.ends_with(".json") {
                continue;
            }
            self.load_theme_from_file(&full_path, themes, diagnostics);
        }
    }

    fn load_theme_from_file(
        &self,
        file_path: &str,
        themes: &mut Vec<Theme>,
        diagnostics: &mut Vec<ResourceDiagnostic>,
    ) {
        match load_theme_from_path(file_path) {
            Ok(theme) => themes.push(theme),
            Err(message) => diagnostics.push(warning(message, file_path)),
        }
    }

    fn load_extension_factories(&self, runtime: &ExtensionRuntime) -> LoadExtensionsResult {
        let mut extensions: Vec<Extension> = Vec::new();
        let mut errors: Vec<ExtensionLoadError> = Vec::new();

        for (index, input) in self.extension_factories.iter().enumerate() {
            let (is_named, factory, name, hidden) = match input {
                InlineExtension::Factory(factory) => {
                    (false, factory.clone(), (index + 1).to_string(), false)
                }
                InlineExtension::Named {
                    factory,
                    name,
                    hidden,
                } => (true, factory.clone(), name.clone(), *hidden),
            };
            let extension_path = format!("<inline:{name}>");
            match load_extension_from_factory(
                factory,
                &self.cwd,
                self.event_bus.clone(),
                runtime,
                Some(&extension_path),
            ) {
                Ok(mut extension) => {
                    extension.hidden = is_named && hidden;
                    extensions.push(extension);
                }
                Err(message) => {
                    errors.push(ExtensionLoadError {
                        path: extension_path,
                        error: message,
                    });
                }
            }
        }

        LoadExtensionsResult {
            extensions,
            errors,
            warnings: Vec::new(),
            runtime: runtime.clone(),
        }
    }

    fn discover_system_prompt_file(&self) -> Option<String> {
        let project_path = node_join(&self.cwd, &[CONFIG_DIR_NAME, "SYSTEM.md"]);
        if self.settings_manager.is_project_trusted() && Path::new(&project_path).exists() {
            return Some(project_path);
        }

        let global_path = node_join(&self.agent_dir, &["SYSTEM.md"]);
        if Path::new(&global_path).exists() {
            return Some(global_path);
        }

        None
    }

    fn discover_append_system_prompt_file(&self) -> Option<String> {
        let project_path = node_join(&self.cwd, &[CONFIG_DIR_NAME, "APPEND_SYSTEM.md"]);
        if self.settings_manager.is_project_trusted() && Path::new(&project_path).exists() {
            return Some(project_path);
        }

        let global_path = node_join(&self.agent_dir, &["APPEND_SYSTEM.md"]);
        if Path::new(&global_path).exists() {
            return Some(global_path);
        }

        None
    }
}

// ===========================================================================
// Free helpers (mirror upstream private methods that need no loader state)
// ===========================================================================

fn default_source_info_for_path(file_path: &str, agent_dir: &str, cwd: &str) -> SourceInfo {
    if file_path.starts_with('<') && file_path.ends_with('>') {
        let inner = &file_path[1..file_path.len() - 1];
        let source = inner.split(':').next().unwrap_or("");
        return SourceInfo {
            path: file_path.to_string(),
            source: if source.is_empty() {
                "temporary".to_string()
            } else {
                source.to_string()
            },
            scope: SourceScope::Temporary,
            origin: SourceOrigin::TopLevel,
            base_dir: None,
        };
    }

    let normalized_path = resolve_auto(file_path);
    let agent_roots = [
        node_join(agent_dir, &["skills"]),
        node_join(agent_dir, &["prompts"]),
        node_join(agent_dir, &["themes"]),
        node_join(agent_dir, &["extensions"]),
    ];
    let project_roots = [
        node_join(cwd, &[CONFIG_DIR_NAME, "skills"]),
        node_join(cwd, &[CONFIG_DIR_NAME, "prompts"]),
        node_join(cwd, &[CONFIG_DIR_NAME, "themes"]),
        node_join(cwd, &[CONFIG_DIR_NAME, "extensions"]),
    ];

    for root in &agent_roots {
        if is_under_path(&normalized_path, root) {
            return SourceInfo {
                path: file_path.to_string(),
                source: "local".to_string(),
                scope: SourceScope::User,
                origin: SourceOrigin::TopLevel,
                base_dir: Some(root.clone()),
            };
        }
    }

    for root in &project_roots {
        if is_under_path(&normalized_path, root) {
            return SourceInfo {
                path: file_path.to_string(),
                source: "local".to_string(),
                scope: SourceScope::Project,
                origin: SourceOrigin::TopLevel,
                base_dir: Some(root.clone()),
            };
        }
    }

    // Upstream statSync throws for missing paths (the callers guarantee
    // existence); the port mirrors that with an expect.
    let is_directory = fs::metadata(&normalized_path)
        .map(|stats| stats.is_dir())
        .expect("statSync failed in getDefaultSourceInfoForPath");
    SourceInfo {
        path: file_path.to_string(),
        source: "local".to_string(),
        scope: SourceScope::Temporary,
        origin: SourceOrigin::TopLevel,
        base_dir: Some(if is_directory {
            normalized_path
        } else {
            node_resolve(&[&normalized_path, ".."])
        }),
    }
}

/// Upstream `detectExtensionConflicts` + `addExtensionConflictDiagnostics`:
/// conflicts are reported as errors while every extension stays loaded, and
/// precedence is handled by load order.
fn add_extension_conflict_diagnostics(extensions_result: &mut LoadExtensionsResult) {
    let mut tool_owners: Vec<(String, String)> = Vec::new();
    let mut flag_owners: Vec<(String, String)> = Vec::new();
    let mut conflicts: Vec<(String, String)> = Vec::new();

    for ext in &extensions_result.extensions {
        // Check tools
        for tool_name in ext.tools.keys() {
            let existing_owner = ordered_get(&tool_owners, tool_name).cloned();
            match existing_owner {
                Some(owner) if owner != ext.path => {
                    conflicts.push((
                        ext.path.clone(),
                        format!("Tool \"{tool_name}\" conflicts with {owner}"),
                    ));
                }
                _ => {
                    ordered_insert(&mut tool_owners, tool_name.to_string(), ext.path.clone());
                }
            }
        }

        // Check flags
        for flag_name in ext.flags.keys() {
            let existing_owner = ordered_get(&flag_owners, flag_name).cloned();
            match existing_owner {
                Some(owner) if owner != ext.path => {
                    conflicts.push((
                        ext.path.clone(),
                        format!("Flag \"--{flag_name}\" conflicts with {owner}"),
                    ));
                }
                _ => {
                    ordered_insert(&mut flag_owners, flag_name.to_string(), ext.path.clone());
                }
            }
        }
    }

    for (path, message) in conflicts {
        extensions_result.errors.push(ExtensionLoadError {
            path,
            error: message,
        });
    }
}

fn dedupe_prompts(prompts: Vec<PromptTemplate>) -> PromptsResult {
    let mut seen: Vec<(String, PromptTemplate)> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    for prompt in prompts {
        let existing = ordered_get(&seen, &prompt.name).cloned();
        match existing {
            Some(existing) => {
                diagnostics.push(ResourceDiagnostic {
                    r#type: ResourceDiagnosticType::Collision,
                    message: format!("name \"/{}\" collision", prompt.name),
                    path: Some(prompt.file_path.clone()),
                    collision: Some(ResourceCollision {
                        resource_type: ResourceCollisionType::Prompt,
                        name: prompt.name.clone(),
                        winner_path: existing.file_path.clone(),
                        loser_path: prompt.file_path.clone(),
                        winner_source: None,
                        loser_source: None,
                    }),
                });
            }
            None => ordered_insert(&mut seen, prompt.name.clone(), prompt),
        }
    }

    PromptsResult {
        prompts: seen.into_iter().map(|(_, prompt)| prompt).collect(),
        diagnostics,
    }
}

/// JS `${name}` template rendering for a JSON scalar (used by
/// `dedupeThemes`; non-scalar JSON renders like `[object Object]`).
fn theme_name_display(name: &serde_json::Value) -> String {
    match name {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => "[object Object]".to_string(),
    }
}

fn dedupe_themes(themes: Vec<Theme>) -> ThemesResult {
    let mut seen: Vec<(String, Theme)> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    for t in themes {
        let name = match &t.name {
            None | Some(serde_json::Value::Null) => "unnamed".to_string(),
            Some(value) => theme_name_display(value),
        };
        let existing = ordered_get(&seen, &name).cloned();
        match existing {
            Some(existing) => {
                diagnostics.push(ResourceDiagnostic {
                    r#type: ResourceDiagnosticType::Collision,
                    message: format!("name \"{name}\" collision"),
                    path: t.source_path.clone(),
                    collision: Some(ResourceCollision {
                        resource_type: ResourceCollisionType::Theme,
                        name: name.clone(),
                        winner_path: existing
                            .source_path
                            .clone()
                            .unwrap_or_else(|| "<builtin>".to_string()),
                        loser_path: t
                            .source_path
                            .clone()
                            .unwrap_or_else(|| "<builtin>".to_string()),
                        winner_source: None,
                        loser_source: None,
                    }),
                });
            }
            None => ordered_insert(&mut seen, name, t),
        }
    }

    ThemesResult {
        themes: seen.into_iter().map(|(_, theme)| theme).collect(),
        diagnostics,
    }
}

#[cfg(test)]
#[path = "resource_loader_tests.rs"]
mod tests;
