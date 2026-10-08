use super::*;
use crate::coding_agent::core::{
    settings_manager::{SettingsLockCallback, SettingsManagerCreateOptions, SettingsStorage},
    trust_test_support::{oracle, Fixture},
};
use serde_json::json;
use std::sync::Arc;

struct ErrorStorage;
impl SettingsStorage for ErrorStorage {
    fn with_lock(&self, scope: SettingsScope, f: SettingsLockCallback<'_>) -> Result<(), String> {
        if scope == SettingsScope::Global {
            Err("backend failed".into())
        } else {
            f(None).map(|_| ())
        }
    }
}
#[test]
fn drains_real_storage_errors_once_and_uses_scope_when_path_is_missing() {
    let settings = SettingsManager::from_storage(
        Arc::new(ErrorStorage),
        SettingsManagerCreateOptions::default(),
    );
    let diagnostics = collect_settings_diagnostics(&settings);
    assert_eq!(
        json!(diagnostics),
        json!([{"type":"warning","message":"Invalid global settings: backend failed"}])
    );
    assert!(collect_settings_diagnostics(&settings).is_empty());
}
#[test]
fn file_backed_diagnostics_include_path_and_both_scopes() {
    let f = Fixture::new();
    f.write("/root/agent/settings.json", "{");
    f.write("/root/project/.pi/settings.json", "{");
    let settings = SettingsManager::create_with(
        &f.p("/root/project"),
        &f.p("/root/agent"),
        SettingsManagerCreateOptions::default(),
    )
    .unwrap();
    let diagnostics = collect_settings_diagnostics(&settings);
    assert_eq!(diagnostics.len(), 2);
    assert!(f
        .text(&diagnostics[0].message)
        .starts_with("Invalid settings file /root/agent/settings.json: "));
    assert!(f
        .text(&diagnostics[1].message)
        .starts_with("Invalid settings file /root/project/.pi/settings.json: "));
    assert!(diagnostics
        .iter()
        .all(|d| d.kind == DiagnosticType::Warning));
    assert!(collect_settings_diagnostics(&settings).is_empty());
}
#[test]
fn dedup_preserves_first_occurrence_type_and_exact_message_from_oracle() {
    let expected = oracle()["diagnostics"].clone();
    let input: Vec<AgentSessionRuntimeDiagnostic> =
        serde_json::from_value(expected["input"].clone()).unwrap();
    assert_eq!(
        json!(deduplicate_diagnostics(&input)),
        expected["deduplicated"]
    );
    let mut with_nulls = input;
    with_nulls.push(AgentSessionRuntimeDiagnostic {
        kind: DiagnosticType::Warning,
        message: "a\0b".into(),
    });
    with_nulls.push(AgentSessionRuntimeDiagnostic {
        kind: DiagnosticType::Warning,
        message: "a\0b".into(),
    });
    assert_eq!(
        deduplicate_diagnostics(&with_nulls).len(),
        expected["deduplicated"].as_array().unwrap().len() + 1
    );
}
