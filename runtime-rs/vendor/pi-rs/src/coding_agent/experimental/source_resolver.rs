//! Port of upstream `experimental/source-resolver.ts`
//! (sha256 c584d52fb68860c0c6ae5dfbbcb6052fa104d6e34c7ef231b9e6eb7e0ec5b2a6).
//!
//! Ported: the tsconfig `paths` alias model (parsing, wildcard validation,
//! longest-pattern-first ordering), `matchAlias` and `resolveSourcePath`
//! candidate resolution (`@earendil-works/*` filtered, repo-root contained,
//! `.js/.mjs/.cjs` -> `.ts/.mts/.cts` rewriting, `index.ts` directory
//! fallback), and the exact upstream error strings.
//!
//! D2 seam (see mod.rs docs): the Node `module.registerHooks` ESM resolver
//! integration is Node-runtime-only and not portable; the resolution model it
//! drives is fully ported and oracle-tested.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Upstream `SourceAlias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAlias {
    pub pattern: String,
    pub prefix: String,
    pub suffix: String,
    pub replacements: Vec<String>,
}

#[derive(Deserialize)]
struct TsConfig {
    #[serde(rename = "compilerOptions")]
    compiler_options: Option<CompilerOptions>,
}

#[derive(Deserialize)]
struct CompilerOptions {
    paths: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Parse upstream's module-level `aliases` list from a tsconfig document.
/// Only `@earendil-works/` patterns are kept; patterns with more than one
/// wildcard are rejected with the exact upstream error; the list is sorted by
/// descending pattern length.
pub fn build_aliases(
    repository_root: &Path,
    tsconfig_path: &Path,
    tsconfig_text: &str,
) -> Result<Vec<SourceAlias>, String> {
    let tsconfig: TsConfig = serde_json::from_str(tsconfig_text)
        .map_err(|error| format!("Failed to parse {}: {error}", tsconfig_path.display()))?;
    let paths = tsconfig
        .compiler_options
        .and_then(|options| options.paths)
        .ok_or_else(|| {
            format!(
                "Source runtime requires compilerOptions.paths in {}",
                tsconfig_path.display()
            )
        })?;
    let mut aliases: Vec<SourceAlias> = Vec::new();
    for (pattern, replacements) in &paths {
        if !pattern.starts_with("@earendil-works/") {
            continue;
        }
        let replacements: Vec<String> = match replacements {
            serde_json::Value::Array(values) => values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            _ => continue,
        };
        let wildcard = pattern.find('*');
        if let Some(first) = wildcard {
            if pattern[first + 1..].contains('*') {
                return Err(format!(
                    "Source runtime does not support multiple wildcards in {pattern}"
                ));
            }
        }
        let (prefix, suffix) = match wildcard {
            Some(first) => (
                pattern[..first].to_string(),
                pattern[first + 1..].to_string(),
            ),
            None => (pattern.clone(), String::new()),
        };
        aliases.push(SourceAlias {
            pattern: pattern.clone(),
            prefix,
            suffix,
            replacements: replacements.clone(),
        });
    }
    aliases.sort_by_key(|left| std::cmp::Reverse(left.pattern.len()));
    let _ = repository_root;
    Ok(aliases)
}

/// Upstream `matchAlias`: returns the wildcard capture (possibly empty for
/// non-wildcard patterns), or `None` when the specifier does not match.
pub fn match_alias(alias: &SourceAlias, specifier: &str) -> Option<String> {
    if !alias.pattern.contains('*') {
        return if specifier == alias.pattern {
            Some(String::new())
        } else {
            None
        };
    }
    if !specifier.starts_with(&alias.prefix) || !specifier.ends_with(&alias.suffix) {
        return None;
    }
    Some(specifier[alias.prefix.len()..specifier.len() - alias.suffix.len()].to_string())
}

/// Upstream `resolveSourcePath`: resolve the (already wildcard-substituted)
/// replacement against the repository root, then pick the first existing
/// candidate file. Mirrors the extension rewriting exactly.
pub fn resolve_source_path(repository_root: &Path, replacement: &str) -> Option<PathBuf> {
    let base_path = repository_root.join(replacement);
    let base_path = normalize(&base_path);
    let root = normalize(repository_root);
    let root_prefix = if root.to_string_lossy().ends_with(std::path::MAIN_SEPARATOR) {
        root.clone()
    } else {
        root.join(std::path::MAIN_SEPARATOR.to_string())
    };
    if !base_path.starts_with(&root_prefix) {
        return None;
    }
    let extension = base_path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()));
    let extension = extension.as_deref();
    let candidates: Vec<PathBuf> = match extension {
        Some(".js") => vec![base_path.clone(), with_extension(&base_path, ".ts")],
        Some(".mjs") => vec![base_path.clone(), with_extension(&base_path, ".mts")],
        Some(".cjs") => vec![base_path.clone(), with_extension(&base_path, ".cts")],
        Some(".ts") | Some(".mts") | Some(".cts") | Some(".json") => vec![base_path.clone()],
        _ => vec![
            base_path.clone(),
            join_extension(&base_path, ".ts"),
            base_path.join("index.ts"),
        ],
    };
    for candidate in candidates {
        if let Ok(metadata) = fs::metadata(&candidate) {
            if metadata.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Upstream's inline `resolve` hook body for matched aliases: try each
/// replacement with the wildcard substituted; the first resolvable candidate
/// wins. Returns `Err` carrying the exact upstream throw text when a pattern
/// matched but no replacement resolved.
pub fn resolve_through_aliases(
    aliases: &[SourceAlias],
    repository_root: &Path,
    specifier: &str,
) -> Result<Option<PathBuf>, String> {
    let mut matched_pattern: Option<&str> = None;
    for alias in aliases {
        let Some(wildcard) = match_alias(alias, specifier) else {
            continue;
        };
        if matched_pattern.is_none() {
            matched_pattern = Some(&alias.pattern);
        }
        for replacement in &alias.replacements {
            let substituted = replacement.replacen('*', &wildcard, 1);
            if let Some(resolved) = resolve_source_path(repository_root, &substituted) {
                return Ok(Some(resolved));
            }
        }
    }
    match matched_pattern {
        Some(pattern) => Err(format!(
            "Source runtime could not resolve {specifier} through tsconfig path {pattern}"
        )),
        None => Ok(None),
    }
}

fn with_extension(path: &Path, source_extension: &str) -> PathBuf {
    path.with_extension(source_extension.trim_start_matches('.'))
}

fn join_extension(path: &Path, suffix: &str) -> PathBuf {
    let mut os: std::ffi::OsString = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests;
