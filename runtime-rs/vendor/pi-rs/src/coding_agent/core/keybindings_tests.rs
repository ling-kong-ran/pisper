//! Tests for the ported `coding-agent/src/core/keybindings.ts`.
//!
//! Sources of truth:
//! - upstream `test/keybindings.test.ts` (Windows/WSL defaults suite),
//! - upstream `test/keybindings-migration.test.ts` (the in-memory loading
//!   cases; the file-rewrite driver `runMigrations` lives in
//!   `src/migrations.ts`, outside this slice — the migration data and
//!   ordering are covered through `migrateKeybindingsConfig` itself),
//! - oracle captures from the real upstream module for four platform
//!   variants: the full `KEYBINDINGS` table (ids, order, defaultKeys,
//!   descriptions), `useWindowsKeybindings` cases, migration ordering, and
//!   `KeybindingsManager.create` file loads.
//!
//! The oracle captures are parsed as [`OrderedValue`]: object key order in
//! the captured configs is semantic (upstream `JSON.stringify` wrote them in
//! JS insertion order), which a plain `serde_json::Value` parse would sort
//! away. Table and migration comparisons are platform-independent (the table
//! is parameterized); manager file-load comparisons run against the oracle
//! variant matching the host platform (the manager resolves host defaults,
//! like upstream).

use std::collections::HashMap;

use serde_json::Value;

use super::{
    keybindings, keybindings_for, migrate_keybindings_config, order_keybindings_config,
    to_keybindings_config, use_windows_keybindings, use_windows_keybindings_with_env,
    KeybindingsConfig, KeybindingsManager, OrderedConfig, ResolvedKeys, UserKeys,
};
use crate::coding_agent::core::model_config::OrderedValue;
use crate::coding_agent::core::oracle_data;

fn oracle(value: &str) -> OrderedValue {
    serde_json::from_str::<OrderedValue>(value).unwrap()
}

fn oget<'a>(value: &'a OrderedValue, key: &str) -> &'a OrderedValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("oracle missing key {key}"))
}

fn o_str(value: &OrderedValue) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
}

fn o_bool(value: &OrderedValue) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| panic!("expected bool, got {value:?}"))
}

fn o_entries(value: &OrderedValue) -> &[(String, OrderedValue)] {
    match value {
        OrderedValue::Object(entries) => entries,
        other => panic!("expected object, got {other:?}"),
    }
}

fn o_value(value: &OrderedValue) -> Value {
    value.to_serde()
}

fn expected_pairs(value: &OrderedValue) -> Vec<(String, Value)> {
    o_entries(value)
        .iter()
        .map(|(key, item)| (key.clone(), o_value(item)))
        .collect()
}

/// Compare actual (key, value) pairs against expected pairs in exact order.
fn compare_pairs(actual: &[(String, Value)], expected: &[(String, Value)], label: &str) {
    let expected_keys: Vec<&str> = expected.iter().map(|(key, _)| key.as_str()).collect();
    let actual_keys: Vec<&str> = actual.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(actual_keys, expected_keys, "{label}: key order");
    for (key, expected_value) in expected {
        let actual_value = actual
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("{label}: missing {key}"));
        assert_eq!(actual_value, expected_value, "{label}: value for {key}");
    }
}

fn user_keys_to_value(config: &KeybindingsConfig) -> Vec<(String, Value)> {
    config
        .entries()
        .iter()
        .map(|(key, keys)| {
            let value = match keys {
                UserKeys::One(text) => Value::String(text.clone()),
                UserKeys::Many(list) => Value::Array(
                    list.iter()
                        .map(|entry| Value::String(entry.clone()))
                        .collect(),
                ),
            };
            (key.clone(), value)
        })
        .collect()
}

fn effective_to_value(effective: &[(String, ResolvedKeys)]) -> Vec<(String, Value)> {
    effective
        .iter()
        .map(|(key, resolved)| {
            let value = match resolved {
                ResolvedKeys::One(text) => Value::String(text.clone()),
                ResolvedKeys::Many(list) => Value::Array(
                    list.iter()
                        .map(|entry| Value::String(entry.clone()))
                        .collect(),
                ),
            };
            (key.clone(), value)
        })
        .collect()
}

/// Compare the ported table against one oracle capture (platform + env).
fn compare_keybindings_table(oracle_text: &str) {
    let capture = oracle(oracle_text);
    let platform = o_str(oget(&capture, "platform"));
    let windows = o_bool(oget(&capture, "detected"));
    let wsl_distro = o_str(oget(&capture, "wslDistro"));

    // windowsKeybindings detection from the oracle's inputs.
    let env_map = if wsl_distro.is_empty() {
        HashMap::new()
    } else {
        HashMap::from([("WSL_DISTRO_NAME".to_string(), wsl_distro.to_string())])
    };
    assert_eq!(
        use_windows_keybindings_with_env(platform, &env_map),
        windows,
        "windowsKeybindings detection ({platform}, wsl={wsl_distro})"
    );
    let ported = keybindings_for(platform, windows);

    // ids in order, byte-equal.
    let oracle_ids: Vec<&str> = match oget(&capture, "keybinding_ids") {
        OrderedValue::Array(ids) => ids.iter().map(o_str).collect(),
        other => panic!("keybinding_ids {other:?}"),
    };
    let ported_ids: Vec<&str> = ported.iter().map(|(id, _)| *id).collect();
    assert_eq!(ported_ids, oracle_ids, "keybinding id order ({platform})");

    // defaultKeys (a bare oracle string is a single key) + descriptions.
    for entry in match oget(&capture, "keybindings") {
        OrderedValue::Array(entries) => entries,
        other => panic!("keybindings {other:?}"),
    } {
        let id = o_str(oget(entry, "id"));
        let expected: Vec<String> = match oget(entry, "defaultKeys") {
            OrderedValue::String(text) => vec![text.clone()],
            OrderedValue::Array(items) => {
                items.iter().map(|item| o_str(item).to_string()).collect()
            }
            other => panic!("unexpected defaultKeys {other:?}"),
        };
        let definition = &ported
            .iter()
            .find(|(entry_id, _)| *entry_id == id)
            .unwrap()
            .1;
        assert_eq!(
            definition.default_keys, expected,
            "defaultKeys for {id} ({platform})"
        );
        assert_eq!(
            definition.description,
            Some(o_str(oget(entry, "description"))),
            "description for {id} ({platform})"
        );
    }

    // The four overridden tui entries keep their original descriptions.
    for entry in match oget(&capture, "tui_overridden_kept_description") {
        OrderedValue::Array(entries) => entries,
        other => panic!("tui_overridden_kept_description {other:?}"),
    } {
        let id = o_str(oget(entry, "id"));
        let definition = &ported
            .iter()
            .find(|(entry_id, _)| *entry_id == id)
            .unwrap()
            .1;
        assert_eq!(
            definition.description,
            Some(o_str(oget(entry, "description"))),
            "kept description for {id} ({platform})"
        );
    }

    // useWindowsKeybindings explicit-argument cases.
    for case in match oget(&capture, "use_windows_cases") {
        OrderedValue::Array(entries) => entries,
        other => panic!("use_windows_cases {other:?}"),
    } {
        let case_platform = o_str(oget(case, "platform"));
        let env: HashMap<String, String> = o_entries(oget(case, "env"))
            .iter()
            .map(|(key, value)| (key.clone(), o_str(value).to_string()))
            .collect();
        assert_eq!(
            use_windows_keybindings_with_env(case_platform, &env),
            o_bool(oget(case, "result")),
            "useWindowsKeybindings({}, {:?})",
            case_platform,
            env
        );
    }

    // Migration cases: ordered config + migrated flag.
    for case in match oget(&capture, "migration_cases") {
        OrderedValue::Array(entries) => entries,
        other => panic!("migration_cases {other:?}"),
    } {
        let raw: Vec<(String, Value)> = o_entries(oget(case, "rawConfig"))
            .iter()
            .map(|(key, value)| (key.clone(), o_value(value)))
            .collect();
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert_eq!(
            migrated,
            o_bool(oget(case, "migrated")),
            "migrated flag for {raw:?}"
        );
        compare_pairs(
            config.entries(),
            &expected_pairs(oget(case, "config")),
            &format!("migration config ({platform})"),
        );
    }
}

#[test]
fn keybindings_tables_match_the_oracle_on_all_platforms() {
    compare_keybindings_table(oracle_data::KEYBINDINGS_WIN32);
    compare_keybindings_table(oracle_data::KEYBINDINGS_LINUX);
    compare_keybindings_table(oracle_data::KEYBINDINGS_LINUX_WSL);
    compare_keybindings_table(oracle_data::KEYBINDINGS_DARWIN);
}

/// Replay the oracle's `KeybindingsManager.create` file-load cases on the
/// host platform: user bindings, effective config, and reload behavior.
#[test]
fn manager_file_loads_match_the_oracle() {
    // The manager resolves host defaults (like upstream's
    // `useWindowsKeybindings()`): on a linux host it still detects WSL from
    // `WSL_DISTRO_NAME`/`WSL_INTEROP` and switches to the windows defaults
    // (e.g. `tui.editor.undo` -> `alt+z` instead of the plain-linux
    // `ctrl+-`), so the host-matching capture is the WSL one there — running
    // the plain-linux capture inside WSL would compare against defaults
    // upstream itself would not produce on that host.
    let host_capture = oracle(if cfg!(windows) {
        oracle_data::KEYBINDINGS_WIN32
    } else if cfg!(target_os = "macos") {
        oracle_data::KEYBINDINGS_DARWIN
    } else if use_windows_keybindings() {
        oracle_data::KEYBINDINGS_LINUX_WSL
    } else {
        oracle_data::KEYBINDINGS_LINUX
    });
    let file_contents: &[(&str, Option<String>)] = &[
        (
            "legacy_names",
            Some(
                serde_json::to_string_pretty(&serde_json::json!({
                    "cursorUp": ["up", "ctrl+p"],
                    "expandTools": "ctrl+x",
                }))
                .unwrap()
                    + "\n",
            ),
        ),
        (
            "namespaced_wins",
            Some(
                serde_json::to_string_pretty(&serde_json::json!({
                    "expandTools": "ctrl+x",
                    "app.tools.expand": "ctrl+y",
                }))
                .unwrap()
                    + "\n",
            ),
        ),
        ("missing_file", None),
        ("invalid_json", Some("{ nope".to_string())),
        ("not_an_object", Some(serde_json::json!([1, 2]).to_string())),
        (
            "array_values_dropped",
            Some(
                serde_json::json!({
                    "app.clear": "ctrl+c",
                    "tui.input.submit": ["enter", 42],
                })
                .to_string(),
            ),
        ),
    ];

    for case in match oget(&host_capture, "manager_cases") {
        OrderedValue::Array(entries) => entries,
        other => panic!("manager_cases {other:?}"),
    } {
        let name = o_str(oget(case, "name"));
        let content = file_contents
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .and_then(|(_, content)| content.clone());
        let dir = tempfile::TempDir::with_prefix("pi-keybindings-oracle-").unwrap();
        let config_path = dir.path().join("keybindings.json");
        if let Some(content) = content.as_deref() {
            std::fs::write(&config_path, content).unwrap();
        }

        let mut manager = KeybindingsManager::create(dir.path().to_str().unwrap());
        compare_pairs(
            &user_keys_to_value(&manager.get_user_bindings()),
            &expected_pairs(oget(oget(case, "before"), "user")),
            &format!("{name}: user bindings"),
        );
        compare_pairs(
            &effective_to_value(&manager.get_effective_config()),
            &expected_pairs(oget(oget(case, "before"), "effective")),
            &format!("{name}: effective config"),
        );

        if content.is_some() {
            std::fs::write(
                &config_path,
                serde_json::to_string_pretty(&serde_json::json!({
                    "app.clear": "ctrl+shift+c",
                }))
                .unwrap()
                    + "\n",
            )
            .unwrap();
            manager.reload();
            compare_pairs(
                &effective_to_value(&manager.get_effective_config()),
                &expected_pairs(oget(oget(case, "afterReload"), "effective")),
                &format!("{name}: after reload"),
            );
            manager.reload();
            compare_pairs(
                &effective_to_value(&manager.get_effective_config()),
                &expected_pairs(oget(case, "effectiveAfterSecondReload")),
                &format!("{name}: after second reload"),
            );
        }
    }
}

/// Upstream `test/keybindings.test.ts` — "applies the detected defaults
/// consistently" (evaluated against the host platform like upstream does).
#[test]
fn applies_the_detected_defaults_consistently() {
    let windows = use_windows_keybindings();
    let native_windows = cfg!(windows);
    let table = keybindings();
    let default_keys = |id: &str| -> Value {
        let definition = &table.iter().find(|(entry, _)| *entry == id).unwrap().1;
        if definition.default_keys.len() == 1 {
            Value::String(definition.default_keys[0].clone())
        } else {
            Value::Array(
                definition
                    .default_keys
                    .iter()
                    .map(|key| Value::String(key.clone()))
                    .collect(),
            )
        }
    };
    let expected = |single: &str, windows_value: &str| -> Value {
        Value::String(if windows {
            windows_value.to_string()
        } else {
            single.to_string()
        })
    };
    assert_eq!(
        default_keys("app.clipboard.pasteImage"),
        expected("ctrl+v", "alt+v")
    );
    assert_eq!(
        default_keys("tui.altScreen.search"),
        expected("ctrl+shift+f", "ctrl+f")
    );
    assert_eq!(
        default_keys("app.message.followUp"),
        expected("alt+enter", "ctrl+q")
    );
    assert_eq!(
        default_keys("app.model.cycleBackward"),
        expected("shift+ctrl+p", "alt+p")
    );
    let undo = if native_windows {
        "ctrl+z"
    } else if windows {
        "alt+z"
    } else {
        "ctrl+-"
    };
    assert_eq!(default_keys("tui.editor.undo"), Value::String(undo.into()));
    let previous_prompt = if windows {
        Value::String("ctrl+up".into())
    } else {
        Value::Array(vec![
            Value::String("ctrl+shift+up".into()),
            Value::String("ctrl+up".into()),
        ])
    };
    assert_eq!(
        default_keys("tui.altScreen.previousPrompt"),
        previous_prompt
    );
    let next_prompt = if windows {
        Value::String("ctrl+down".into())
    } else {
        Value::Array(vec![
            Value::String("ctrl+shift+down".into()),
            Value::String("ctrl+down".into()),
        ])
    };
    assert_eq!(default_keys("tui.altScreen.nextPrompt"), next_prompt);
    assert_eq!(
        default_keys("app.message.dequeue"),
        expected("alt+up", "alt+q")
    );
}

/// Upstream `test/keybindings.test.ts` — explicit platform/env cases.
#[test]
fn uses_windows_keybindings_on_native_windows() {
    assert!(use_windows_keybindings_with_env("win32", &HashMap::new()));
}

#[test]
fn uses_windows_keybindings_in_wsl() {
    assert!(use_windows_keybindings_with_env(
        "linux",
        &HashMap::from([("WSL_DISTRO_NAME".to_string(), "Ubuntu".to_string())])
    ));
    assert!(use_windows_keybindings_with_env(
        "linux",
        &HashMap::from([(
            "WSL_INTEROP".to_string(),
            "/run/WSL/123_interop".to_string()
        )])
    ));
}

#[test]
fn does_not_use_windows_keybindings_from_wt_session_alone() {
    assert!(!use_windows_keybindings_with_env(
        "linux",
        &HashMap::from([("WT_SESSION".to_string(), "session".to_string())])
    ));
}

#[test]
fn keeps_non_windows_defaults_on_other_platforms() {
    assert!(!use_windows_keybindings_with_env("linux", &HashMap::new()));
    assert!(!use_windows_keybindings_with_env("darwin", &HashMap::new()));
}

/// Upstream `test/keybindings-migration.test.ts` — "rewrites old key names to
/// namespaced ids" (data level; the file rewrite lives in migrations.ts,
/// outside this slice). The rewrite output is in KEYBINDINGS order: the tui
/// id first, then the app id.
#[test]
fn rewrites_old_key_names_to_namespaced_ids() {
    let raw = vec![
        ("cursorUp".to_string(), serde_json::json!(["up", "ctrl+p"])),
        ("expandTools".to_string(), serde_json::json!("ctrl+x")),
    ];
    let (config, migrated) = migrate_keybindings_config(&raw);
    assert!(migrated);
    compare_pairs(
        config.entries(),
        &[
            (
                "tui.editor.cursorUp".to_string(),
                serde_json::json!(["up", "ctrl+p"]),
            ),
            ("app.tools.expand".to_string(), serde_json::json!("ctrl+x")),
        ],
        "rewrite",
    );
}

/// Upstream `test/keybindings-migration.test.ts` — "keeps the namespaced
/// value when old and new names both exist".
#[test]
fn keeps_the_namespaced_value_when_old_and_new_names_both_exist() {
    let raw = vec![
        ("expandTools".to_string(), serde_json::json!("ctrl+x")),
        ("app.tools.expand".to_string(), serde_json::json!("ctrl+y")),
    ];
    let (config, migrated) = migrate_keybindings_config(&raw);
    assert!(migrated);
    compare_pairs(
        config.entries(),
        &[("app.tools.expand".to_string(), serde_json::json!("ctrl+y"))],
        "namespaced wins",
    );
}

/// Upstream `test/keybindings-migration.test.ts` — "loads old key names in
/// memory before the file is rewritten", via `KeybindingsManager.create`.
#[test]
fn loads_old_key_names_in_memory_before_the_file_is_rewritten() {
    let dir = tempfile::TempDir::with_prefix("pi-keybindings-test-").unwrap();
    let config_path = dir.path().join("keybindings.json");
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "selectConfirm": "enter",
            "interrupt": "ctrl+x",
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let keybindings = KeybindingsManager::create(dir.path().to_str().unwrap());

    compare_pairs(
        &user_keys_to_value(&keybindings.get_user_bindings()),
        &[
            ("tui.select.confirm".to_string(), serde_json::json!("enter")),
            ("app.interrupt".to_string(), serde_json::json!("ctrl+x")),
        ],
        "user bindings",
    );
    let effective = effective_to_value(&keybindings.get_effective_config());
    assert_eq!(
        effective
            .iter()
            .find(|(id, _)| id == "tui.select.confirm")
            .map(|(_, value)| value),
        Some(&Value::String("enter".to_string()))
    );
    assert_eq!(
        effective
            .iter()
            .find(|(id, _)| id == "app.interrupt")
            .map(|(_, value)| value),
        Some(&Value::String("ctrl+x".to_string()))
    );
}

/// `getEffectiveConfig` renders single-key bindings as bare strings and
/// multi-key bindings as arrays (upstream `getResolvedBindings`).
#[test]
fn effective_config_uses_bare_string_for_single_key() {
    let manager = KeybindingsManager::new(KeybindingsConfig::default(), None);
    let effective = manager.get_effective_config();
    let (_, resolved) = effective
        .iter()
        .find(|(id, _)| id == "app.interrupt")
        .unwrap();
    assert_eq!(*resolved, ResolvedKeys::One("escape".to_string()));
    let (_, multi) = effective
        .iter()
        .find(|(id, _)| id == "tui.editor.cursorLeft")
        .unwrap();
    assert_eq!(
        *multi,
        ResolvedKeys::Many(vec!["left".to_string(), "ctrl+b".to_string()])
    );
}

/// `reload()` picks up an externally rewritten keybindings.json.
#[test]
fn reload_picks_up_external_changes() {
    let dir = tempfile::TempDir::with_prefix("pi-keybindings-reload-").unwrap();
    let config_path = dir.path().join("keybindings.json");
    std::fs::write(&config_path, "{\n  \"app.clear\": \"ctrl+o\"\n}\n").unwrap();
    let mut manager = KeybindingsManager::create(dir.path().to_str().unwrap());
    assert_eq!(manager.get_keys("app.clear"), vec!["ctrl+o".to_string()]);
    std::fs::write(&config_path, "{\n  \"app.clear\": \"ctrl+shift+c\"\n}\n").unwrap();
    manager.reload();
    assert_eq!(
        manager.get_keys("app.clear"),
        vec!["ctrl+shift+c".to_string()]
    );
}

/// Malformed/absent config files behave like upstream `loadRawConfig`:
/// `undefined` → empty user bindings, defaults intact.
#[test]
fn malformed_and_missing_config_files_fall_back_to_defaults() {
    let dir = tempfile::TempDir::with_prefix("pi-keybindings-bad-").unwrap();
    let manager = KeybindingsManager::create(dir.path().to_str().unwrap());
    assert!(manager.get_user_bindings().is_empty());
    assert_eq!(
        manager.get_keys("app.interrupt"),
        vec!["escape".to_string()]
    );

    std::fs::write(dir.path().join("keybindings.json"), "{ nope").unwrap();
    let manager = KeybindingsManager::create(dir.path().to_str().unwrap());
    assert!(manager.get_user_bindings().is_empty());

    std::fs::write(dir.path().join("keybindings.json"), "[1, 2]").unwrap();
    let manager = KeybindingsManager::create(dir.path().to_str().unwrap());
    // Arrays are JS objects: entries land under index keys and are dropped by
    // toKeybindingsConfig (non-string values).
    assert!(manager.get_user_bindings().is_empty());
}

/// Unknown extras order after the known ids with a JS-style sort; unknown
/// values pass migration untouched and non-string bindings are dropped by
/// `toKeybindingsConfig`.
#[test]
fn migration_orders_known_ids_first_then_sorted_extras() {
    let capture = oracle(oracle_data::KEYBINDINGS_WIN32);
    let migration_cases = match oget(&capture, "migration_cases") {
        OrderedValue::Array(entries) => entries,
        other => panic!("migration_cases {other:?}"),
    };
    for case in migration_cases.iter().take(5) {
        let raw: Vec<(String, Value)> = o_entries(oget(case, "rawConfig"))
            .iter()
            .map(|(key, value)| (key.clone(), o_value(value)))
            .collect();
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert_eq!(migrated, o_bool(oget(case, "migrated")));
        compare_pairs(
            config.entries(),
            &expected_pairs(oget(case, "config")),
            "migration case",
        );
    }

    // toKeybindingsConfig keeps only strings and all-string arrays; the
    // case-4 values (["enter", 42] array, object, null) are all dropped.
    let case = &migration_cases[4];
    let raw: Vec<(String, Value)> = o_entries(oget(case, "rawConfig"))
        .iter()
        .map(|(key, value)| (key.clone(), o_value(value)))
        .collect();
    let (config, _) = migrate_keybindings_config(&raw);
    let user_config = to_keybindings_config(config.entries());
    assert!(user_config.is_empty(), "non-string bindings dropped");

    // The manager oracle case with the same shape keeps only the plain
    // string binding.
    let user_config = to_keybindings_config(&[
        ("app.clear".to_string(), serde_json::json!("ctrl+c")),
        (
            "tui.input.submit".to_string(),
            serde_json::json!(["enter", 42]),
        ),
    ]);
    assert_eq!(
        user_config
            .entries()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["app.clear"]
    );
    let all_strings = to_keybindings_config(&[(
        "tui.input.submit".to_string(),
        serde_json::json!(["enter", "tab"]),
    )]);
    assert_eq!(
        all_strings.get("tui.input.submit"),
        Some(&UserKeys::Many(vec![
            "enter".to_string(),
            "tab".to_string()
        ]))
    );
}

/// `orderKeybindingsConfig` places known ids first (KEYBINDINGS order) and
/// sorts extras by UTF-16 code units.
#[test]
fn order_keybindings_config_sorts_extras_like_js() {
    let raw = vec![
        ("zzz.custom".to_string(), serde_json::json!(1)),
        ("app.clear".to_string(), serde_json::json!("ctrl+c")),
        ("aaa.beta".to_string(), serde_json::json!(true)),
        ("undo".to_string(), serde_json::json!("ctrl+-")),
    ];
    let (migrated, _) = migrate_keybindings_config(&raw);
    let ordered = order_keybindings_config(&migrated);
    let keys = ordered.keys();
    assert_eq!(keys.first(), Some(&"tui.editor.undo"));
    let undo_index = keys
        .iter()
        .position(|key| *key == "tui.editor.undo")
        .unwrap();
    let app_clear_index = keys.iter().position(|key| *key == "app.clear").unwrap();
    assert!(undo_index < app_clear_index);
    // JS default sort on the extras: "Undo"? No — the extras here are
    // "aaa.beta" (0x61) < "zzz.custom" (0x7A); "Undo" (0x55) would sort
    // before both, which the Uppercase case below pins.
    let extras: Vec<&str> = keys[keys.len() - 2..].to_vec();
    assert_eq!(extras, vec!["aaa.beta", "zzz.custom"]);

    let uppercase = OrderedConfig::new(vec![
        ("zzz.custom".to_string(), serde_json::json!(1)),
        ("Undo".to_string(), serde_json::json!(2)),
        ("aaa.beta".to_string(), serde_json::json!(3)),
    ]);
    let ordered = order_keybindings_config(&uppercase);
    let extras: Vec<&str> = ordered.keys()[..].to_vec();
    assert_eq!(extras, vec!["Undo", "aaa.beta", "zzz.custom"]);
}
