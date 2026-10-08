//! Port of upstream `coding-agent/src/core/skills.ts` (skill discovery,
//! validation, and system-prompt formatting; upstream SHA256 at migration
//! time: `055dbfde974fd1951267dd6f9204b5d713ff0004e0991eb870fce9158c6e359a`).
//!
//! Deterministic outputs (per-fixture discovery order, warning/collision
//! diagnostics with their exact texts, `formatSkillsForPrompt` bytes) were
//! captured from the real upstream source under node (type stripping) into
//! `tests/fixtures/core_oracle_w38/skills.oracle.json` (generator
//! `oracle_skills.mjs`); the tests pin them byte-for-byte.
//!
//! Shared seams (upstream imports modules outside this slice; disclosed):
//!
//! - **npm `ignore`** (gitignore matching): reused from the vendored
//!   [`crate::coding_agent::package_manager::vendor::IgnoreMatcher`] — the
//!   same npm-package semantics the package-manager slice vendored. The
//!   skills-local `prefixIgnorePattern` helper is ported here (skills.ts
//!   carries its own copy) and feeds the vendored matcher.
//! - `getAgentDir` fallback: [`crate::coding_agent::core::get_agent_dir`].
//! - Frontmatter parsing via the ported utils frontmatter scanner.
//!
//! Divergences (disclosed):
//!
//! 1. **YAML scanner prose**: upstream surfaces the npm `yaml` parser's
//!    error message verbatim in the diagnostic; the port's scanner prose
//!    differs (same policy as the frontmatter utils port). Invalid-YAML
//!    diagnostics are pinned by type + path, not message bytes.
//! 2. **File-read error prose**: `readFileSync` errno messages ("ENOENT: …")
//!    are platform/JS-owned; the port renders `std::io::Error` text. Not
//!    pinned.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::coding_agent::core::diagnostics::{
    ResourceCollision, ResourceCollisionType, ResourceDiagnostic, ResourceDiagnosticType,
};
use crate::coding_agent::extensions::types::{
    create_synthetic_source_info, SourceInfo, SourceScope,
};
use crate::coding_agent::package_manager::vendor::IgnoreMatcher;
use crate::coding_agent::utils::frontmatter::{parse_frontmatter, FrontmatterValue};
use crate::coding_agent::utils::paths::{
    canonicalize_path, resolve_path_auto_base, resolve_path_with, PathInputOptions,
};

use super::CONFIG_DIR_NAME;

/// Max name length per spec
const MAX_NAME_LENGTH: usize = 64;

/// Max description length per spec
const MAX_DESCRIPTION_LENGTH: usize = 1024;

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

pub type IgnoreRules = Vec<String>;

// ===========================================================================
// node path shims (host platform)
// ===========================================================================

fn node_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `path.relative(from, to)` on the host platform.
fn node_relative(from: &str, to: &str) -> String {
    let cwd = node_cwd();
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_relative(from, to, &cwd)
    } else {
        crate::coding_agent::utils::node_path::posix_relative(from, to, &cwd)
    }
}

/// `path.dirname(path)` on the host platform.
pub(crate) fn node_dirname(path: &str) -> String {
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

/// `path.basename(path)` on the host platform.
pub(crate) fn node_basename(path: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    match path.trim_end_matches(separators).rfind(separators) {
        Some(index) => path[index + 1..].to_string(),
        None => path.trim_end_matches(separators).to_string(),
    }
}

pub(crate) fn node_join(base: &str, segments: &[&str]) -> String {
    let mut parts = vec![base];
    parts.extend_from_slice(segments);
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&parts)
    } else {
        crate::coding_agent::utils::node_path::posix_join(&parts)
    }
}

fn to_posix_path(p: &str) -> String {
    if cfg!(windows) {
        p.replace('\\', "/")
    } else {
        p.to_string()
    }
}

fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line.to_string();
    let mut negated = false;

    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest.to_string();
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest.to_string();
    }

    if let Some(rest) = pattern.strip_prefix('/') {
        pattern = rest.to_string();
    }

    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

fn add_ignore_rules(rules: &mut IgnoreRules, dir: &str, root_dir: &str) {
    let relative_dir = node_relative(root_dir, dir);
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{}/", to_posix_path(&relative_dir))
    };

    for filename in IGNORE_FILE_NAMES {
        let ignore_path = node_join(dir, &[filename]);
        if !Path::new(&ignore_path).exists() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&ignore_path) else {
            continue;
        };
        let patterns: Vec<String> = content
            .split(['\n', '\r'])
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        if !patterns.is_empty() {
            rules.extend(patterns);
        }
    }
}

/// `ig.ignores(path)` against the accumulated rules, evaluated relative to
/// `root_dir` (the vendored matcher is rooted; upstream tests relative paths
/// against the same root).
fn ignores(rules: &[String], root_dir: &str, absolute_path: &str, is_dir: bool) -> bool {
    if rules.is_empty() {
        return false;
    }
    IgnoreMatcher::from_rules(root_dir, rules).ignores(absolute_path, is_dir)
}

// ===========================================================================
// Skill records
// ===========================================================================

/// Upstream `Skill`.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub source_info: SourceInfo,
    pub disable_model_invocation: bool,
}

/// Upstream `LoadSkillsResult`.
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

fn warning(message: impl Into<String>, path: &str) -> ResourceDiagnostic {
    ResourceDiagnostic {
        r#type: ResourceDiagnosticType::Warning,
        message: message.into(),
        path: Some(path.to_string()),
        collision: None,
    }
}

/// JS string length (UTF-16 code units).
fn js_length(value: &str) -> usize {
    value.encode_utf16().count()
}

/// Validate skill name per Agent Skills spec.
/// Returns array of validation error messages (empty if valid).
fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();

    if js_length(name) > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            js_length(name)
        ));
    }

    let valid_chars = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_chars {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }

    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }

    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }

    errors
}

/// Validate description per Agent Skills spec.
fn validate_description(description: Option<&FrontmatterValue>) -> Vec<String> {
    let mut errors = Vec::new();

    match description {
        Some(FrontmatterValue::Str(description)) if !description.trim().is_empty() => {
            if js_length(description) > MAX_DESCRIPTION_LENGTH {
                errors.push(format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
                    js_length(description)
                ));
            }
        }
        _ => errors.push("description is required".to_string()),
    }

    errors
}

pub struct LoadSkillsFromDirOptions<'a> {
    /// Directory to scan for skills
    pub dir: &'a str,
    /// Source identifier for these skills
    pub source: &'a str,
}

fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
    match source {
        "user" => create_synthetic_source_info(
            file_path,
            "local",
            Some(SourceScope::User),
            None,
            Some(base_dir.to_string()),
        ),
        "project" => create_synthetic_source_info(
            file_path,
            "local",
            Some(SourceScope::Project),
            None,
            Some(base_dir.to_string()),
        ),
        "path" => {
            create_synthetic_source_info(file_path, "local", None, None, Some(base_dir.to_string()))
        }
        other => {
            create_synthetic_source_info(file_path, other, None, None, Some(base_dir.to_string()))
        }
    }
}

/// Load skills from a directory.
///
/// Discovery rules:
/// - if a directory contains SKILL.md, treat it as a skill root and do not recurse further
/// - otherwise, load direct .md children in the root
/// - recurse into subdirectories to find SKILL.md
pub fn load_skills_from_dir(options: LoadSkillsFromDirOptions<'_>) -> LoadSkillsResult {
    let LoadSkillsFromDirOptions { dir, source } = options;
    load_skills_from_dir_internal(dir, source, true, &mut Vec::new(), None)
}

struct DirEntryInfo {
    name: String,
    full_path: String,
    is_symlink: bool,
    /// Dirent `isFile()` — lstat semantics, symlinks report false.
    is_file: bool,
    /// Dirent `isDirectory()` — lstat semantics, symlinks report false.
    is_dir: bool,
}

fn read_dir_entries(dir: &str) -> Option<Vec<DirEntryInfo>> {
    let entries = fs::read_dir(dir).ok()?;
    let mut infos = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let file_type = entry.file_type().ok()?;
        infos.push(DirEntryInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            full_path: node_join(dir, &[&entry.file_name().to_string_lossy()]),
            is_symlink: file_type.is_symlink(),
            is_file: file_type.is_file(),
            is_dir: file_type.is_dir(),
        });
    }
    Some(infos)
}

fn load_skills_from_dir_internal(
    dir: &str,
    source: &str,
    include_root_files: bool,
    rules: &mut IgnoreRules,
    root_dir: Option<&str>,
) -> LoadSkillsResult {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    if !Path::new(dir).exists() {
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    let root = root_dir.unwrap_or(dir).to_string();
    add_ignore_rules(rules, dir, &root);

    let Some(entries) = read_dir_entries(dir) else {
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    };

    for entry in &entries {
        if entry.name != "SKILL.md" {
            continue;
        }

        let mut is_file = entry.is_file;
        if entry.is_symlink {
            match fs::metadata(&entry.full_path) {
                Ok(stats) => is_file = stats.is_file(),
                Err(_) => continue,
            }
        }

        if !is_file || ignores(rules, &root, &entry.full_path, false) {
            continue;
        }

        let result = load_skill_from_file(&entry.full_path, source);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
        diagnostics.extend(result.diagnostics);
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    for entry in &entries {
        if entry.name.starts_with('.') {
            continue;
        }

        // Skip node_modules to avoid scanning dependencies
        if entry.name == "node_modules" {
            continue;
        }

        // For symlinks, check if they point to a directory and follow them
        let mut is_directory = entry.is_dir;
        let mut is_file = entry.is_file;
        if entry.is_symlink {
            match fs::metadata(&entry.full_path) {
                Ok(stats) => {
                    is_directory = stats.is_dir();
                    is_file = stats.is_file();
                }
                Err(_) => {
                    // Broken symlink, skip it
                    continue;
                }
            }
        }

        if ignores(rules, &root, &entry.full_path, is_directory) {
            continue;
        }

        if is_directory {
            let sub_result =
                load_skills_from_dir_internal(&entry.full_path, source, false, rules, Some(&root));
            skills.extend(sub_result.skills);
            diagnostics.extend(sub_result.diagnostics);
            continue;
        }

        if !is_file || !include_root_files || !entry.name.ends_with(".md") {
            continue;
        }

        let result = load_skill_from_file(&entry.full_path, source);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
        diagnostics.extend(result.diagnostics);
    }

    LoadSkillsResult {
        skills,
        diagnostics,
    }
}

struct LoadedSkillFile {
    skill: Option<Skill>,
    diagnostics: Vec<ResourceDiagnostic>,
}

fn load_skill_from_file(file_path: &str, source: &str) -> LoadedSkillFile {
    let mut diagnostics = Vec::new();
    let is_declared_skill = node_basename(file_path) == "SKILL.md";

    let raw_content = match fs::read(file_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            diagnostics.push(warning(error.to_string(), file_path));
            return LoadedSkillFile {
                skill: None,
                diagnostics,
            };
        }
    };

    let frontmatter = match parse_frontmatter(&raw_content) {
        Ok(parsed) => parsed.frontmatter,
        Err(error) => {
            if is_declared_skill {
                diagnostics.push(warning(error.to_string(), file_path));
            }
            return LoadedSkillFile {
                skill: None,
                diagnostics,
            };
        }
    };

    let description = frontmatter.get("description");
    let description_str = match description {
        Some(FrontmatterValue::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let has_description = description_str.is_some_and(|s| !s.trim().is_empty());
    if !is_declared_skill && !has_description {
        return LoadedSkillFile {
            skill: None,
            diagnostics,
        };
    }

    let skill_dir = node_dirname(file_path);
    let parent_dir_name = node_basename(&skill_dir);

    // Validate description
    for error in validate_description(description) {
        diagnostics.push(warning(error, file_path));
    }

    // Use name from frontmatter, or fall back to parent directory name
    let frontmatter_name = match frontmatter.get("name") {
        Some(FrontmatterValue::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let name = frontmatter_name.unwrap_or_else(|| parent_dir_name.clone());

    // Validate name
    for error in validate_name(&name) {
        diagnostics.push(warning(error, file_path));
    }

    // Still load the skill even with warnings, unless description is missing or empty.
    if !has_description {
        return LoadedSkillFile {
            skill: None,
            diagnostics,
        };
    }

    let disable_model_invocation =
        frontmatter.get("disable-model-invocation") == Some(&FrontmatterValue::Bool(true));

    LoadedSkillFile {
        skill: Some(Skill {
            name,
            description: description_str.unwrap_or_default().to_string(),
            file_path: file_path.to_string(),
            base_dir: skill_dir.clone(),
            source_info: create_skill_source_info(file_path, &skill_dir, source),
            disable_model_invocation,
        }),
        diagnostics,
    }
}

// ===========================================================================
// Prompt formatting
// ===========================================================================

/// Upstream `fileReadTool` parameter (`"read" | "bash"`, default `"read"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileReadTool {
    #[default]
    Read,
    Bash,
}

/// Format skills for inclusion in a system prompt.
/// Uses XML format per Agent Skills standard.
/// See: <https://agentskills.io/integrate-skills>
///
/// Skills with disableModelInvocation=true are excluded from the prompt
/// (they can only be invoked explicitly via /skill:name commands).
pub fn format_skills_for_prompt(skills: &[Skill], file_read_tool: FileReadTool) -> String {
    let visible_skills: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();

    if visible_skills.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        if file_read_tool == FileReadTool::Read {
            "Use the read tool to load a skill's file when the task matches its description.".to_string()
        } else {
            "Use bash to load a skill's file when the task matches its description.".to_string()
        },
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible_skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());

    lines.join("\n")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ===========================================================================
// loadSkills
// ===========================================================================

pub struct LoadSkillsOptions {
    /// Working directory for project-local skills.
    pub cwd: String,
    /// Agent config directory for global skills.
    pub agent_dir: String,
    /// Explicit skill paths (files or directories)
    pub skill_paths: Vec<String>,
    /// Include default skills directories.
    pub include_defaults: bool,
}

fn is_under_path(target: &str, root: &str) -> bool {
    let normalized_root = resolve_path_auto_base(root).unwrap_or_else(|_| root.to_string());
    if target == normalized_root {
        return true;
    }
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let prefix = if normalized_root.ends_with(sep) {
        normalized_root.clone()
    } else {
        format!("{normalized_root}{sep}")
    };
    target.starts_with(&prefix)
}

/// Load skills from all configured locations.
/// Returns skills and any validation diagnostics.
pub fn load_skills(options: LoadSkillsOptions) -> LoadSkillsResult {
    let LoadSkillsOptions {
        cwd,
        agent_dir,
        skill_paths,
        include_defaults,
    } = options;

    // Resolve agentDir - if not provided, use default from config
    let resolved_cwd = resolve_path_auto_base(&cwd).unwrap_or_else(|_| cwd.clone());
    let resolved_agent_dir =
        resolve_path_auto_base(&agent_dir).unwrap_or_else(|_| agent_dir.clone());

    let mut skill_map: Vec<(String, Skill)> = Vec::new();
    let mut real_path_set: HashSet<String> = HashSet::new();
    let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();
    let mut collision_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    fn add_skills(
        result: LoadSkillsResult,
        skill_map: &mut Vec<(String, Skill)>,
        real_path_set: &mut HashSet<String>,
        collision_diagnostics: &mut Vec<ResourceDiagnostic>,
    ) {
        for skill in result.skills {
            // Resolve symlinks to detect duplicate files
            let real_path = canonicalize_path(&skill.file_path);

            // Skip silently if we've already loaded this exact file (via symlink)
            if real_path_set.contains(&real_path) {
                continue;
            }

            let existing = skill_map.iter().find(|(name, _)| *name == skill.name);
            match existing {
                Some((_, existing)) => {
                    collision_diagnostics.push(ResourceDiagnostic {
                        r#type: ResourceDiagnosticType::Collision,
                        message: format!("name \"{}\" collision", skill.name),
                        path: Some(skill.file_path.clone()),
                        collision: Some(ResourceCollision {
                            resource_type: ResourceCollisionType::Skill,
                            name: skill.name.clone(),
                            winner_path: existing.file_path.clone(),
                            loser_path: skill.file_path.clone(),
                            winner_source: None,
                            loser_source: None,
                        }),
                    });
                }
                None => {
                    real_path_set.insert(real_path);
                    skill_map.push((skill.name.clone(), skill));
                }
            }
        }
    }

    if include_defaults {
        add_skills(
            load_skills_from_dir_internal(
                &node_join(&resolved_agent_dir, &["skills"]),
                "user",
                true,
                &mut Vec::new(),
                None,
            ),
            &mut skill_map,
            &mut real_path_set,
            &mut collision_diagnostics,
        );
        add_skills(
            load_skills_from_dir_internal(
                &node_join(&resolved_cwd, &[CONFIG_DIR_NAME, "skills"]),
                "project",
                true,
                &mut Vec::new(),
                None,
            ),
            &mut skill_map,
            &mut real_path_set,
            &mut collision_diagnostics,
        );
    }

    let user_skills_dir = node_join(&resolved_agent_dir, &["skills"]);
    let project_skills_dir = node_join(&resolved_cwd, &[CONFIG_DIR_NAME, "skills"]);

    let get_source = |resolved_path: &str| -> &'static str {
        if !include_defaults {
            if is_under_path(resolved_path, &user_skills_dir) {
                return "user";
            }
            if is_under_path(resolved_path, &project_skills_dir) {
                return "project";
            }
        }
        "path"
    };

    for raw_path in &skill_paths {
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
        if !Path::new(&resolved_path).exists() {
            all_diagnostics.push(warning("skill path does not exist", &resolved_path));
            continue;
        }

        let stats = match fs::metadata(&resolved_path) {
            Ok(stats) => stats,
            Err(error) => {
                all_diagnostics.push(warning(error.to_string(), &resolved_path));
                continue;
            }
        };
        let source = get_source(&resolved_path);
        if stats.is_dir() {
            add_skills(
                load_skills_from_dir_internal(&resolved_path, source, true, &mut Vec::new(), None),
                &mut skill_map,
                &mut real_path_set,
                &mut collision_diagnostics,
            );
        } else if stats.is_file() && resolved_path.ends_with(".md") {
            let result = load_skill_from_file(&resolved_path, source);
            match result.skill {
                Some(skill) => {
                    add_skills(
                        LoadSkillsResult {
                            skills: vec![skill],
                            diagnostics: result.diagnostics,
                        },
                        &mut skill_map,
                        &mut real_path_set,
                        &mut collision_diagnostics,
                    );
                }
                None => all_diagnostics.extend(result.diagnostics),
            }
        } else {
            all_diagnostics.push(warning("skill path is not a markdown file", &resolved_path));
        }
    }

    all_diagnostics.extend(collision_diagnostics);
    LoadSkillsResult {
        skills: skill_map.into_iter().map(|(_, skill)| skill).collect(),
        diagnostics: all_diagnostics,
    }
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
