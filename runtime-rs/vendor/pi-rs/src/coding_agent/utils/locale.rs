//! Locale-aware filename collation for the native ls tool. Sorting uses ICU4X
//! rather than Unicode scalar/byte order, after JavaScript's locale-insensitive
//! lowercasing. This preserves stable ties (case and canonical equivalents).
//! Windows uses the user's regional LocaleName; POSIX uses ICU's environment
//! precedence. A host embedding pi and overriding ICU's process-global locale
//! should explicitly supply that locale. ICU4X's CLDR 48.2.1 data is not claimed
//! to be identical to every possible upstream Node/ICU data release.
use icu_collator::{options::CollatorOptions, Collator, CollatorBorrowed};
use icu_locale_core::Locale;
use std::{cmp::Ordering, sync::LazyLock};

pub struct LocaleComparator(CollatorBorrowed<'static>);
impl LocaleComparator {
    pub fn new(locale: &str) -> Result<Self, String> {
        let locale = locale.parse::<Locale>().map_err(|e| e.to_string())?;
        Collator::try_new(locale.into(), CollatorOptions::default())
            .map(Self)
            .map_err(|e| e.to_string())
    }
    pub fn compare(&self, left: &str, right: &str) -> Ordering {
        self.0.compare(left, right)
    }
    pub fn sort_case_insensitive(&self, entries: Vec<String>) -> Vec<String> {
        let mut entries: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                let lower = entry.to_lowercase();
                (entry, lower)
            })
            .collect();
        entries.sort_by(|a, b| self.compare(&a.1, &b.1));
        entries.into_iter().map(|(entry, _)| entry).collect()
    }
}

/// ICU initializes and caches its default locale once; do the same here.
pub fn default_sort_locale() -> &'static str {
    static LOCALE: LazyLock<String> = LazyLock::new(|| {
        #[cfg(windows)]
        let candidate = windows_registry::CURRENT_USER
            .open(r"Control Panel\International")
            .and_then(|key| key.get_string("LocaleName"))
            .ok();
        #[cfg(not(windows))]
        let candidate = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok());
        normalize_system_locale(candidate.as_deref().unwrap_or("en-US"))
    });
    &LOCALE
}

/// Convert conventional ICU/POSIX names to BCP47. The V8 default for C/POSIX
/// is en-US, not the POSIX tailored collation. Do not consult LANGUAGE or
/// LC_COLLATE: upstream's process-wide Intl locale does not use those.
pub fn normalize_system_locale(locale: &str) -> String {
    let base = locale.split(['.', '@']).next().unwrap_or_default();
    if matches!(base, "" | "C" | "POSIX" | "en_US_POSIX") {
        return "en-US".into();
    }
    let mut normalized = base.replace('_', "-");
    if let Some((_, modifier)) = locale.rsplit_once('@') {
        match modifier.split('.').next().unwrap_or_default() {
            "latin" => {
                let (lang, rest) = normalized.split_once('-').unwrap_or((&normalized, ""));
                normalized = if rest.is_empty() {
                    format!("{lang}-Latn")
                } else {
                    format!("{lang}-Latn-{rest}")
                };
            }
            "cyrillic" => {
                let (lang, rest) = normalized.split_once('-').unwrap_or((&normalized, ""));
                normalized = if rest.is_empty() {
                    format!("{lang}-Cyrl")
                } else {
                    format!("{lang}-Cyrl-{rest}")
                };
            }
            "nynorsk" if normalized.starts_with("no") => normalized = "nn-NO".into(),
            _ => {}
        }
    }
    normalized
        .parse::<Locale>()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "en-US".into())
}

#[cfg(test)]
#[path = "locale_tests.rs"]
mod tests;
