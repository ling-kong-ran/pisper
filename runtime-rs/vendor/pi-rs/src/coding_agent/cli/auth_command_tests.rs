//! Tests for the ported `coding-agent/src/cli/auth-command.ts` — oracle
//! byte-comparisons (parse battery, usage/help text, credential extraction)
//! plus the explicit upstream `credential-print.test.ts` expectations.

use serde_json::{json, Value};

use crate::ai::auth::types::{AuthResult, ModelAuth};
use crate::ai::types::ProviderHeaders;
use crate::coding_agent::cli::auth_command::*;

const ORACLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cli_oracle/oracle.json"
));

fn oracle() -> &'static Value {
    use std::sync::OnceLock;
    static ORACLE_VALUE: OnceLock<Value> = OnceLock::new();
    ORACLE_VALUE.get_or_init(|| serde_json::from_str(ORACLE).expect("oracle json"))
}

fn argv(key: &str) -> Vec<String> {
    serde_json::from_str(key).expect("JSON argv")
}

fn command_to_json(command: &AuthCommand) -> Value {
    let mut out = json!({
        "kind": command.kind.as_str(),
        "args": command.args,
        "json": command.json,
        "credentials": command.credentials,
        "noRefresh": command.no_refresh,
    });
    if let Some(min_expiry_ms) = command.min_expiry_ms {
        out["minExpiryMs"] = json!(min_expiry_ms);
    }
    out
}

#[test]
fn parse_auth_command_matches_the_node_oracle_byte_for_byte() {
    let cases = oracle()["authCommand"].as_object().unwrap();
    for (key, expected) in cases {
        if !key.starts_with('[') {
            continue;
        }
        let actual = match parse_auth_command(&argv(key)) {
            Ok(Some(command)) => command_to_json(&command),
            Ok(None) => json!("undefined"),
            Err(error) => json!({ "error": error.0 }),
        };
        assert_eq!(actual, *expected, "parseAuthCommand mismatch for {key}");
    }
}

#[test]
fn usage_and_names_match_the_oracle() {
    let oracle = oracle()["authCommand"].as_object().unwrap();
    let usage = oracle["usage"].as_object().unwrap();
    for (key, expected) in usage {
        let kind = match key.as_str() {
            "check" => AuthCommandKind::Check,
            "api_key" => AuthCommandKind::ApiKey,
            "bearer_token" => AuthCommandKind::BearerToken,
            other => panic!("unknown oracle kind {other}"),
        };
        assert_eq!(get_auth_command_usage(kind), expected.as_str().unwrap());
    }
    let names = oracle["name"].as_object().unwrap();
    for (key, expected) in names {
        let kind = match key.as_str() {
            "check" => AuthCommandKind::Check,
            "api_key" => AuthCommandKind::ApiKey,
            "bearer_token" => AuthCommandKind::BearerToken,
            other => panic!("unknown oracle kind {other}"),
        };
        assert_eq!(get_auth_command_name(kind), expected.as_str().unwrap());
    }
}

#[test]
fn is_auth_command_help_matches_the_oracle() {
    let is_help = oracle()["authCommand"]["isHelp"].as_object().unwrap();
    let expected = |key: &str| is_help[key].as_bool().unwrap();
    assert_eq!(
        is_auth_command_help(&["auth".to_string()]),
        expected("bare")
    );
    assert_eq!(
        is_auth_command_help(&["auth".to_string(), "help".to_string()]),
        expected("helpWord")
    );
    assert_eq!(
        is_auth_command_help(&[
            "auth".to_string(),
            "print-api-key".to_string(),
            "--help".to_string()
        ]),
        expected("helpFlag")
    );
    assert_eq!(
        is_auth_command_help(&[
            "auth".to_string(),
            "print-bearer-token".to_string(),
            "-h".to_string()
        ]),
        expected("hFlag")
    );
    assert_eq!(
        is_auth_command_help(&[
            "auth".to_string(),
            "check".to_string(),
            "--help".to_string()
        ]),
        expected("checkHelp")
    );
    assert_eq!(
        is_auth_command_help(&["notauth".to_string()]),
        expected("nonAuth")
    );
}

fn auth_with_headers(headers: Option<ProviderHeaders>) -> AuthResult {
    AuthResult {
        auth: ModelAuth {
            api_key: None,
            headers,
            base_url: None,
        },
        ..AuthResult::default()
    }
}

#[test]
fn get_auth_credential_matches_the_oracle() {
    let expected = oracle()["authCommand"]["credential"].as_object().unwrap();

    let mut api_key = AuthResult::default();
    api_key.auth.api_key = Some("sk-1".to_string());
    assert_eq!(
        get_auth_credential(Some(&api_key)).as_deref(),
        expected["apiKey"].as_str()
    );

    let mut headers = ProviderHeaders::default();
    headers.insert(
        "Authorization".to_string(),
        Some("Bearer tok-1".to_string()),
    );
    assert_eq!(
        get_auth_credential(Some(&auth_with_headers(Some(headers)))).as_deref(),
        expected["bearerHeader"].as_str()
    );

    let mut lower_headers = ProviderHeaders::default();
    lower_headers.insert(
        "authorization".to_string(),
        Some("bearer  spaced tok".to_string()),
    );
    assert_eq!(
        get_auth_credential(Some(&auth_with_headers(Some(lower_headers)))).as_deref(),
        expected["bearerLower"].as_str()
    );

    assert_eq!(
        get_auth_credential(Some(&auth_with_headers(None))),
        None,
        "no headers -> no credential (oracle key dropped as undefined)"
    );
    assert_eq!(
        get_auth_credential(None),
        None,
        "undefined auth -> undefined credential"
    );
}

/// Upstream credential-print.test.ts: `auth check` help text (oracle byte
/// comparison).
#[test]
fn print_auth_command_help_matches_the_oracle() {
    assert_eq!(
        format!(
            "{}
",
            print_auth_command_help()
        ),
        oracle()["help"]["auth"].as_str().unwrap()
    );
}
