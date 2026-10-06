//! Port of `packages/agent/src/harness/prompt-templates.ts` (270 lines):
//! markdown prompt-template discovery with YAML frontmatter, argument
//! parsing/substitution, and template invocation formatting. Oracle:
//! `packages/agent/test/harness/prompt-templates.test.ts`.
//!
//! Disclosed substitutions (shared with the skills module, where the same
//! upstream helpers are also duplicated):
//! - **Frontmatter YAML.** The npm `yaml` package becomes `yaml-rust2`
//!   (pinned exact); parse errors keep the `parse_failed` code but the
//!   message text comes from the Rust scanner. An empty or non-mapping
//!   document yields no fields, like upstream's `parse(...) ?? {}`.
//! - **`localeCompare`.** Approximated as case-insensitive ordering with a
//!   case-sensitive tiebreak.
//! - **`substituteArgs` regexes.** The four upstream global replaces run as
//!   four sequential single-pass `regex` crate replaces in the same order, so
//!   text inserted by an earlier pass is visible to later passes exactly as
//!   upstream. Passes 1-2 (`$N`, `${@:N:L}`) use upstream function replacers,
//!   so their replacement text is inserted verbatim. Passes 3-4
//!   (`$ARGUMENTS`, `$@`) pass the joined arguments as a JS *string*
//!   replacement, which expands the replacement patterns `$$` -> `$`,
//!   `$&` -> the matched placeholder text, `` $` `` -> the text before the
//!   match, and `$'` -> the text after the match ([`expand_js_replacement`]
//!   reproduces this; `$n`/`$<name>` are inert without capture groups, as
//!   upstream).
//! - **First-line description slice.** Upstream `slice(0, 60)` counts UTF-16
//!   units; the port takes 60 `char`s (identical for ASCII).

use std::sync::LazyLock;

use regex::{Captures, Regex};
use yaml_rust2::{Yaml, YamlLoader};

use crate::agent_core::harness::skills::SourcedDiagnostic;
use crate::agent_core::harness::types::{
    FileErrorCode, FileInfo, FileKind, FileSystem, PromptTemplate, SourcedInput,
};
use crate::agent_core::harness::Context;

/// Upstream `PromptTemplateDiagnosticCode` (`prompt-templates.ts:5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTemplateDiagnosticCode {
    FileInfoFailed,
    ListFailed,
    ReadFailed,
    ParseFailed,
}

/// Warning produced while loading prompt templates (upstream
/// `PromptTemplateDiagnostic`, `prompt-templates.ts:7-17`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplateDiagnostic {
    /// Diagnostic severity; only warnings are emitted (`type` is a reserved
    /// word in Rust, hence the raw identifier).
    pub r#type: String,
    /// Stable diagnostic code.
    pub code: PromptTemplateDiagnosticCode,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Path associated with the diagnostic.
    pub path: String,
}

impl PromptTemplateDiagnostic {
    fn warning(
        code: PromptTemplateDiagnosticCode,
        message: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        PromptTemplateDiagnostic {
            r#type: "warning".to_string(),
            code,
            message: message.into(),
            path: path.into(),
        }
    }
}

/// Upstream `{ promptTemplate, source }` record returned by
/// [`load_sourced_prompt_templates`] (`prompt-templates.ts:80-84`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourcedPromptTemplate<TPromptTemplate = PromptTemplate, TSource = ()> {
    pub prompt_template: TPromptTemplate,
    pub source: TSource,
}

/// Upstream `{ promptTemplates, diagnostics }` return shape
/// (`prompt-templates.ts:31-37`).
#[derive(Debug, Clone, PartialEq)]
pub struct PromptTemplatesLoadResult {
    pub prompt_templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// Upstream `{ promptTemplates, diagnostics }` return shape of
/// `loadSourcedPromptTemplates` (`prompt-templates.ts:80-84`).
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedPromptTemplatesResult<TPromptTemplate, TSource> {
    pub prompt_templates: Vec<SourcedPromptTemplate<TPromptTemplate, TSource>>,
    pub diagnostics: Vec<SourcedDiagnostic<PromptTemplateDiagnostic, TSource>>,
}

/// Load prompt templates from one or more paths (`prompt-templates.ts:25-64`).
///
/// Directory inputs load direct `.md` children non-recursively. File inputs
/// load explicit `.md` files. Missing paths and non-markdown files are
/// skipped. Read and parse failures are returned as diagnostics.
pub async fn load_prompt_templates<S, I>(
    env: &dyn FileSystem,
    paths: I,
    context: Context,
) -> PromptTemplatesLoadResult
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let mut prompt_templates = Vec::new();
    let mut diagnostics = Vec::new();
    for path in paths {
        let path = path.as_ref();
        let info = match env.file_info(path, context.clone()).await {
            Err(error) => {
                if error.code != FileErrorCode::NotFound {
                    diagnostics.push(PromptTemplateDiagnostic::warning(
                        PromptTemplateDiagnosticCode::FileInfoFailed,
                        error.message,
                        path,
                    ));
                }
                continue;
            }
            Ok(info) => info,
        };
        match resolve_kind(env, &info, &mut diagnostics, context.clone()).await {
            Some(FileKind::Directory) => {
                let result = load_templates_from_dir(env, &info.path, context.clone()).await;
                prompt_templates.extend(result.prompt_templates);
                diagnostics.extend(result.diagnostics);
            }
            Some(FileKind::File) if info.name.ends_with(".md") => {
                let result =
                    load_template_from_file(env, &info.path, &info.name, context.clone()).await;
                if let Some(prompt_template) = result.prompt_template {
                    prompt_templates.push(prompt_template);
                }
                diagnostics.extend(result.diagnostics);
            }
            _ => {}
        }
    }
    PromptTemplatesLoadResult {
        prompt_templates,
        diagnostics,
    }
}

/// Upstream `(promptTemplate, source, context) => TPromptTemplate` mapper
/// callback (`prompt-templates.ts:75-77`).
pub type PromptTemplateMapper<TSource, TPromptTemplate> =
    dyn Fn(PromptTemplate, &TSource, Context) -> TPromptTemplate + Send + Sync;

/// Load prompt templates from source-tagged paths
/// (`prompt-templates.ts:66-98`).
///
/// Source values are preserved exactly and attached to every loaded prompt
/// template and diagnostic. When `map_prompt_template` is `None`, each loaded
/// [`PromptTemplate`] converts into `TPromptTemplate` via `From`.
pub async fn load_sourced_prompt_templates<TSource, TPromptTemplate>(
    env: &dyn FileSystem,
    inputs: &[SourcedInput<TSource>],
    map_prompt_template: Option<&PromptTemplateMapper<TSource, TPromptTemplate>>,
    context: Context,
) -> SourcedPromptTemplatesResult<TPromptTemplate, TSource>
where
    TSource: Clone + Send + Sync + 'static,
    TPromptTemplate: From<PromptTemplate>,
{
    let mut prompt_templates = Vec::new();
    let mut diagnostics = Vec::new();
    for input in inputs {
        let result = load_prompt_templates(env, [&input.path], context.clone()).await;
        for prompt_template in result.prompt_templates {
            let mapped = match map_prompt_template {
                Some(map) => map(prompt_template, &input.source, context.clone()),
                None => TPromptTemplate::from(prompt_template),
            };
            prompt_templates.push(SourcedPromptTemplate {
                prompt_template: mapped,
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
    SourcedPromptTemplatesResult {
        prompt_templates,
        diagnostics,
    }
}

/// `loadTemplatesFromDir` (`prompt-templates.ts:100-127`).
async fn load_templates_from_dir(
    env: &dyn FileSystem,
    dir: &str,
    context: Context,
) -> PromptTemplatesLoadResult {
    let mut prompt_templates = Vec::new();
    let mut diagnostics = Vec::new();
    let entries = match env.list_dir(dir, context.clone()).await {
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic::warning(
                PromptTemplateDiagnosticCode::ListFailed,
                error.message,
                dir,
            ));
            return PromptTemplatesLoadResult {
                prompt_templates,
                diagnostics,
            };
        }
        Ok(entries) => entries,
    };

    let mut sorted: Vec<&FileInfo> = entries.iter().collect();
    sorted.sort_by_key(|entry| (entry.name.to_lowercase(), entry.name.clone()));
    for entry in sorted {
        let Some(FileKind::File) =
            resolve_kind(env, entry, &mut diagnostics, context.clone()).await
        else {
            continue;
        };
        if !entry.name.ends_with(".md") {
            continue;
        }
        let result = load_template_from_file(env, &entry.path, &entry.name, context.clone()).await;
        if let Some(prompt_template) = result.prompt_template {
            prompt_templates.push(prompt_template);
        }
        diagnostics.extend(result.diagnostics);
    }
    PromptTemplatesLoadResult {
        prompt_templates,
        diagnostics,
    }
}

/// Upstream `{ promptTemplate, diagnostics }` return shape of
/// `loadTemplateFromFile` (`prompt-templates.ts:129`).
struct TemplateFileResult {
    prompt_template: Option<PromptTemplate>,
    diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// `loadTemplateFromFile` (`prompt-templates.ts:129-173`).
async fn load_template_from_file(
    env: &dyn FileSystem,
    file_path: &str,
    file_name: &str,
    context: Context,
) -> TemplateFileResult {
    let mut diagnostics = Vec::new();
    let raw_content = match env.read_text_file(file_path, context.clone()).await {
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic::warning(
                PromptTemplateDiagnosticCode::ReadFailed,
                error.message,
                file_path,
            ));
            return TemplateFileResult {
                prompt_template: None,
                diagnostics,
            };
        }
        Ok(raw_content) => raw_content,
    };

    let (frontmatter, body) = match parse_frontmatter(&raw_content) {
        Err(error) => {
            diagnostics.push(PromptTemplateDiagnostic::warning(
                PromptTemplateDiagnosticCode::ParseFailed,
                error,
                file_path,
            ));
            return TemplateFileResult {
                prompt_template: None,
                diagnostics,
            };
        }
        Ok(parsed) => parsed,
    };

    // Fall back to the first non-blank body line, truncated to 60 chars.
    let first_line = body.split('\n').find(|line| !line.trim().is_empty());
    let mut description = frontmatter_string(&frontmatter, "description").unwrap_or_default();
    if description.is_empty() {
        if let Some(first_line) = first_line {
            description = first_line.chars().take(60).collect();
            if first_line.chars().count() > 60 {
                description.push_str("...");
            }
        }
    }
    let name = strip_markdown_extension(file_name);
    TemplateFileResult {
        prompt_template: Some(PromptTemplate {
            name: name.to_string(),
            description: Some(description),
            content: body,
        }),
        diagnostics,
    }
}

/// `fileName.replace(/\.md$/i, "")`: strip a case-insensitive `.md` suffix.
fn strip_markdown_extension(file_name: &str) -> &str {
    let stem_len = file_name.len().saturating_sub(3);
    if file_name.is_char_boundary(stem_len) && file_name[stem_len..].eq_ignore_ascii_case(".md") {
        &file_name[..stem_len]
    } else {
        file_name
    }
}

/// `resolveKind` (`prompt-templates.ts:175-207`): direct kinds pass through;
/// symlinks are resolved through the canonical path, and only file/directory
/// targets count.
async fn resolve_kind(
    env: &dyn FileSystem,
    info: &FileInfo,
    diagnostics: &mut Vec<PromptTemplateDiagnostic>,
    context: Context,
) -> Option<FileKind> {
    if info.kind != FileKind::Symlink {
        return Some(info.kind);
    }
    let canonical_path = match env.canonical_path(&info.path, context.clone()).await {
        Err(error) => {
            if error.code != FileErrorCode::NotFound {
                diagnostics.push(PromptTemplateDiagnostic::warning(
                    PromptTemplateDiagnosticCode::FileInfoFailed,
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
                diagnostics.push(PromptTemplateDiagnostic::warning(
                    PromptTemplateDiagnosticCode::FileInfoFailed,
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

/// `parseFrontmatter` (`prompt-templates.ts:209-223`): split a leading
/// `---`-delimited YAML document from the body. Returns the raw YAML
/// document (a mapping in practice) and the trimmed body, or an error
/// message when the YAML does not parse.
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

/// Upstream `/\$(\d+)/g` (`prompt-templates.ts:254`).
static POSITIONAL_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$(\d+)").expect("valid regex"));
/// Upstream `/\$\{@:(\d+)(?::(\d+))?\}/g` (`prompt-templates.ts:255`).
static SLICE_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").expect("valid regex"));
/// Upstream `/\$ARGUMENTS/g` (`prompt-templates.ts:262`).
static ALL_ARGUMENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$ARGUMENTS").expect("valid regex"));
/// Upstream `/\$@/g` (`prompt-templates.ts:263`).
static AT_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$@").expect("valid regex"));

/// Parse an argument string using simple shell-style single and double
/// quotes (`prompt-templates.ts:226-249`).
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for character in args_string.chars() {
        if let Some(quote) = in_quote {
            if character == quote {
                in_quote = None;
            } else {
                current.push(character);
            }
        } else if character == '"' || character == '\'' {
            in_quote = Some(character);
        } else if character == ' ' || character == '\t' {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute prompt template placeholders (`$1`, `$@`, `$ARGUMENTS`,
/// `${@:N}`, `${@:N:L}`) with command arguments (`prompt-templates.ts:252-265`).
pub fn substitute_args(content: &str, args: &[String]) -> String {
    // The four upstream replaces run in the same order on the accumulated
    // result, so text inserted by one pass is visible to the next.
    let result = POSITIONAL_ARGUMENT.replace_all(content, |captures: &Captures| {
        let index = captures[1]
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| args.get(index));
        index.cloned().unwrap_or_default()
    });
    let result = SLICE_ARGUMENT.replace_all(&result, |captures: &Captures| {
        let start = captures[1]
            .parse::<usize>()
            .unwrap_or(usize::MAX)
            .saturating_sub(1);
        let length = captures
            .get(2)
            .map(|length| length.as_str().parse::<usize>().unwrap_or(usize::MAX));
        let tail = args.get(start..).unwrap_or(&[]);
        match length {
            Some(length) => tail.get(..length).unwrap_or(tail).join(" "),
            None => tail.join(" "),
        }
    });
    let all_args = args.join(" ");
    // Passes 3/4 pass `allArgs` as a JS *string* replacement, which expands
    // the replacement patterns inside it; see `expand_js_replacement`.
    let result = ALL_ARGUMENTS.replace_all(&result, |captures: &Captures| {
        let matched = captures.get(0).expect("regex group 0");
        expand_js_replacement(
            &result,
            matched.start(),
            matched.end(),
            matched.as_str(),
            &all_args,
        )
    });
    let result = AT_ARGUMENT.replace_all(&result, |captures: &Captures| {
        let matched = captures.get(0).expect("regex group 0");
        expand_js_replacement(
            &result,
            matched.start(),
            matched.end(),
            matched.as_str(),
            &all_args,
        )
    });
    result.into_owned()
}

/// Expand the JS `String.prototype.replace` replacement patterns for one
/// match of `matched` at `match_start..match_end` in `haystack` (the string
/// the replace ran on): `$$` -> `$`, `$&` -> the matched text, `` $` `` ->
/// the text before the match, `$'` -> the text after the match. Any other
/// `$` sequence (including `$n`/`$<name>`, which are inert because these two
/// upstream regexes have no capture groups, and a lone trailing `$`) is kept
/// literally, as upstream.
fn expand_js_replacement(
    haystack: &str,
    match_start: usize,
    match_end: usize,
    matched: &str,
    replacement: &str,
) -> String {
    let mut expanded = String::with_capacity(replacement.len());
    let bytes = replacement.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() {
            match bytes[index + 1] {
                b'$' => {
                    expanded.push('$');
                    index += 2;
                    continue;
                }
                b'&' => {
                    expanded.push_str(matched);
                    index += 2;
                    continue;
                }
                b'`' => {
                    expanded.push_str(&haystack[..match_start]);
                    index += 2;
                    continue;
                }
                b'\'' => {
                    expanded.push_str(&haystack[match_end..]);
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        // Copy one full character (multibyte-safe).
        let character = replacement[index..]
            .chars()
            .next()
            .expect("non-empty remainder");
        expanded.push(character);
        index += character.len_utf8();
    }
    expanded
}

/// Format a prompt template invocation with positional arguments
/// (`prompt-templates.ts:267-270`).
pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

#[cfg(test)]
mod tests;
