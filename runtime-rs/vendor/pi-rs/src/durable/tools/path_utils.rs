//! Port of `src/tools/path-utils.ts`: model-path normalization and the
//! ordered read fallbacks for near-miss filenames.

use std::collections::HashSet;
use std::sync::LazyLock;

use icu_normalizer::DecomposingNormalizerBorrowed;
use regex::Regex;

use crate::agent_core::chord_support::context::Context;
use crate::durable::env::ExecutionEnv;
use crate::durable::errors::PlainError;

const NARROW_NO_BREAK_SPACE: char = '\u{202f}';

/// `normalizeToolPath(path)` (`tools/path-utils.ts`): the UNICODE_SPACES
/// replacement (`[\u00A0\u2000-\u200A\u202F\u205F\u3000]`) and a leading `@`.
pub fn normalize_tool_path(path: &str) -> String {
    let normalized: String = path
        .chars()
        .map(|c| match c {
            '\u{00a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            _ => c,
        })
        .collect();
    normalized
        .strip_prefix('@')
        .map(str::to_string)
        .unwrap_or(normalized)
}

/// `resolveToolPath(env, path, context)` (`tools/path-utils.ts`).
pub fn resolve_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    context: &Context,
) -> Result<String, PlainError> {
    env.absolute_path(&normalize_tool_path(path), context)
        .map_err(|error| PlainError::new(error.message))
}

/// `resolveReadToolPath(env, path, context)` (`tools/path-utils.ts`): the
/// first existing variant, else the plain resolution.
pub fn resolve_read_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    context: &Context,
) -> Result<String, PlainError> {
    static PERIOD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i) (AM|PM)\.").expect("static regex"));
    let resolved = resolve_tool_path(env, path, context)?;
    let nfd = DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&resolved)
        .into_owned();
    let variants = [
        resolved.clone(),
        PERIOD
            .replace_all(&resolved, format!("{NARROW_NO_BREAK_SPACE}$1."))
            .into_owned(),
        nfd.clone(),
        resolved.replace('\'', "\u{2019}"),
        nfd.replace('\'', "\u{2019}"),
    ];

    // The upstream `Set` deduplicates while keeping first-seen order; the
    // `variants` array still drives the iteration order, and `exists` runs
    // once per unique variant.
    let mut seen = HashSet::new();
    for variant in variants {
        if seen.insert(variant.clone()) {
            let exists = env
                .exists(&variant, context)
                .map_err(|error| PlainError::new(error.message))?;
            if exists {
                return Ok(variant);
            }
        }
    }
    Ok(resolved)
}
