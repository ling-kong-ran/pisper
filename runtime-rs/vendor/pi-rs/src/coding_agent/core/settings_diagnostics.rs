//! Settings error draining and stable diagnostic deduplication from
//! core/settings-diagnostics.ts. Startup and runtime managers may report the
//! same file error; distinct severity levels must not be merged.
use super::{
    agent_session_services::{AgentSessionRuntimeDiagnostic, DiagnosticType},
    settings_manager::{SettingsManager, SettingsScope},
};
use std::collections::HashSet;

pub fn collect_settings_diagnostics(
    settings_manager: &SettingsManager,
) -> Vec<AgentSessionRuntimeDiagnostic> {
    settings_manager
        .drain_errors()
        .into_iter()
        .map(|error| {
            let message = match error.path.filter(|p| !p.is_empty()) {
                Some(path) => format!("Invalid settings file {path}: {}", error.error),
                None => format!(
                    "Invalid {} settings: {}",
                    match error.scope {
                        SettingsScope::Global => "global",
                        SettingsScope::Project => "project",
                    },
                    error.error
                ),
            };
            AgentSessionRuntimeDiagnostic {
                kind: DiagnosticType::Warning,
                message,
            }
        })
        .collect()
}

pub fn deduplicate_diagnostics(
    diagnostics: &[AgentSessionRuntimeDiagnostic],
) -> Vec<AgentSessionRuntimeDiagnostic> {
    let mut seen = HashSet::new();
    diagnostics
        .iter()
        .filter(|d| seen.insert((d.kind as u8, d.message.as_str())))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "settings_diagnostics_tests.rs"]
mod tests;
