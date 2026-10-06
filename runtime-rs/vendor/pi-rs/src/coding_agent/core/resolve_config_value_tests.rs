//! Tests for the ported `coding-agent/src/core/resolve-config-value.ts`
//! (vendored dependency of the W3.6 model stack).
//!
//! Every table comes from `tests/fixtures/core_oracle_model/resolve_config_value.oracle.json`
//! (the real upstream module under node; generator `oracle_config_value.mjs`).
//! Live shell-command output is platform dependent and deliberately not in
//! the capture; the failing-command error text is (both failure channels
//! converge on the same throw upstream).

use super::*;
use serde_json::Value;

/// Upstream `resolve-config-value.ts` capture (real upstream module under
/// node; generator `tests/fixtures/core_oracle_model/oracle_config_value.mjs`).
const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_model/resolve_config_value.oracle.json");

/// Serializes the process-env reads/mutations against the fixed oracle env
/// names (cargo runs test targets in threads).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn oracle() -> Value {
    serde_json::from_str(ORACLE).unwrap()
}

fn env_of(case: &Value, key: &str) -> Option<ConfigEnv> {
    case[key].as_object().map(|entries| {
        entries
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect()
    })
}

#[test]
fn env_var_name_extraction_matches_the_upstream_oracle() {
    let oracle = oracle();
    for case in oracle["envVarNames"].as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        let expected_single = case["single"].as_str().map(str::to_string);
        assert_eq!(
            get_config_value_env_var_name(value),
            expected_single,
            "single name for {value:?}"
        );
        let expected_names: Vec<String> = case["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            get_config_value_env_var_names(value),
            expected_names,
            "names for {value:?}"
        );
        assert_eq!(
            is_command_config_value(value),
            case["isCommand"].as_bool().unwrap(),
            "isCommand for {value:?}"
        );
    }
}

#[test]
fn missing_env_names_and_configured_match_the_upstream_oracle() {
    let oracle = oracle();
    for case in oracle["missingEnv"].as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        let env = env_of(case, "env");
        let expected: Vec<String> = case["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            get_missing_config_value_env_var_names(value, env.as_ref()),
            expected,
            "missing names for {value:?}"
        );
    }
    for case in oracle["isConfigured"].as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        let env = env_of(case, "env");
        assert_eq!(
            is_config_value_configured(value, env.as_ref()),
            case["configured"].as_bool().unwrap(),
            "configured for {value:?}"
        );
    }
}

#[test]
fn resolution_matches_the_upstream_oracle() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The oracle pins `ORACLE_PENV` for the overlay/spot checks; scrub it so
    // both the overlay and unset cases are deterministic, then restore.
    let saved_penv = std::env::var("ORACLE_PENV").ok();
    std::env::remove_var("ORACLE_PENV");
    std::env::set_var("ORACLE_PENV", "process-value");

    let oracle = oracle();
    for case in oracle["resolve"].as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        let env = env_of(case, "env");
        // Overlay semantics: present overlay wins, absent overlay reads the
        // process env (set to the oracle's process value above).
        let expected = case["resolved"].as_str().map(str::to_string);
        let actual = resolve_config_value(value, env.as_ref());
        assert_eq!(actual, expected, "resolution for {value:?}");
    }

    match saved_penv {
        Some(value) => std::env::set_var("ORACLE_PENV", value),
        None => std::env::remove_var("ORACLE_PENV"),
    }
}

#[test]
fn or_throw_error_texts_match_the_upstream_oracle() {
    // environment-anchored: the failing cases pass no overlay env, so the
    // resolver reads the live process environment; the oracle captured a
    // machine where the template names are absent. Pin that precondition
    // (under the shared env lock) instead of inheriting ambient CI vars.
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut restore: Vec<(String, Option<String>)> = Vec::new();
    for case in oracle()["orThrow"].as_array().unwrap() {
        if case["ok"].as_bool().unwrap() {
            continue;
        }
        let value = case["value"].as_str().unwrap();
        for name in get_config_value_env_var_names(value) {
            if let Ok(previous) = std::env::var(&name) {
                restore.push((name.clone(), Some(previous)));
            } else {
                restore.push((name.clone(), None));
            }
            std::env::remove_var(&name);
        }
    }
    for case in oracle()["orThrow"].as_array().unwrap() {
        let value = case["value"].as_str().unwrap();
        let description = case["description"].as_str().unwrap();
        let env = env_of(case, "env");
        let result = resolve_config_value_or_throw(value, description, env.as_ref());
        if case["ok"].as_bool().unwrap() {
            assert_eq!(
                result.unwrap(),
                case["value"].as_str().unwrap(),
                "ok case {value:?}"
            );
        } else {
            assert_eq!(
                result.unwrap_err(),
                case["error"].as_str().unwrap(),
                "error case {value:?}"
            );
        }
    }
    for (name, previous) in restore {
        match previous {
            Some(value) => std::env::set_var(&name, value),
            None => std::env::remove_var(&name),
        }
    }
}

#[test]
fn header_resolution_matches_the_upstream_oracle() {
    let oracle = oracle();
    let read_headers = |case: &Value| -> Option<Vec<(String, String)>> {
        case["headers"].as_object().map(|entries| {
            entries
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
    };
    let as_map = |resolved: &Option<Vec<(String, String)>>| -> BTreeMap<String, String> {
        resolved.iter().flatten().cloned().collect()
    };
    for case in oracle["resolveHeaders"].as_array().unwrap() {
        let headers = read_headers(case);
        let env = env_of(case, "env");
        // The oracle stores a resolved record (or null); canonical JSON is
        // key-sorted, so map comparison is order-insensitive on both sides.
        let expected: Option<BTreeMap<String, String>> =
            case["resolved"].as_object().map(|entries| {
                entries
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            });
        let actual = resolve_headers(headers.as_deref(), env.as_ref());
        assert_eq!(as_map(&actual), expected.unwrap_or_default());
    }
    for case in oracle["resolveHeadersOrThrow"].as_array().unwrap() {
        let headers = read_headers(case);
        let env = env_of(case, "env");
        // The capture does not store the description argument (the generator
        // passed `model "p/m"`); it only shows in the error texts.
        let description = "model \"p/m\"";
        let result = resolve_headers_or_throw(headers.as_deref(), description, env.as_ref());
        if case["ok"].as_bool().unwrap() {
            let value = result.unwrap().unwrap();
            let expected: BTreeMap<String, String> = case["value"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect();
            assert_eq!(value.iter().cloned().collect::<BTreeMap<_, _>>(), expected);
        } else {
            assert_eq!(result.unwrap_err(), case["error"].as_str().unwrap());
        }
    }
}

/// The failing-command error text is deterministic across the two upstream
/// failure channels (configured shell vs default shell) and matches the
/// oracle's captured `commandError`.
#[test]
fn failing_command_produces_the_pinned_error_text() {
    let oracle = oracle();
    let expected_error = oracle["commandError"]["error"].as_str().unwrap();
    assert!(!oracle["commandError"]["ok"].as_bool().unwrap());

    clear_config_value_cache();
    let error =
        resolve_config_value_or_throw("!oracle-definitely-missing-binary", "header \"x-c\"", None)
            .unwrap_err();
    assert_eq!(error, expected_error);

    // Uncached path agrees.
    let uncached = resolve_config_value_uncached("!oracle-definitely-missing-binary", None);
    assert_eq!(uncached, None);
    clear_config_value_cache();
}

/// Template parsing edge cases exercised structurally through the public
/// single-name accessor (upstream behaviors covered by its unit battery):
/// `$$`/`$!` escapes, `${not valid}` literals, unterminated `${`.
#[test]
fn template_parsing_edges() {
    assert_eq!(get_config_value_env_var_name("$$A"), None);
    assert_eq!(resolve_config_value("$$A $!B", None).unwrap(), "$A !B");
    assert_eq!(get_config_value_env_var_names("${A}${B}"), ["A", "B"]);
    assert_eq!(
        get_config_value_env_var_names("prefix-$A1-b-$C"),
        ["A1", "C"]
    );
    // `${A` (unterminated): the `{` branch appends a literal `$` and resumes
    // one char later; the remainder has no further `$`, so it stays literal.
    assert_eq!(get_config_value_env_var_names("${A"), Vec::<String>::new());
    // `${bad name}` stays a literal.
    assert_eq!(
        resolve_config_value("${bad name}", None).unwrap(),
        "${bad name}"
    );
    // `$1x`: digits do not start an env name, so the `$` stays literal.
    assert_eq!(resolve_config_value("$1x", None).unwrap(), "$1x");
    // `a$b$c` with partial env: missing `$b` resolves the whole to undefined.
    assert_eq!(resolve_config_value("a$b$c", Some(&ConfigEnv::new())), None);
}

/// Empty-string overlay values fall through to the process environment
/// (JS `||`), and empty process values resolve to `undefined`.
#[test]
fn empty_values_fall_through_like_js_or() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::set_var("RCV_FALLBACK_VAR", "process-value");
    std::env::set_var("RCV_EMPTY_VAR", "");

    let mut env = ConfigEnv::new();
    env.insert("RCV_FALLBACK_VAR".to_string(), String::new());
    assert_eq!(
        resolve_config_value("$RCV_FALLBACK_VAR", Some(&env)).unwrap(),
        "process-value"
    );
    assert_eq!(resolve_config_value("$RCV_EMPTY_VAR", None), None);
    assert!(!is_config_value_configured("$RCV_EMPTY_VAR", None));

    std::env::remove_var("RCV_FALLBACK_VAR");
    std::env::remove_var("RCV_EMPTY_VAR");
}
