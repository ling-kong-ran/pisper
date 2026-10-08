//! Port of `packages/agent/src/harness/skills.ts` (396 lines): SKILL.md
//! discovery and parsing, ignore-file filtering, frontmatter validation, and
//! skill invocation formatting. Oracle: `packages/agent/test/harness/skills.test.ts`.
//!
//! Disclosed substitutions:
//! - **Ignore matching (`skills.ts:1`, 185-250).** Upstream filters entries
//!   with the npm `ignore` package's in-memory `ignore().add(lines).ignores(path)`;
//!   no directory walking is involved. The port uses the same style of
//!   in-memory gitignore matching via the ripgrep `ignore` crate's
//!   [`ignore::gitignore::GitignoreBuilder`] / [`ignore::gitignore::Gitignore`]
//!   (pinned exact in `Cargo.toml`). Two shape mappings:
//!   upstream passes directory paths with a trailing slash, the port passes
//!   the plain relative path with an `is_dir` flag; and because the upstream
//!   matcher is one shared object that accumulates rules while the recursion
//!   descends (later siblings observe earlier siblings' ignore files), the
//!   port accumulates the prefixed pattern lines in a shared list and rebuilds
//!   the matcher at each check, so every check observes all rules added so
//!   far — the same set upstream would see. Lines that do not parse as globs
//!   are skipped; upstream would throw from `ignore.add`, which only matters
//!   for malformed ignore files that never occur in practice.
//! - **Frontmatter YAML (`skills.ts:332-346`).** Upstream parses with the npm
//!   `yaml` package; the port uses `yaml-rust2` (pinned exact). Parse errors
//!   keep the `parse_failed` diagnostic code, but the message text comes from
//!   the Rust scanner, not the upstream YAML parser. An empty or non-mapping
//!   document yields no fields, like upstream's `parse(...) ?? {}` followed by
//!   `undefined` property reads. Only `name` (string), `description`
//!   (string), and `disable-model-invocation` (strictly `true`) are read, as
//!   upstream.
//! - **`localeCompare` (`skills.ts:159`).** Approximated as case-insensitive
//!   ordering with a case-sensitive tiebreak; full ICU collation is not
//!   reimplemented.
//! - **String lengths (`skills.ts:310-330`).** Upstream `String.length`
//!   counts UTF-16 units; the port counts `char`s (identical for the ASCII
//!   names the character validation admits).
//! - **`loadSourcedSkills` generics (`skills.ts:86-108`).** Upstream
//!   `TSkill extends Skill` (a widening cast when no mapper is given) becomes
//!   `TSkill: From<Skill>` plus an optional explicit mapper closure.
//! - `parseFrontmatter` is duplicated in [`crate::agent_core::harness::prompt_templates`],
//!   mirroring the upstream files, which also duplicate it.

use std::sync::Mutex;

use futures::future::BoxFuture;
use ignore::gitignore::GitignoreBuilder;
use ignore::Match;
use yaml_rust2::{Yaml, YamlLoader};

use crate::agent_core::harness::types::{
    FileErrorCode, FileInfo, FileKind, FileSystem, Skill, SourcedInput,
};
use crate::agent_core::harness::Context;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// Upstream `SkillDiagnosticCode` (`skills.ts:12-17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticCode {
    FileInfoFailed,
    ListFailed,
    ReadFailed,
    ParseFailed,
    InvalidMetadata,
}

/// Warning produced while loading skills (upstream `SkillDiagnostic`,
/// `skills.ts:19-29`). The `type` field is always `"warning"` today.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDiagnostic {
    /// Diagnostic severity; only warnings are emitted (`type` is a reserved
    /// word in Rust, hence the raw identifier).
    pub r#type: String,
    /// Stable diagnostic code.
    pub code: SkillDiagnosticCode,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Path associated with the diagnostic.
    pub path: String,
}

impl SkillDiagnostic {
    fn warning(
        code: SkillDiagnosticCode,
        message: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        SkillDiagnostic {
            r#type: "warning".to_string(),
            code,
            message: message.into(),
            path: path.into(),
        }
    }
}

/// Upstream `{ skill, source }` record returned by
/// [`load_sourced_skills`] (`skills.ts:90-94`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourcedSkill<TSkill = Skill, TSource = ()> {
    pub skill: TSkill,
    pub source: TSource,
}

/// Upstream `SkillDiagnostic & { source }` record (`skills.ts:93`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourcedDiagnostic<TDiagnostic, TSource> {
    #[serde(flatten)]
    pub diagnostic: TDiagnostic,
    pub source: TSource,
}

/// Upstream `{ skills, diagnostics }` return shape (`skills.ts:51-55`).
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsLoadResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Upstream `{ skills, diagnostics }` return shape of `loadSourcedSkills`
/// (`skills.ts:90-94`).
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedSkillsResult<TSkill, TSource> {
    pub skills: Vec<SourcedSkill<TSkill, TSource>>,
    pub diagnostics: Vec<SourcedDiagnostic<SkillDiagnostic, TSource>>,
}

/// Format a skill invocation prompt, optionally appending additional user
/// instructions (`skills.ts:38-42`).
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String {
    let skill_block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        skill.name,
        skill.file_path,
        dirname_env_path(&skill.file_path),
        skill.content
    );
    match additional_instructions.filter(|instructions| !instructions.is_empty()) {
        Some(instructions) => format!("{skill_block}\n\n{instructions}"),
        None => skill_block,
    }
}

/// Load skills from one or more directories (`skills.ts:44-78`).
///
/// Traverses directories recursively, loads `SKILL.md` files, loads direct
/// root `.md` files with skill frontmatter, honors ignore files, and returns
/// diagnostics for invalid declared skill files. Missing input directories
/// are skipped.
pub async fn load_skills<S, I>(env: &dyn FileSystem, dirs: I, context: Context) -> SkillsLoadResult
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();
    for dir in dirs {
        let dir = dir.as_ref();
        let root_info = match env.file_info(dir, context.clone()).await {
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(SkillDiagnostic::warning(
                        SkillDiagnosticCode::FileInfoFailed,
                        error.message,
                        dir,
                    ));
                }
                continue;
            }
            Ok(root_info) => root_info,
        };
        if resolve_kind(env, &root_info, &mut diagnostics, context.clone()).await
            != Some(FileKind::Directory)
        {
            continue;
        }
        let patterns = Mutex::new(Vec::new());
        let result = load_skills_from_dir_internal(
            env,
            &root_info.path,
            true,
            &patterns,
            &root_info.path,
            context.clone(),
        )
        .await;
        skills.extend(result.skills);
        diagnostics.extend(result.diagnostics);
    }
    SkillsLoadResult {
        skills,
        diagnostics,
    }
}

/// Upstream `(skill, source, context) => TSkill` mapper callback
/// (`skills.ts:89`).
pub type SkillMapper<TSource, TSkill> = dyn Fn(Skill, &TSource, Context) -> TSkill + Send + Sync;

/// Load skills from source-tagged directories (`skills.ts:80-108`).
///
/// Source values are preserved exactly and attached to every loaded skill and
/// diagnostic. The agent package does not interpret source values;
/// applications define their own provenance shape. When `map_skill` is
/// `None`, each loaded [`Skill`] converts into `TSkill` via `From`.
pub async fn load_sourced_skills<TSource, TSkill>(
    env: &dyn FileSystem,
    inputs: &[SourcedInput<TSource>],
    map_skill: Option<&SkillMapper<TSource, TSkill>>,
    context: Context,
) -> SourcedSkillsResult<TSkill, TSource>
where
    TSource: Clone + Send + Sync + 'static,
    TSkill: From<Skill>,
{
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();
    for input in inputs {
        let result = load_skills(env, [&input.path], context.clone()).await;
        for skill in result.skills {
            let mapped = match map_skill {
                Some(map_skill) => map_skill(skill, &input.source, context.clone()),
                None => TSkill::from(skill),
            };
            skills.push(SourcedSkill {
                skill: mapped,
                source: input.source.clone(),
            });
        }
        for diagnostic in result.diagnostics {
            diagnostics.push(SourcedDiagnostic {
                diagnostic,
                source: input.source.clone(),
            });
        }
    }
    SourcedSkillsResult {
        skills,
        diagnostics,
    }
}

/// `loadSkillsFromDirInternal` (`skills.ts:110-183`). A `fn` returning a
/// boxed future so the recursion boxes cleanly.
fn load_skills_from_dir_internal<'a>(
    env: &'a dyn FileSystem,
    dir: &'a str,
    include_root_files: bool,
    patterns: &'a Mutex<Vec<String>>,
    root_dir: &'a str,
    context: Context,
) -> BoxFuture<'a, SkillsLoadResult> {
    Box::pin(async move {
        let mut skills = Vec::new();
        let mut diagnostics = Vec::new();

        let dir_info = match env.file_info(dir, context.clone()).await {
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(SkillDiagnostic::warning(
                        SkillDiagnosticCode::FileInfoFailed,
                        error.message,
                        dir,
                    ));
                }
                return SkillsLoadResult {
                    skills,
                    diagnostics,
                };
            }
            Ok(dir_info) => dir_info,
        };
        if resolve_kind(env, &dir_info, &mut diagnostics, context.clone()).await
            != Some(FileKind::Directory)
        {
            return SkillsLoadResult {
                skills,
                diagnostics,
            };
        }

        add_ignore_rules(
            env,
            patterns,
            dir,
            root_dir,
            &mut diagnostics,
            context.clone(),
        )
        .await;

        let entries = match env.list_dir(dir, context.clone()).await {
            Err(error) => {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::ListFailed,
                    error.message,
                    dir,
                ));
                return SkillsLoadResult {
                    skills,
                    diagnostics,
                };
            }
            Ok(entries) => entries,
        };

        // First direct `SKILL.md` file wins: load it and skip everything else
        // in this directory (`skills.ts:145-157`).
        for entry in &entries {
            if entry.name != "SKILL.md" {
                continue;
            }
            let Some(kind) = resolve_kind(env, entry, &mut diagnostics, context.clone()).await
            else {
                continue;
            };
            if kind != FileKind::File {
                continue;
            }
            let rel_path = relative_env_path(root_dir, &entry.path);
            if ignores(patterns, &rel_path, false) {
                continue;
            }
            let result =
                load_skill_from_file(env, &entry.path, &dir_info.name, context.clone()).await;
            if let Some(skill) = result.skill {
                skills.push(skill);
            }
            diagnostics.extend(result.diagnostics);
            return SkillsLoadResult {
                skills,
                diagnostics,
            };
        }

        // Sorted traversal for sibling directories and root markdown files.
        let mut sorted: Vec<&FileInfo> = entries.iter().collect();
        sorted.sort_by_key(|entry| (entry.name.to_lowercase(), entry.name.clone()));
        for entry in sorted {
            if entry.name.starts_with('.') || entry.name == "node_modules" {
                continue;
            }
            let Some(kind) = resolve_kind(env, entry, &mut diagnostics, context.clone()).await
            else {
                continue;
            };

            let rel_path = relative_env_path(root_dir, &entry.path);
            // Upstream marks directories with a trailing slash; the port
            // carries the directory kind as an `is_dir` flag (see module docs).
            if ignores(patterns, &rel_path, kind == FileKind::Directory) {
                continue;
            }

            if kind == FileKind::Directory {
                let result = load_skills_from_dir_internal(
                    env,
                    &entry.path,
                    false,
                    patterns,
                    root_dir,
                    context.clone(),
                )
                .await;
                skills.extend(result.skills);
                diagnostics.extend(result.diagnostics);
                continue;
            }

            if kind != FileKind::File || !include_root_files || !entry.name.ends_with(".md") {
                continue;
            }
            let result =
                load_skill_from_file(env, &entry.path, &dir_info.name, context.clone()).await;
            if let Some(skill) = result.skill {
                skills.push(skill);
            }
            diagnostics.extend(result.diagnostics);
        }

        SkillsLoadResult {
            skills,
            diagnostics,
        }
    })
}

/// `addIgnoreRules` (`skills.ts:185-232`): read this directory's ignore
/// files into the shared pattern list, prefixing each pattern with the
/// directory's path relative to the load root.
async fn add_ignore_rules(
    env: &dyn FileSystem,
    patterns: &Mutex<Vec<String>>,
    dir: &str,
    root_dir: &str,
    diagnostics: &mut Vec<SkillDiagnostic>,
    context: Context,
) {
    let relative_dir = relative_env_path(root_dir, dir);
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{relative_dir}/")
    };

    for filename in IGNORE_FILE_NAMES {
        let ignore_path = match env
            .join_path(&[dir.to_string(), filename.to_string()], context.clone())
            .await
        {
            Err(error) => {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::FileInfoFailed,
                    error.message,
                    dir,
                ));
                continue;
            }
            Ok(ignore_path) => ignore_path,
        };
        let info = match env.file_info(&ignore_path, context.clone()).await {
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(SkillDiagnostic::warning(
                        SkillDiagnosticCode::FileInfoFailed,
                        error.message,
                        &ignore_path,
                    ));
                }
                continue;
            }
            Ok(info) => info,
        };
        if info.kind != FileKind::File {
            continue;
        }
        let content = match env.read_text_file(&ignore_path, context.clone()).await {
            Err(error) => {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::ReadFailed,
                    error.message,
                    &ignore_path,
                ));
                continue;
            }
            Ok(content) => content,
        };
        // `content.split(/\r?\n/)` upstream.
        let new_patterns: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        if !new_patterns.is_empty() {
            patterns
                .lock()
                .expect("ignore patterns lock")
                .extend(new_patterns);
        }
    }
}

/// `prefixIgnorePattern` (`skills.ts:234-250`): skip blank lines and
/// comments; anchor every other pattern to the directory it came from.
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest;
    }
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    let prefixed = if prefix.is_empty() {
        pattern.to_string()
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

/// The upstream `ignoreMatcher.ignores(path)` checks (`skills.ts:151, 167`):
/// build a matcher from every pattern accumulated so far and test one
/// relative path. See the module docs for the trailing-slash substitution.
fn ignores(patterns: &Mutex<Vec<String>>, rel_path: &str, is_dir: bool) -> bool {
    let patterns = patterns.lock().expect("ignore patterns lock");
    if patterns.is_empty() {
        return false;
    }
    let mut builder = GitignoreBuilder::new("");
    for line in patterns.iter() {
        // Malformed globs are skipped; see the module docs.
        let _ = builder.add_line(None, line);
    }
    let Ok(matcher) = builder.build() else {
        return false;
    };
    matches!(
        matcher.matched(std::path::Path::new(rel_path), is_dir),
        Match::Ignore(_)
    )
}

/// `loadSkillFromFile` (`skills.ts:252-308`).
async fn load_skill_from_file(
    env: &dyn FileSystem,
    file_path: &str,
    parent_dir_name: &str,
    context: Context,
) -> SkillFileResult {
    let mut diagnostics = Vec::new();
    let is_declared_skill = file_path
        .trim_end_matches(['/', '\\'])
        .split(['/', '\\'])
        .next_back()
        == Some("SKILL.md");
    let raw_content = match env.read_text_file(file_path, context.clone()).await {
        Err(error) => {
            diagnostics.push(SkillDiagnostic::warning(
                SkillDiagnosticCode::ReadFailed,
                error.message,
                file_path,
            ));
            return SkillFileResult {
                skill: None,
                diagnostics,
            };
        }
        Ok(raw_content) => raw_content,
    };

    let (frontmatter, body) = match parse_frontmatter(&raw_content) {
        Err(error) => {
            if is_declared_skill {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::ParseFailed,
                    error,
                    file_path,
                ));
            }
            return SkillFileResult {
                skill: None,
                diagnostics,
            };
        }
        Ok(parsed) => parsed,
    };

    let description = frontmatter_string(&frontmatter, "description");
    let description_blank = description.as_deref().is_none_or(|d| d.trim().is_empty());
    // Non-declared files without a description are silently skipped.
    if !is_declared_skill && description_blank {
        return SkillFileResult {
            skill: None,
            diagnostics,
        };
    }

    for error in validate_description(description.as_deref()) {
        diagnostics.push(SkillDiagnostic::warning(
            SkillDiagnosticCode::InvalidMetadata,
            error,
            file_path,
        ));
    }

    // `frontmatterName || parentDirName`: empty strings fall through, too.
    let frontmatter_name = frontmatter_string(&frontmatter, "name").filter(|name| !name.is_empty());
    let name = frontmatter_name.unwrap_or_else(|| parent_dir_name.to_string());
    for error in validate_name(&name, parent_dir_name) {
        diagnostics.push(SkillDiagnostic::warning(
            SkillDiagnosticCode::InvalidMetadata,
            error,
            file_path,
        ));
    }

    if description_blank {
        return SkillFileResult {
            skill: None,
            diagnostics,
        };
    }

    let disable_model_invocation = frontmatter_bool(&frontmatter, "disable-model-invocation");
    SkillFileResult {
        skill: Some(Skill {
            name,
            description: description.expect("description checked non-blank above"),
            content: body,
            file_path: file_path.to_string(),
            disable_model_invocation: Some(disable_model_invocation),
        }),
        diagnostics,
    }
}

/// Upstream `{ skill, diagnostics }` return shape of `loadSkillFromFile`
/// (`skills.ts:252`).
struct SkillFileResult {
    skill: Option<Skill>,
    diagnostics: Vec<SkillDiagnostic>,
}

/// `validateName` (`skills.ts:310-320`).
fn validate_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            "name \"{name}\" does not match parent directory \"{parent_dir_name}\""
        ));
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            name.chars().count()
        ));
    }
    let valid_characters = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_characters {
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

/// `validateDescription` (`skills.ts:322-330`).
fn validate_description(description: Option<&str>) -> Vec<String> {
    match description {
        Some(description) if !description.trim().is_empty() => {
            if description.chars().count() > MAX_DESCRIPTION_LENGTH {
                vec![format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
                    description.chars().count()
                )]
            } else {
                Vec::new()
            }
        }
        _ => vec!["description is required".to_string()],
    }
}

/// `parseFrontmatter` (`skills.ts:332-346`): split a leading `---`-delimited
/// YAML document from the body. Returns the raw YAML document (a mapping in
/// practice) and the trimmed body, or an error message when the YAML does not
/// parse.
fn parse_frontmatter(content: &str) -> Result<(Yaml, String), String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok((Yaml::Null, normalized));
    }
    let Some(end_index) = normalized.find("\n---") else {
        return Ok((Yaml::Null, normalized));
    };
    let yaml_string = if end_index >= 4 {
        &normalized[4..end_index]
    } else {
        ""
    };
    let body = normalized[end_index + 4..].trim().to_string();
    let mut documents =
        YamlLoader::load_from_str(yaml_string).map_err(|error| error.to_string())?;
    // `parse(yamlString) ?? {}`: absent documents behave like null.
    let frontmatter = documents.drain(..).next().unwrap_or(Yaml::Null);
    Ok((frontmatter, body))
}

/// Read a string field from a frontmatter mapping (upstream typed
/// `frontmatter` property reads: non-strings and missing keys are `undefined`).
fn frontmatter_string(frontmatter: &Yaml, key: &str) -> Option<String> {
    match frontmatter {
        Yaml::Hash(hash) => match hash.get(&Yaml::String(key.to_string())) {
            Some(Yaml::String(value)) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Strict `frontmatter[key] === true` read.
fn frontmatter_bool(frontmatter: &Yaml, key: &str) -> bool {
    matches!(
        frontmatter,
        Yaml::Hash(hash) if hash.get(&Yaml::String(key.to_string())) == Some(&Yaml::Boolean(true))
    )
}

/// `resolveKind` (`skills.ts:348-380`): direct kinds pass through; symlinks
/// are resolved through the canonical path, and only file/directory targets
/// count.
async fn resolve_kind(
    env: &dyn FileSystem,
    info: &FileInfo,
    diagnostics: &mut Vec<SkillDiagnostic>,
    context: Context,
) -> Option<FileKind> {
    if info.kind != FileKind::Symlink {
        return Some(info.kind);
    }
    let canonical_path = match env.canonical_path(&info.path, context.clone()).await {
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::FileInfoFailed,
                    error.message,
                    &info.path,
                ));
            }
            return None;
        }
        Ok(canonical_path) => canonical_path,
    };
    let target = match env.file_info(&canonical_path, context.clone()).await {
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(SkillDiagnostic::warning(
                    SkillDiagnosticCode::FileInfoFailed,
                    error.message,
                    &info.path,
                ));
            }
            return None;
        }
        Ok(target) => target,
    };
    (target.kind != FileKind::Symlink).then_some(target.kind)
}

/// `dirnameEnvPath` (`skills.ts:382-387`).
fn dirname_env_path(path: &str) -> String {
    let normalized = path.trim_end_matches(['/', '\\']);
    // Byte indices are safe: the separators and drive colon are ASCII.
    match normalized.rfind(['/', '\\']) {
        Some(separator_index) if separator_index == 2 && normalized.as_bytes()[1] == b':' => {
            normalized[..3].to_string()
        }
        Some(separator_index) if separator_index > 0 => normalized[..separator_index].to_string(),
        _ => "/".to_string(),
    }
}

/// `relativeEnvPath` (`skills.ts:389-396`): backslashes normalize to
/// forward slashes, trailing slashes drop, and paths outside the root lose
/// their leading slashes.
fn relative_env_path(root: &str, path: &str) -> String {
    let normalized_root = root.replace('\\', "/");
    let normalized_root = normalized_root.trim_end_matches('/');
    let normalized_path = path.replace('\\', "/");
    let normalized_path = normalized_path.trim_end_matches('/');
    if normalized_path == normalized_root {
        return String::new();
    }
    let prefix = format!("{normalized_root}/");
    if let Some(stripped) = normalized_path.strip_prefix(&prefix) {
        stripped.to_string()
    } else {
        normalized_path.trim_start_matches('/').to_string()
    }
}

#[cfg(test)]
mod tests;
