//! Port of upstream `coding-agent/src/utils/deprecation.ts`.
//!
//! Seam: the upstream `chalk.yellow` wrapper is replaced by a minimal yellow
//! formatter. The escape codes were pinned against the vendored chalk 6.0.0
//! (`\x1b[33m...\x1b[39m`; plain when color is disabled — chalk level 0).
//! Color enablement mirrors chalk's auto-detection as far as practical:
//! enabled when stderr is a terminal and `NO_COLOR` is unset, forced on by
//! `FORCE_COLOR` (any value), disabled by `NO_COLOR`.

use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock};

fn emitted_deprecation_warnings() -> &'static Mutex<HashSet<String>> {
    static EMITTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    EMITTED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn yellow(message: &str, color_enabled: bool) -> String {
    if color_enabled {
        format!("\x1b[33m{message}\x1b[39m")
    } else {
        message.to_string()
    }
}

fn color_enabled_for_stderr() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("FORCE_COLOR").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    std::io::stderr().is_terminal()
}

fn format_deprecation_warning(message: &str) -> String {
    yellow(
        &format!("Deprecation warning: {message}"),
        color_enabled_for_stderr(),
    )
}

/// Emit `Deprecation warning: <message>` on stderr once per message.
pub fn warn_deprecation(message: &str) {
    let mut emitted = emitted_deprecation_warnings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !emitted.insert(message.to_string()) {
        return;
    }
    eprintln!("{}", format_deprecation_warning(message));
}

/// Clear deprecation warning state. Exported for tests.
pub fn clear_deprecation_warnings_for_tests() {
    emitted_deprecation_warnings()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    #[test]
    fn formats_with_chalk_pinned_escapes_when_colored() {
        // chalk.level 1..=3
        assert_eq!(
            yellow("Deprecation warning: x", true),
            oracle::CHALK_YELLOW[1]
        );
        // chalk.level 0 (color disabled)
        assert_eq!(
            yellow("Deprecation warning: x", false),
            oracle::CHALK_YELLOW[0]
        );
    }

    #[test]
    fn emits_each_message_once_until_cleared() {
        clear_deprecation_warnings_for_tests();
        let mut seen: Vec<String> = Vec::new();
        {
            let mut emitted = emitted_deprecation_warnings()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for message in ["first", "second", "first"] {
                if emitted.insert(message.to_string()) {
                    seen.push(message.to_string());
                }
            }
        }
        assert_eq!(seen, vec!["first", "second"]);
        clear_deprecation_warnings_for_tests();
        // After clearing, the same message is emitted again.
        let mut emitted = emitted_deprecation_warnings()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(emitted.insert("first".to_string()));
        drop(emitted);
        clear_deprecation_warnings_for_tests();
    }
}
