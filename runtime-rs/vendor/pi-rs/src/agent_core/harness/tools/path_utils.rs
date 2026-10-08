//! Upstream tools/path-utils.ts: addressed paths and ordered read fallbacks.
use super::super::{Context, ExecutionEnv};
use icu_normalizer::DecomposingNormalizerBorrowed;
use std::collections::HashSet;
use std::sync::LazyLock;

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
        .unwrap_or(&normalized)
        .to_string()
}

pub async fn resolve_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    context: Context,
) -> anyhow::Result<String> {
    Ok(env
        .absolute_path(&normalize_tool_path(path), context)
        .await?)
}

pub async fn resolve_read_tool_path(
    env: &dyn ExecutionEnv,
    path: &str,
    context: Context,
) -> anyhow::Result<String> {
    static PERIOD: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i) (AM|PM)\.").expect("static regex"));
    let resolved = resolve_tool_path(env, path, context.clone()).await?;
    let nfd = DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&resolved)
        .into_owned();
    let variants = [
        resolved.clone(),
        PERIOD.replace_all(&resolved, "\u{202f}$1.").into_owned(),
        nfd.clone(),
        resolved.replace('\'', "\u{2019}"),
        nfd.replace('\'', "\u{2019}"),
    ];
    let mut seen = HashSet::new();
    for variant in variants {
        if seen.insert(variant.clone()) && env.exists(&variant, context.clone()).await? {
            return Ok(variant);
        }
    }
    Ok(resolved)
}
