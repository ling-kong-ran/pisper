use super::*;
use serde_json::{json, Value};
use std::sync::Mutex;
#[tokio::test]
async fn shell_discovery_matches_all_upstream_platform_and_lookup_traces() {
    let fixture: Value = serde_json::from_str(include_str!("shell_config_oracle.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let trace = Arc::new(Mutex::new(Vec::<Value>::new()));
        let windows = case["windows"].as_bool().unwrap_or(false);
        let existing = case["exists"].as_array().cloned().unwrap_or_default();
        let lookup = case["lookup"].clone();
        let host = ShellDiscovery {
            windows,
            environment: serde_json::from_value(case.get("env").cloned().unwrap_or(json!([])))
                .unwrap(),
            exists: {
                let trace = trace.clone();
                Arc::new(move |path| {
                    trace.lock().unwrap().push(json!(["exists", path]));
                    existing.contains(&json!(path))
                })
            },
            lookup: {
                let trace = trace.clone();
                Arc::new(move |name| {
                    let value = lookup[&name].clone();
                    let mut options = json!({"encoding":"utf-8","timeout":5000});
                    if windows {
                        options["windowsHide"] = json!(true);
                    }
                    trace.lock().unwrap().push(json!([
                        "lookup",
                        if windows { "where" } else { "which" },
                        [name],
                        options
                    ]));
                    Box::pin(async move {
                        if let Some(text) = value.as_str() {
                            Some(text.into())
                        } else if value["status"] == 0 {
                            value["stdout"].as_str().map(str::to_owned)
                        } else {
                            None
                        }
                    })
                })
            },
        };
        let result = if case["kind"] == "powershell" {
            get_powershell_config_with(&host).await
        } else {
            get_shell_config_with(case["custom"].as_str(), &host).await
        };
        let actual = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error}),
        };
        assert_eq!(actual, case["outcome"], "{}", case["id"]);
        assert_eq!(
            json!(*trace.lock().unwrap()),
            case["trace"],
            "{} trace",
            case["id"]
        );
    }
}
#[test]
fn shell_environment_preserves_path_key_order_and_exact_deduplication() {
    let fixture: Value = serde_json::from_str(include_str!("shell_config_oracle.json")).unwrap();
    for case in fixture["environments"].as_array().unwrap() {
        let env = serde_json::from_value(case["env"].clone()).unwrap();
        assert_eq!(
            json!(shell_environment_with(
                env,
                case["bin"].as_str().unwrap(),
                case["windows"].as_bool().unwrap()
            )),
            case["value"]
        );
    }
}
#[test]
fn binary_sanitization_follows_actual_upstream_predicate_including_lone_surrogates() {
    let fixture: Value = serde_json::from_str(include_str!("shell_config_oracle.json")).unwrap();
    for case in fixture["sanitize"].as_array().unwrap() {
        let units: Vec<u16> = serde_json::from_value(case["units"].clone()).unwrap();
        let expected: Vec<u16> = serde_json::from_value(case["value"].clone()).unwrap();
        assert_eq!(
            sanitize_binary_output_utf16(&Utf16Text::from_units(units.clone())).as_ref(),
            expected.as_slice()
        );
        if let Ok(text) = String::from_utf16(&units) {
            assert_eq!(
                sanitize_binary_output(&text)
                    .encode_utf16()
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
