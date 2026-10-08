//! Tests for the ported `coding-agent/src/core/model-config.ts`.
//!
//! The fixture battery in `tests/fixtures/core_oracle/model_config.oracle.json` was
//! captured from the real upstream `ModelConfig.load` running under node with
//! the local typebox install (upstream pins typebox 1.3.27; the capture used
//! 1.3.11 — patch-level, disclosed). The port's hand-written typebox-faithful
//! validator must reproduce:
//! - schema-error rendering byte-for-byte (modulo the trailing
//!   `\n\nFile: <path>` suffix, which embeds the loader's own temp path),
//! - provider id order (document order),
//! - provider payloads rendered exactly like `JSON.stringify(provider)`.
//!
//! Fixture bodies are read through [`OrderedValue`] so the oracle's document
//! key order survives (a plain `serde_json::Value` parse would sort keys and
//! erase exactly the ordering under test); they are written to the fixture
//! file in compact form (whitespace only — no assertion depends on fixture
//! file bytes).

use super::{ModelConfig, OrderedValue};
use crate::coding_agent::core::oracle_data;

fn oracle() -> OrderedValue {
    serde_json::from_str::<OrderedValue>(oracle_data::MODEL_CONFIG).unwrap()
}

fn text<'a>(value: &'a OrderedValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(OrderedValue::as_str)
}

fn fixtures(capture: &OrderedValue) -> Vec<(&str, &OrderedValue)> {
    match capture.get("fixtures") {
        Some(OrderedValue::Object(entries)) => entries
            .iter()
            .map(|(name, case)| (name.as_str(), case))
            .collect(),
        _ => Vec::new(),
    }
}

/// Write the fixture (compact, document order) and load it.
async fn load_fixture(config: &OrderedValue) -> ModelConfig {
    let dir = tempfile::TempDir::with_prefix("pi-model-config-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, config.to_json_string()).unwrap();
    ModelConfig::load(Some(path.to_str().unwrap()))
        .await
        .unwrap()
}

fn compare_case(actual: &ModelConfig, case: &OrderedValue, label: &str) {
    // Error: byte-equal up to the `\n\nFile: <path>` suffix.
    let actual_error = actual.get_error().unwrap_or_default();
    let expected_error = text(case, "error").unwrap_or("");
    if expected_error.is_empty() {
        assert!(
            actual_error.is_empty(),
            "{label}: expected no error, got {actual_error:?}"
        );
    } else {
        let expected_prefix = expected_error
            .split("\n\nFile: ")
            .next()
            .unwrap()
            .to_string();
        let actual_prefix = actual_error.split("\n\nFile: ").next().unwrap().to_string();
        assert_eq!(
            actual_prefix, expected_prefix,
            "{label}: schema error rendering"
        );
        assert!(
            actual_error.contains("\n\nFile: "),
            "{label}: error must carry the file suffix"
        );
    }

    // Provider ids in document order.
    let expected_ids: Vec<&str> = match case.get("providerIds") {
        Some(OrderedValue::Array(ids)) => ids.iter().map(|id| id.as_str().unwrap()).collect(),
        _ => Vec::new(),
    };
    assert_eq!(
        actual.get_provider_ids(),
        expected_ids,
        "{label}: provider ids"
    );

    // Raw provider payloads in document order (the deep-frozen JS object).
    for (provider_id, field) in [
        ("p", "providerJson"),
        ("a", "providerAJson"),
        ("zeta", "providerZetaJson"),
    ] {
        let Some(expected_string) = text(case, field) else {
            continue;
        };
        let actual_raw = actual.get_provider_raw(provider_id).expect("raw provider");
        assert_eq!(
            actual_raw.to_json_string(),
            expected_string,
            "{label}: provider {provider_id} JSON"
        );
    }
}

#[tokio::test]
async fn model_config_load_matches_the_oracle_battery() {
    let capture = oracle();
    for (name, case) in fixtures(&capture) {
        let Some(config) = case.get("input") else {
            continue;
        };
        let loaded = load_fixture(config).await;
        compare_case(&loaded, case, name);
    }
}

/// JSONC (comments + trailing commas) and a leading BOM are stripped before
/// parsing, like upstream `stripJsonComments(stripBom(content))`.
#[tokio::test]
async fn jsonc_with_bom_loads_cleanly() {
    let capture = oracle();
    let case = capture
        .get("fixtures")
        .unwrap()
        .get("jsonc_with_bom")
        .unwrap();
    let raw = text(case, "rawInput").unwrap();
    let dir = tempfile::TempDir::with_prefix("pi-model-config-jsonc-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, raw).unwrap();
    let loaded = ModelConfig::load(Some(path.to_str().unwrap()))
        .await
        .unwrap();
    assert!(loaded.get_error().is_none());
    assert_eq!(loaded.get_provider_ids(), vec!["p"]);
    assert_eq!(
        loaded.get_provider_raw("p").unwrap().to_json_string(),
        text(case, "providerJson").unwrap()
    );
}

/// Malformed JSON produces the parse-failure channel. The prefix text comes
/// from V8 upstream and from serde_json on the port (disclosed divergence);
/// the `Failed to parse models.json:` framing and file suffix must match.
#[tokio::test]
async fn malformed_json_reports_the_parse_error() {
    let capture = oracle();
    let case = capture
        .get("fixtures")
        .unwrap()
        .get("malformed_json")
        .unwrap();
    let dir = tempfile::TempDir::with_prefix("pi-model-config-bad-").unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, text(case, "rawInput").unwrap()).unwrap();
    let loaded = ModelConfig::load(Some(path.to_str().unwrap()))
        .await
        .unwrap();
    let error = loaded.get_error().expect("parse error expected");
    assert!(error.starts_with("Failed to parse models.json: "));
    assert!(error.contains("\n\nFile: "));
    // The upstream (V8) prefix is recorded for disclosure.
    assert!(text(case, "errorPrefix").is_some());
}

/// A missing file yields an empty config without error (upstream ENOENT).
#[tokio::test]
async fn missing_file_yields_an_empty_config() {
    let dir = tempfile::TempDir::with_prefix("pi-model-config-missing-").unwrap();
    let path = dir.path().join("nope").join("models.json");
    let loaded = ModelConfig::load(Some(path.to_str().unwrap()))
        .await
        .unwrap();
    assert!(loaded.get_error().is_none());
    assert!(loaded.get_provider_ids().is_empty());
    assert!(loaded.get_provider("p").is_none());
}

/// An undefined path yields an empty config without error.
#[tokio::test]
async fn undefined_path_yields_an_empty_config() {
    let loaded = ModelConfig::load(None).await.unwrap();
    assert!(loaded.get_error().is_none());
    assert!(loaded.get_provider_ids().is_empty());
}

/// Typed provider extraction matches the validated payloads.
#[tokio::test]
async fn typed_provider_extraction_round_trips() {
    let capture = oracle();
    let case = capture
        .get("fixtures")
        .unwrap()
        .get("provider_full")
        .unwrap();
    let loaded = load_fixture(case.get("input").unwrap()).await;
    let provider = loaded.get_provider("p").unwrap();
    assert_eq!(provider.name.as_deref(), Some("Provider"));
    assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
    assert_eq!(provider.oauth.as_deref(), Some("radius"));
    assert_eq!(provider.auth_header, Some(true));
    assert_eq!(provider.headers.as_ref().unwrap()[0].0, "x-a");
    let models = provider.models.as_ref().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "full-model");
    assert_eq!(models[0].reasoning, Some(true));
    assert_eq!(models[0].context_window, Some(128_000.0));
    assert_eq!(
        models[0].input.as_ref().unwrap(),
        &vec!["text".to_string(), "image".to_string()]
    );
    let overrides = provider.model_overrides.as_ref().unwrap();
    assert_eq!(overrides[0].0, "o-model");
    assert_eq!(overrides[0].1.reasoning, Some(false));

    let anthropic = capture
        .get("fixtures")
        .unwrap()
        .get("anthropic_compat")
        .unwrap();
    let loaded = load_fixture(anthropic.get("input").unwrap()).await;
    let provider = loaded.get_provider("a").unwrap();
    assert_eq!(provider.api.as_deref(), Some("anthropic-messages"));
    assert!(provider.compat.is_some());
}

/// Error-state configs expose no providers (upstream builds an empty map).
#[tokio::test]
async fn error_configs_expose_no_providers() {
    let capture = oracle();
    let case = capture
        .get("fixtures")
        .unwrap()
        .get("model_missing_id")
        .unwrap();
    let loaded = load_fixture(case.get("input").unwrap()).await;
    assert!(loaded.get_error().is_some());
    assert!(loaded.get_provider_ids().is_empty());
    assert!(loaded.get_provider("p").is_none());
}
