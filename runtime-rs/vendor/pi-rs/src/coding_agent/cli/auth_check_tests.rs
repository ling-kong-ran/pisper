//! Tests for the ported `coding-agent/src/cli/auth-check.ts` (upstream
//! `test/auth-check.test.ts`, ported scenarios listed per test).

use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::ai::auth::credential_store::CredentialStore;
use crate::coding_agent::cli::args::parse_args;
use crate::coding_agent::cli::auth_check::{
    check_provider_auth, create_auth_check_model_runtime, get_provider_credential, AuthCheckReason,
    AuthCheckResult, AuthCheckStatus, GetProviderCredentialOptions,
};
use crate::coding_agent::cli::auth_command::AuthCommandError;
use crate::coding_agent::core::auth_storage::{AuthStorage, ReadOnlyAuthStorage};

fn runtime() -> Runtime {
    Runtime::new().expect("tokio runtime")
}

fn api_key_storage() -> AuthStorage {
    AuthStorage::in_memory(vec![(
        "openai".to_string(),
        serde_json::from_value(serde_json::json!({
            "type": "api_key", "key": "test-key"
        }))
        .unwrap(),
    )])
}

fn create_runtime(
    credentials: impl CredentialStore + 'static,
) -> crate::coding_agent::core::model_runtime::ModelRuntime {
    tokio::runtime::Runtime::new().unwrap().block_on(crate::coding_agent::core::model_runtime::ModelRuntime::create(
        crate::coding_agent::core::model_runtime::CreateModelRuntimeOptions {
            credentials: Some(Arc::new(credentials)),
            models_store: Some(Arc::new(
                crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore::default(),
            )),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        },
    ))
    .expect("runtime create")
}

fn parse(argv: &[&str]) -> crate::coding_agent::cli::args::Args {
    parse_args(&argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
}

/// Upstream: "reports a configured provider as ready".
#[test]
fn reports_a_configured_provider_as_ready() {
    let rt = runtime();
    let model_runtime = create_runtime(api_key_storage());
    let result = rt.block_on(check_provider_auth(
        &parse(&["--provider", "openai"]),
        &model_runtime,
        None,
    ));
    assert_eq!(
        result.unwrap(),
        AuthCheckResult {
            status: AuthCheckStatus::Ready,
            provider: "openai".to_string(),
            reason: None,
            auth_type: Some(crate::ai::auth::types::AuthType::ApiKey),
        }
    );
}

/// Upstream: "reports an unknown provider as not ready".
#[test]
fn reports_an_unknown_provider_as_not_ready() {
    let rt = runtime();
    let model_runtime = create_runtime(AuthStorage::in_memory(Vec::new()));
    let result = rt.block_on(check_provider_auth(
        &parse(&["--provider", "not-installed"]),
        &model_runtime,
        None,
    ));
    assert_eq!(
        result.unwrap(),
        AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider: "not-installed".to_string(),
            reason: Some(AuthCheckReason::ProviderNotFound),
            auth_type: None,
        }
    );
}

/// Upstream: "does not treat an unresolved stored environment reference as
/// configured".
#[test]
fn unresolved_env_reference_is_not_configured() {
    let rt = runtime();
    let temp = tempfile_dir();
    let auth_path = format!("{}/auth.json", temp);
    std::fs::write(
        &auth_path,
        r#"{"openai":{"type":"api_key","key":"$MISSING_AUTH_CHECK_KEY"}}"#,
    )
    .unwrap();
    let model_runtime = create_runtime(ReadOnlyAuthStorage::new(&auth_path));
    let result = rt.block_on(check_provider_auth(
        &parse(&["--provider", "openai"]),
        &model_runtime,
        None,
    ));
    assert_eq!(
        result.unwrap(),
        AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider: "openai".to_string(),
            reason: Some(AuthCheckReason::CredentialsNotConfigured),
            auth_type: None,
        }
    );
    std::fs::remove_file(&auth_path).ok();
}

/// Upstream: "reports malformed auth state as invalid".
#[test]
fn malformed_auth_state_is_invalid() {
    let rt = runtime();
    let temp = tempfile_dir();
    let auth_path = format!("{}/auth.json", temp);
    std::fs::write(&auth_path, "{invalid-json").unwrap();
    let model_runtime = create_runtime(ReadOnlyAuthStorage::new(&auth_path));
    let result = rt.block_on(check_provider_auth(
        &parse(&["--provider", "openai"]),
        &model_runtime,
        None,
    ));
    assert_eq!(
        result.unwrap(),
        AuthCheckResult {
            status: AuthCheckStatus::Invalid,
            provider: "openai".to_string(),
            reason: Some(AuthCheckReason::InvalidState),
            auth_type: None,
        }
    );
    std::fs::remove_file(&auth_path).ok();
}

/// Upstream: "does not create an auth file or its parent directory".
#[test]
fn does_not_create_the_auth_file_or_parent_directory() {
    let rt = runtime();
    let temp = tempfile_dir();
    let auth_path = format!("{}/agent/auth.json", temp);
    let model_runtime = create_runtime(ReadOnlyAuthStorage::new(&auth_path));
    let result = rt.block_on(check_provider_auth(
        &parse(&["--provider", "openai"]),
        &model_runtime,
        None,
    ));
    let result = result.unwrap();
    assert_eq!(result.status, AuthCheckStatus::NotReady);
    assert_eq!(
        result.reason,
        Some(AuthCheckReason::CredentialsNotConfigured)
    );
    assert!(!std::path::Path::new(&auth_path).exists());
    assert!(!std::path::Path::new(&format!("{temp}/agent")).exists());
}

/// Upstream: "reads credentials without refreshing OAuth when requested"
/// (api-key half).
#[test]
fn get_provider_credential_reads_the_stored_key_without_refresh() {
    let rt = runtime();
    let store = api_key_storage();
    let model_runtime = create_runtime(api_key_storage());
    let credential = rt.block_on(get_provider_credential(
        "openai",
        &model_runtime,
        &store,
        GetProviderCredentialOptions { refresh: false },
    ));
    assert_eq!(credential.as_deref(), Some("test-key"));
}

/// Upstream: auth command validation rejects unknown options for the auth
/// commands ("reports unknown auth options like package commands", the parse
/// error string).
#[test]
fn auth_check_requires_provider_or_model() {
    let error = check_provider_auth_parse_error(&[]);
    assert_eq!(
        error,
        AuthCommandError(
            "Auth checks require --provider <provider> or --model <model>".to_string()
        )
    );
}

fn check_provider_auth_parse_error(argv: &[&str]) -> AuthCommandError {
    let rt = runtime();
    let model_runtime = create_runtime(AuthStorage::in_memory(Vec::new()));
    rt.block_on(async {
        let result = check_provider_auth(&parse(argv), &model_runtime, None).await;
        result.expect_err("expected an auth command error")
    })
}

fn tempfile_dir() -> String {
    let dir = std::env::temp_dir().join(format!(
        "pi-rust-auth-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().to_string()
}

/// Upstream: "creates an auth-check runtime without catalog storage".
#[test]
fn creates_an_auth_check_runtime_without_catalog_storage() {
    let rt = runtime();
    let model_runtime = rt
        .block_on(create_auth_check_model_runtime(Arc::new(
            AuthStorage::in_memory(Vec::new()),
        )))
        .unwrap();
    assert!(rt.block_on(model_runtime.get_provider("openai")).is_some());
}
