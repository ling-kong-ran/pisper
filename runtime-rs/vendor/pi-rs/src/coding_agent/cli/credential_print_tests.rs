//! Tests for the ported `coding-agent/src/cli/credential-print.ts` (upstream
//! `test/credential-print.test.ts`, the scenarios reachable over the ported
//! runtime surface; the parse-side expectations live in the oracle tests).

use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::ai::auth::credential_store::CredentialStore;
use crate::coding_agent::cli::args::parse_args;
use crate::coding_agent::cli::auth_command::AuthCommandError;
use crate::coding_agent::cli::credential_print::{
    resolve_credential_for_print, CredentialPrintKind,
};
use crate::coding_agent::core::auth_storage::AuthStorage;
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;

fn api_key_storage() -> AuthStorage {
    AuthStorage::in_memory(vec![(
        "openai".to_string(),
        serde_json::from_value(serde_json::json!({
            "type": "api_key", "key": "test-api-key"
        }))
        .unwrap(),
    )])
}

fn oauth_storage() -> AuthStorage {
    AuthStorage::in_memory(vec![(
        "openai-codex".to_string(),
        serde_json::from_value(serde_json::json!({
            "type": "oauth",
            "access": "test-token-not-to-be-printed",
            "refresh": "test-refresh-token",
            "expires": 9_000_000_000_000i64
        }))
        .unwrap(),
    )])
}

fn create_runtime(credentials: AuthStorage) -> ModelRuntime {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(credentials)),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            ..CreateModelRuntimeOptions::default()
        }))
        .unwrap()
}

fn parse(argv: &[&str]) -> crate::coding_agent::cli::args::Args {
    parse_args(&argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
}

/// Upstream: "prints a resolved API key".
#[test]
fn prints_a_resolved_api_key() {
    let rt = Runtime::new().unwrap();
    let runtime = create_runtime(api_key_storage());
    let args = parse(&["--provider", "openai"]);
    let credential = rt.block_on(resolve_credential_for_print(
        &args,
        &runtime,
        CredentialPrintKind::ApiKey,
        None,
        None,
    ));
    assert_eq!(credential.unwrap(), "test-api-key");
}

/// Upstream: "rejects invalid arguments or credential types" — missing
/// --provider/--model.
#[test]
fn requires_provider_or_model() {
    let rt = Runtime::new().unwrap();
    let runtime = create_runtime(oauth_storage());
    let args = parse(&[]);
    let error = rt
        .block_on(resolve_credential_for_print(
            &args,
            &runtime,
            CredentialPrintKind::ApiKey,
            None,
            None,
        ))
        .err()
        .unwrap();
    assert_eq!(
        error,
        AuthCommandError(
            "Credential printing requires --provider <provider> or --model <model>".to_string()
        )
    );
}

/// Upstream: "rejects invalid arguments or credential types" — an
/// OAuth-configured provider is rejected for `print-api-key`.
#[test]
fn api_key_kind_rejects_oauth_configured_providers() {
    let rt = Runtime::new().unwrap();
    let runtime = create_runtime(oauth_storage());
    let args = parse(&["--provider", "openai-codex"]);
    let error = rt
        .block_on(resolve_credential_for_print(
            &args,
            &runtime,
            CredentialPrintKind::ApiKey,
            None,
            None,
        ))
        .err()
        .unwrap();
    assert!(
        error.0.contains("configured with OAuth"),
        "unexpected error: {error}"
    );
}

/// The api-key path never surfaces an OAuth access token even without an
/// explicit --provider rejection (the kind filter skips oauth entries).
#[test]
fn api_key_kind_does_not_print_the_oauth_token() {
    let rt = Runtime::new().unwrap();
    let runtime = create_runtime(oauth_storage());
    let args = parse(&["--provider", "openai-codex"]);
    let credential = rt.block_on(resolve_credential_for_print(
        &args,
        &runtime,
        CredentialPrintKind::ApiKey,
        None,
        None,
    ));
    assert!(credential.is_err());
}

/// Unknown providers are rejected before any resolution (oracle-adjacent
/// error string from credential-print.ts).
#[test]
fn unknown_provider_is_rejected() {
    let rt = Runtime::new().unwrap();
    let runtime = create_runtime(api_key_storage());
    let args = parse(&["--provider", "not-installed"]);
    let error = rt
        .block_on(resolve_credential_for_print(
            &args,
            &runtime,
            CredentialPrintKind::ApiKey,
            None,
            None,
        ))
        .err()
        .unwrap();
    assert_eq!(
        error,
        AuthCommandError(
            "Unknown provider \"not-installed\". Use --list-models to see available providers."
                .to_string()
        )
    );
}

/// The unused-import guard for `CredentialStore` (kept for the storage type
/// the runtime consumes).
#[allow(dead_code)]
fn _store_is_credential_store(_: Arc<dyn CredentialStore>) {}
