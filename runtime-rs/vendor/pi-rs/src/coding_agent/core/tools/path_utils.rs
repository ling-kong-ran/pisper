//! Upstream core/tools/path-utils.ts, including macOS filename fallbacks.
use crate::coding_agent::utils::paths::{self, PathInputOptions};
use icu_normalizer::DecomposingNormalizerBorrowed;
use std::sync::LazyLock;
fn options() -> PathInputOptions {
    PathInputOptions {
        normalize_unicode_spaces: true,
        strip_at_prefix: true,
        ..Default::default()
    }
}
pub fn expand_path(path: &str) -> Result<String, String> {
    paths::normalize_path_with_options(path, &options()).map_err(|e| e.to_string())
}
pub fn resolve_to_cwd(path: &str, cwd: &str) -> Result<String, String> {
    paths::resolve_path_with(path, cwd, &options(), cfg!(windows)).map_err(|e| e.to_string())
}
pub async fn path_exists(path: &str) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}
fn variants(path: &str) -> Vec<String> {
    static PERIOD: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i) (AM|PM)\.").expect("static regex"));
    let nfd = DecomposingNormalizerBorrowed::new_nfd()
        .normalize(path)
        .into_owned();
    vec![
        path.into(),
        PERIOD.replace_all(path, "\u{202f}$1.").into_owned(),
        nfd.clone(),
        path.replace('\'', "\u{2019}"),
        nfd.replace('\'', "\u{2019}"),
    ]
}
pub fn resolve_read_path(path: &str, cwd: &str) -> Result<String, String> {
    let resolved = resolve_to_cwd(path, cwd)?;
    Ok(variants(&resolved)
        .into_iter()
        .find(|p| std::fs::metadata(p).is_ok())
        .unwrap_or(resolved))
}
pub async fn resolve_read_path_async(path: &str, cwd: &str) -> Result<String, String> {
    let resolved = resolve_to_cwd(path, cwd)?;
    for variant in variants(&resolved) {
        if path_exists(&variant).await {
            return Ok(variant);
        }
    }
    Ok(resolved)
}
