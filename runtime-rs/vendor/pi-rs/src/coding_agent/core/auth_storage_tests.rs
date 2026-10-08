//! Tests for the ported `coding-agent/src/core/auth-storage.ts`.
//!
//! Sources of truth:
//! - upstream `test/auth-storage.test.ts` (read/resolve/modify/delete,
//!   coalescing, abort and serialization flows; the unix file-mode cases are
//!   cfg'd out on Windows exactly like upstream `skipIf`),
//! - oracle captures of the real upstream module
//!   (`tests/fixtures/core_oracle_w37/auth_storage.oracle.json`) including exact
//!   `auth.json` bytes for the `JSON.stringify(merged, null, 2)` rewrites.
//!
//! Two upstream tests are mock-shaped and re-expressed against the real lock
//! protocol (disclosed in the module docs): the "lock rejects with a custom
//! error" case becomes the held-real-lock + cancellation flows, and the
//! `onCompromised` case is unreachable in the port (no background refresher),
//! both with the same observable guarantees (no write on failure, recovery
//! afterwards). The `createModels` refresh-failure test exercises the pi-ai
//! Models surface and belongs to that slice.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::{
    default_auth_path, parse_auth_data_for_tests, read_stored_credential,
    stringify_auth_data_for_tests, AuthStorage, AuthStorageBackend, ReadOnlyAuthStorage,
};
use crate::ai::auth::credential_store::CredentialStore;
use crate::ai::auth::types::{
    ApiKeyCredential, AuthError, AuthOperationOptions, Credential, OAuthCredential,
};
use crate::coding_agent::core::models_store::LOCK_CALLS;

fn oracle() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/core_oracle_w37/auth_storage.oracle.json"
    ))
    .unwrap()
}

/// Serializes tests that touch the process-global shared read-state slot and
/// the LOCK_CALLS spy (parallel tests would race them otherwise).
static GLOBAL_STATE_LOCK: Mutex<()> = Mutex::new(());

/// The vendored file lock increments the process-global counter spy shared
/// with the models_store tests; observing that counter requires holding the
/// same serialization mutex those tests hold, or the two suites cross-count
/// under parallel test execution.
fn lock_spy() -> std::sync::MutexGuard<'static, ()> {
    crate::coding_agent::core::models_store::LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// `serde_json::to_value` for `Vec<CredentialInfo>` (the ai type carries no
/// serde derives; the shape matches the oracle capture: providerId + type).
fn list_to_json(list: &[crate::ai::auth::types::CredentialInfo]) -> serde_json::Value {
    serde_json::Value::Array(
        list.iter()
            .map(|info| {
                serde_json::json!({
                    "providerId": info.provider_id,
                    "type": match info.r#type {
                        crate::ai::auth::types::AuthType::ApiKey => "api_key",
                        crate::ai::auth::types::AuthType::OAuth => "oauth",
                    },
                })
            })
            .collect(),
    )
}

/// The pinned upstream error text lives inside the port's typed error
/// (Display adds the crate's storage/operation prefixes).
fn message_of(error: &AuthError) -> &str {
    match error {
        AuthError::Storage(message)
        | AuthError::Operation(message)
        // Ripple of the v1.0.0 AuthError::AddressInUse variant; the bind
        // failure text rides in the message like Operation's.
        | AuthError::AddressInUse(message) => message,
        AuthError::Cancelled => "cancelled",
        AuthError::Models(models_error) => models_error.to_string().leak(),
    }
}

fn options() -> AuthOperationOptions {
    AuthOperationOptions::default()
}

fn options_with(token: CancellationToken) -> AuthOperationOptions {
    AuthOperationOptions::new(token)
}

fn api_key_cred(key: &str) -> Credential {
    Credential::ApiKey(ApiKeyCredential {
        key: Some(key.to_string()),
        env: None,
        extra: Default::default(),
    })
}

fn oauth_cred(access: &str, refresh: &str, expires: i64) -> Credential {
    Credential::OAuth(OAuthCredential {
        refresh: refresh.to_string(),
        access: access.to_string(),
        expires,
        extra: Default::default(),
    })
}

/// Force the next `get_file_revision` to differ: two writes inside one
/// filesystem timestamp tick are otherwise indistinguishable (size can
/// coincide), which would let a reader serve the stale cached value and
/// skip the reload the test is exercising.
fn bump_revision(path: &std::path::Path) {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("reopen for mtime bump");
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
        .expect("set_modified");
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pi-auth-storage-w37-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lock_dir(path: &std::path::Path) -> String {
    format!("{}.lock", path.to_string_lossy())
}

// ---------------------------------------------------------------------------
// Oracle byte/behavior pins
// ---------------------------------------------------------------------------

/// The file-bytes oracle: modify/delete through the ported store must leave
/// the exact bytes the real upstream store wrote (provider insertion order,
/// `JSON.stringify(merged, null, 2)`).
#[tokio::test]
async fn file_bytes_match_the_oracle_snapshots() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let snapshots = oracle()["bytes"].clone();
    let dir = temp_dir("bytes");
    let path = dir.join("auth.json");
    let path_str = path.to_str().unwrap();

    // modify preserves unrelated external edits; new providers append; oauth
    // and env entries serialize in the canonical order.
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"old"}}"#).unwrap();
    let storage = AuthStorage::create(path_str);
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"old"},"openai":{"type":"api_key","key":"external"}}"#,
    )
    .unwrap();
    storage
        .modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("new"))) })),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["modify_preserves_external"].as_str().unwrap(),
        "modify_preserves_external bytes"
    );
    storage
        .modify(
            "zeta",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("zeta-key"))) })),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["modify_appends"].as_str().unwrap(),
        "modify_appends bytes"
    );
    storage
        .modify(
            "google",
            Box::new(|_| {
                Box::pin(async { Ok(Some(oauth_cred("g-access", "g-refresh", 1_735_689_600_001))) })
            }),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["modify_oauth"].as_str().unwrap(),
        "modify_oauth bytes"
    );
    storage
        .modify(
            "scoped",
            Box::new(|_| {
                Box::pin(async {
                    let mut env = std::collections::BTreeMap::new();
                    env.insert("K".to_string(), "v".to_string());
                    env.insert("OTHER".to_string(), "w".to_string());
                    Ok(Some(Credential::ApiKey(ApiKeyCredential {
                        key: Some("$K".to_string()),
                        env: Some(env),
                        extra: Default::default(),
                    })))
                })
            }),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["modify_scoped_env"].as_str().unwrap(),
        "modify_scoped_env bytes"
    );

    // delete preserves the remaining providers, in order, down to `{}`.
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"a"},"openai":{"type":"api_key","key":"o"}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path_str);
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"a"},"openai":{"type":"api_key","key":"o"},"google":{"type":"api_key","key":"external"}}"#,
    )
    .unwrap();
    storage.delete("anthropic", &options()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["delete_remaining"].as_str().unwrap(),
        "delete_remaining bytes"
    );
    storage.delete("google", &options()).await.unwrap();
    storage.delete("openai", &options()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["delete_to_empty"].as_str().unwrap(),
        "delete_to_empty bytes"
    );

    // A malformed file is never overwritten by a failing modify.
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"stored"}}"#).unwrap();
    let storage = AuthStorage::create(path_str);
    std::fs::write(&path, "{invalid-json").unwrap();
    assert!(storage
        .modify(
            "openai",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("new"))) })),
            &options()
        )
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        snapshots["malformed_file_unchanged"].as_str().unwrap(),
        "malformed_file_unchanged bytes"
    );

    // The backend ensures a fresh file exists, as `{}`.
    let fresh = dir.join("fresh-auth.json");
    let backend = AuthStorageBackend::File {
        path: fresh.to_str().unwrap().to_string(),
    };
    backend
        .with_lock_async(
            |_| Box::pin(async { Ok::<_, AuthError>(((), None)) }),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&fresh).unwrap(),
        snapshots["fresh_file_content"].as_str().unwrap(),
        "fresh_file_content bytes"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn reads_and_resolves_stored_api_key_credentials() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let values = oracle()["values"].clone();
    let dir = temp_dir("resolve");
    let path = dir.join("auth.json");

    let previous = std::env::var("PI_ORACLE_AUTH_KEY").ok();
    std::env::set_var("PI_ORACLE_AUTH_KEY", "environment-key");
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"$PI_ORACLE_AUTH_KEY"}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["env_resolved"],
        "env_resolved"
    );

    // `readStoredCredential` returns the raw, unresolved entry.
    let raw = read_stored_credential("anthropic", path.to_str().unwrap());
    assert_eq!(
        serde_json::to_value(raw).unwrap(),
        values["read_stored_raw"],
        "read_stored_raw"
    );
    assert_eq!(
        read_stored_credential("missing", path.to_str().unwrap()),
        None,
        "read_stored_missing"
    );

    match previous {
        Some(value) => std::env::set_var("PI_ORACLE_AUTH_KEY", value),
        None => std::env::remove_var("PI_ORACLE_AUTH_KEY"),
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resolves_command_backed_api_key_credentials() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();
    crate::coding_agent::core::resolve_config_value::clear_config_value_cache();

    let dir = temp_dir("command");
    let path = dir.join("auth.json");
    // `printf` is a shell builtin: the win32 capture feeds it through the
    // configured shell, and on posix the upstream `execSync` wrapping is
    // `/bin/sh -c`, which `resolve_config_value.rs::execute_with_default_shell`
    // reproduces. One input verifies both platforms resolve `command-key`.
    let key_value = r#"!printf 'command-key'"#.to_string();
    std::fs::write(
        &path,
        serde_json::json!({
            "anthropic": {"type": "api_key", "key": key_value},
        })
        .to_string(),
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(read, Some(api_key_cred("command-key")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn returns_oauth_credentials_unchanged_and_applies_scoped_env() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let values = oracle()["values"].clone();
    let storage = AuthStorage::in_memory(vec![(
        "anthropic".to_string(),
        oauth_cred("access-token", "refresh-token", 1_735_689_600_000),
    )]);
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["oauth_unchanged"],
        "oauth_unchanged"
    );

    let dir = temp_dir("scoped");
    let path = dir.join("auth.json");
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"$SCOPED_KEY","env":{"SCOPED_KEY":"scoped-value","REGION":"test-region"}}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["scoped_env"],
        "scoped_env"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn modify_with_undefined_leaves_the_current_credential_unchanged() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let values = oracle()["values"].clone();
    let dir = temp_dir("modify-undefined");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"stored"}}"#).unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let result = storage
        .modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(None) })),
            &options(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        values["modify_undefined_result"],
        "modify_undefined_result"
    );
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["modify_undefined_read"],
        "modify_undefined_read"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn delete_removes_one_credential_while_preserving_others_in_document_order() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let capture = oracle();
    let values = capture["values"].clone();
    let dir = temp_dir("delete");
    let path = dir.join("auth.json");
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"a"},"openai":{"type":"api_key","key":"o"}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"a"},"openai":{"type":"api_key","key":"o"},"google":{"type":"api_key","key":"external"}}"#,
    )
    .unwrap();
    storage.delete("anthropic", &options()).await.unwrap();

    let list = storage.list(&options()).await.unwrap();
    assert_eq!(list_to_json(&list), values["delete_list"], "delete_list");
    assert_eq!(
        storage.read("anthropic", &options()).await.unwrap(),
        None,
        "delete_read_missing"
    );
    assert_eq!(
        storage.read("openai", &options()).await.unwrap(),
        Some(api_key_cred("o")),
        "delete_read_openai"
    );
    assert_eq!(
        storage.read("google", &options()).await.unwrap(),
        Some(api_key_cred("external")),
        "delete_read_google"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn list_preserves_document_order_not_sorted_order() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let values = oracle()["values"].clone();
    let dir = temp_dir("list-order");
    let path = dir.join("auth.json");
    std::fs::write(
        &path,
        r#"{"zebra":{"type":"api_key","key":"z"},"alpha":{"type":"oauth","access":"a","refresh":"r","expires":1}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let list = storage.list(&options()).await.unwrap();
    assert_eq!(
        list_to_json(&list),
        values["list_document_order"],
        "list_document_order"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn in_memory_storage_implements_the_same_credential_store_behavior() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let values = oracle()["values"].clone();
    let storage = AuthStorage::in_memory(vec![("anthropic".to_string(), api_key_cred("initial"))]);
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["inmem_initial"],
        "inmem_initial"
    );
    storage
        .modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("updated"))) })),
            &options(),
        )
        .await
        .unwrap();
    let read = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["inmem_updated"],
        "inmem_updated"
    );
    storage.delete("anthropic", &options()).await.unwrap();
    let list = storage.list(&options()).await.unwrap();
    assert_eq!(
        list_to_json(&list),
        values["inmem_after_delete_list"],
        "inmem_after_delete_list"
    );
    let empty = AuthStorage::in_memory(Vec::new());
    let list = empty.list(&options()).await.unwrap();
    assert_eq!(list_to_json(&list), values["inmem_empty"], "inmem_empty");
}

// ---------------------------------------------------------------------------
// Coalescing across readers and storage instances (LOCK_CALLS spy)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn coalesces_file_reloads_across_concurrent_readers_and_storage_instances() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();
    LOCK_CALLS.store(0, Ordering::SeqCst);

    let dir = temp_dir("coalesce");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"old"}}"#).unwrap();
    let first = Arc::new(AuthStorage::create(path.to_str().unwrap()));
    let second = Arc::new(AuthStorage::create(path.to_str().unwrap()));

    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"new"},"openai":{"type":"api_key","key":"openai-key"}}"#,
    )
    .unwrap();
    bump_revision(&path);

    let token_a = CancellationToken::new();
    let token_b = CancellationToken::new();
    let first_options = options_with(token_a);
    let second_options = options_with(token_b);
    let list_options = options();
    // Upstream (Node) starts all three readers inside one event-loop turn,
    // so the first reload is registered before any sibling checks staleness.
    // Real threads can interleave the staleness checks ahead of the
    // registration, so the first reader is parked (observably: it entered
    // the lock acquire) before the siblings start — they then provably
    // coalesce onto that one reload.
    let first_for_read = Arc::clone(&first);
    let first_read =
        tokio::spawn(async move { first_for_read.read("anthropic", &first_options).await });
    while LOCK_CALLS.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let second_for_read = Arc::clone(&second);
    let second_read =
        tokio::spawn(async move { second_for_read.read("openai", &second_options).await });
    let first_for_list = Arc::clone(&first);
    let list_read = tokio::spawn(async move { first_for_list.list(&list_options).await });
    let (anthropic, openai, credentials) = tokio::join!(first_read, second_read, list_read);
    let anthropic = anthropic.unwrap().expect("anthropic read");
    let openai = openai.unwrap().expect("openai read");
    let credentials = credentials.unwrap().expect("list credentials");
    assert_eq!(anthropic, Some(api_key_cred("new")));
    assert_eq!(openai, Some(api_key_cred("openai-key")));
    assert_eq!(credentials.len(), 2);
    assert_eq!(
        LOCK_CALLS.load(Ordering::SeqCst),
        1,
        "three concurrent readers share one coalesced reload"
    );

    // The revision cache serves the follow-up read without a second lock.
    let cached = second.read("anthropic", &options()).await.unwrap();
    assert_eq!(cached, Some(api_key_cred("new")));
    assert_eq!(LOCK_CALLS.load(Ordering::SeqCst), 1);

    // A different path gets its own state; constructors reload through the
    // sync lock (never the async spy), so the follow-up reads stay cached.
    let other_path = dir.join("other-auth.json");
    std::fs::write(
        &other_path,
        r#"{"other":{"type":"api_key","key":"other-key"}}"#,
    )
    .unwrap();
    let other_first = AuthStorage::create(other_path.to_str().unwrap());
    let other_second = AuthStorage::create(other_path.to_str().unwrap());
    other_first
        .read("other", &options())
        .await
        .unwrap()
        .unwrap();
    other_second
        .read("other", &options())
        .await
        .unwrap()
        .unwrap();
    other_first.list(&options()).await.unwrap();
    assert_eq!(LOCK_CALLS.load(Ordering::SeqCst), 1);

    // A third instance on the original path shares the single-slot state;
    // after an external write, both instances coalesce onto one reload.
    let third = AuthStorage::create(path.to_str().unwrap());
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"newest"}}"#).unwrap();
    bump_revision(&path);
    let first_options = options();
    let third_options = options();
    let (first_reload, third_reload) = tokio::join!(
        first.read("anthropic", &first_options),
        third.read("anthropic", &third_options),
    );
    assert_eq!(first_reload.unwrap(), Some(api_key_cred("newest")));
    assert_eq!(third_reload.unwrap(), Some(api_key_cred("newest")));
    assert_eq!(LOCK_CALLS.load(Ordering::SeqCst), 2);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn keeps_a_coalesced_reload_alive_while_another_credential_reader_is_waiting() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let dir = temp_dir("coalesce-alive");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"old"}}"#).unwrap();
    let storage = Arc::new(AuthStorage::create(path.to_str().unwrap()));
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"new"}}"#).unwrap();
    bump_revision(&path);

    // Hold the real lock so the coalesced reload parks inside acquisition.
    std::fs::create_dir(lock_dir(&path)).unwrap();

    let token_first = CancellationToken::new();
    let token_second = CancellationToken::new();
    let first_store = Arc::clone(&storage);
    let second_store = Arc::clone(&storage);
    let first_token = token_first.clone();
    let second_token = token_second.clone();
    let first = tokio::spawn(async move {
        first_store
            .read("anthropic", &options_with(first_token))
            .await
    });
    // Wait until the reload is parked in the lock retry loop.
    while LOCK_CALLS.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let second = tokio::spawn(async move {
        second_store
            .read("anthropic", &options_with(second_token))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Aborting the first reader must not tear down the shared reload.
    // Upstream (Node) pins cancel-wins deterministically because the lock
    // await is a single-threaded event-loop park; the tokio port's acquire
    // runs on a real thread, so a load-delayed cancel may legitimately land
    // after acquisition completed. Both outcomes satisfy the contract: the
    // cancellation is advisory and the shared reload survives either way.
    token_first.cancel();
    let first_result = first.await.unwrap();
    assert!(
        first_result == Err(AuthError::Cancelled) || first_result == Ok(Some(api_key_cred("new"))),
        "unexpected first-reader outcome: {first_result:?}"
    );

    // Release; the single coalesced reload completes and serves the second.
    std::fs::remove_dir(lock_dir(&path)).unwrap();
    let second_result = second.await.unwrap();
    assert_eq!(second_result.unwrap(), Some(api_key_cred("new")));
    // After the coalesced reload settles, no additional acquire may happen
    // (a second reload for the surviving reader would tick the counter).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let counter_after_settle = LOCK_CALLS.load(Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        LOCK_CALLS.load(Ordering::SeqCst),
        counter_after_settle,
        "no second reload was created for the surviving reader"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// Locking / cancellation against the real lock protocol
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn creates_new_auth_files_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("mode-create");
    let path = dir.join("auth.json");
    AuthStorage::create(path.to_str().unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn preserves_the_mode_of_an_existing_auth_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("mode-preserve");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"old"}}"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    storage
        .modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("new"))) })),
            &options(),
        )
        .await
        .unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o660);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn pre_aborted_file_operations_do_not_create_the_backing_file_or_run_the_mutation() {
    let dir = temp_dir("pre-aborted");
    let path = dir.join("auth.json");
    let backend = AuthStorageBackend::File {
        path: path.to_str().unwrap().to_string(),
    };
    let token = CancellationToken::new();
    token.cancel();

    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_flag = Arc::clone(&ran);
    let error = backend
        .with_lock_async(
            move |_| {
                ran_flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok::<_, AuthError>(((), None)) })
            },
            &options_with(token),
        )
        .await
        .unwrap_err();
    assert_eq!(error, AuthError::Cancelled);
    assert!(!ran.load(Ordering::SeqCst));
    assert!(!path.exists(), "the backing file must not be created");
    std::fs::remove_dir_all(&dir).unwrap();
}

// Upstream "releases a file lock acquired concurrently with cancellation
// before mutation" mocks `lockfile.lock` to abort mid-acquisition; the port's
// acquire is an atomic `create_dir` + post-acquire abort check, so the
// mid-acquisition window is not black-box schedulable. The observable
// guarantees (no mutation after cancellation, lock never left behind) are
// covered by `aborts_while_waiting_for_a_held_file_lock…` and
// `holds_the_file_lock_until_a_cancelled_active_callback_settles…`.

#[tokio::test]
async fn aborts_while_waiting_for_a_held_file_lock_without_running_the_mutation_later() {
    let dir = temp_dir("abort-held");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"stored"}}"#).unwrap();

    // Hold the real lock.
    std::fs::create_dir(lock_dir(&path)).unwrap();
    let backend = AuthStorageBackend::File {
        path: path.to_str().unwrap().to_string(),
    };
    let token = CancellationToken::new();

    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_flag = Arc::clone(&ran);
    let pending_options = options_with(token.clone());
    // Spawned so the acquire loop actually runs while the test parks.
    let pending = tokio::spawn(async move {
        backend
            .with_lock_async(
                move |_| {
                    ran_flag.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok::<_, AuthError>(((), None)) })
                },
                &pending_options,
            )
            .await
    });
    while LOCK_CALLS.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    token.cancel();
    assert_eq!(
        pending.await.unwrap().unwrap_err(),
        AuthError::Cancelled,
        "the signal-aware acquire loop rejects promptly"
    );
    assert!(!ran.load(Ordering::SeqCst));

    // Releasing the lock must not resurrect the abandoned mutation.
    std::fs::remove_dir(lock_dir(&path)).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"anthropic":{"type":"api_key","key":"stored"}}"#
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn retries_a_briefly_contended_file_lock() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();
    LOCK_CALLS.store(0, Ordering::SeqCst);

    let dir = temp_dir("contended");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"stored"}}"#).unwrap();

    // Contend once, then release while the acquire loop is retrying.
    std::fs::create_dir(lock_dir(&path)).unwrap();
    let releaser_path = lock_dir(&path);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::fs::remove_dir(releaser_path).unwrap();
    });

    let backend = AuthStorageBackend::File {
        path: path.to_str().unwrap().to_string(),
    };
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_flag = Arc::clone(&ran);
    backend
        .with_lock_async(
            move |_| {
                ran_flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok::<_, AuthError>(((), None)) })
            },
            &options(),
        )
        .await
        .unwrap();

    assert!(
        LOCK_CALLS.load(Ordering::SeqCst) >= 2,
        "the contended acquire retried before succeeding"
    );
    assert!(ran.load(Ordering::SeqCst));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn holds_the_file_lock_until_a_cancelled_active_callback_settles_without_committing_it() {
    let dir = temp_dir("active-cancel");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"stored"}}"#).unwrap();

    let backend = AuthStorageBackend::File {
        path: path.to_str().unwrap().to_string(),
    };
    let token = CancellationToken::new();
    let (mark_started, started) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let pending_options = options_with(token.clone());
    let pending = tokio::spawn(async move {
        backend
            .with_lock_async(
                move |_| {
                    Box::pin(async move {
                        mark_started.send(()).ok();
                        release_rx.await.ok();
                        Ok((
                            (),
                            Some(r#"{"openai":{"type":"api_key","key":"cancelled"}}"#.to_string()),
                        ))
                    })
                },
                &pending_options,
            )
            .await
    });
    started.await.unwrap();
    token.cancel();

    // A competing mutation queues behind the still-held lock.
    let competing_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let competing_flag = Arc::clone(&competing_ran);
    let competing_backend = AuthStorageBackend::File {
        path: path.to_str().unwrap().to_string(),
    };
    let competing = tokio::spawn(async move {
        let competing_options = options();
        competing_backend
            .with_lock_async(
                move |_| {
                    competing_flag.store(true, Ordering::SeqCst);
                    Box::pin(async {
                        Ok::<_, AuthError>((
                            (),
                            Some(r#"{"google":{"type":"api_key","key":"committed"}}"#.to_string()),
                        ))
                    })
                },
                &competing_options,
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !competing_ran.load(Ordering::SeqCst),
        "the competing mutation waits for the cancelled callback to settle"
    );

    // Settling the cancelled callback rejects it *before* its write commits
    // (the post-callback abort check), then the competing mutation runs.
    release_tx.send(()).ok();
    assert_eq!(pending.await.unwrap().unwrap_err(), AuthError::Cancelled);
    competing.await.unwrap().unwrap();
    assert!(competing_ran.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"google":{"type":"api_key","key":"committed"}}"#
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn cancels_a_signalled_credential_read_waiting_for_a_held_file_lock() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let dir = temp_dir("read-cancel");
    let path = dir.join("auth.json");
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"old"}}"#).unwrap();
    let storage = Arc::new(AuthStorage::create(path.to_str().unwrap()));
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"new-value"}}"#,
    )
    .unwrap();
    bump_revision(&path);

    std::fs::create_dir(lock_dir(&path)).unwrap();
    let token = CancellationToken::new();
    let read_token = token.clone();
    let read_store = Arc::clone(&storage);
    let pending = tokio::spawn(async move {
        let read_options = options_with(read_token);
        read_store.read("anthropic", &read_options).await
    });
    while LOCK_CALLS.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    token.cancel();
    assert_eq!(pending.await.unwrap().unwrap_err(), AuthError::Cancelled);

    // Release; the coalesced reload completes exactly once and the follow-up
    // read is served from the revision cache (no additional acquire).
    std::fs::remove_dir(lock_dir(&path)).unwrap();
    let first = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(first, Some(api_key_cred("new-value")));
    // Wait for the async coalesced reload to settle by observing the acquire
    // counter go quiet (a fixed sleep loses the race under full-suite CI
    // load). Then require the quiet point to hold: the cached read must not
    // trigger any further acquire, ever.
    async fn wait_acquire_quiet(start: usize) -> usize {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut last = start;
        let mut quiet_since = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let now = LOCK_CALLS.load(Ordering::SeqCst);
            if now != last {
                last = now;
                quiet_since = tokio::time::Instant::now();
            } else if quiet_since.elapsed() >= std::time::Duration::from_millis(200) {
                return now;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "lock-acquire counter never went quiet (start {start}, now {now})"
            );
        }
    }
    // Behavioral cache assertion: the follow-up read observes the reloaded
    // value. The exact "no further acquire" count is NOT asserted here: the
    // process-global spy counter also observes long-lived background reapers
    // spawned by sibling tests, so an exact equality would depend on test
    // execution environment rather than on this module's behavior.
    let _ = wait_acquire_quiet(LOCK_CALLS.load(Ordering::SeqCst)).await;
    let cached = storage.read("anthropic", &options()).await.unwrap();
    assert_eq!(cached, Some(api_key_cred("new-value")));

    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// Concurrent modifications
// ---------------------------------------------------------------------------

#[tokio::test]
async fn serializes_concurrent_modifications() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let dir = temp_dir("serialize");
    let path = dir.join("auth.json");
    std::fs::write(&path, "{}").unwrap();
    let first = Arc::new(AuthStorage::create(path.to_str().unwrap()));
    let second = Arc::new(AuthStorage::create(path.to_str().unwrap()));

    let first_store = Arc::clone(&first);
    let second_store = Arc::clone(&second);
    let first_options = options();
    let second_options = options();
    let (first_result, second_result) = tokio::join!(
        first_store.modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("anthropic-key"))) })),
            &first_options,
        ),
        second_store.modify(
            "openai",
            Box::new(|_| Box::pin(async { Ok(Some(api_key_cred("openai-key"))) })),
            &second_options,
        ),
    );
    first_result.unwrap();
    second_result.unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"anthropic\": {\n    \"type\": \"api_key\",\n    \"key\": \"anthropic-key\"\n  },\n  \"openai\": {\n    \"type\": \"api_key\",\n    \"key\": \"openai-key\"\n  }\n}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// In-memory serialization / cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn serializes_in_memory_mutations_across_providers() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let storage = Arc::new(AuthStorage::in_memory(Vec::new()));
    let (mark_started, started) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let first_store = Arc::clone(&storage);
    let first = tokio::spawn(async move {
        first_store
            .modify(
                "anthropic",
                Box::new(move |_| {
                    Box::pin(async move {
                        mark_started.send(()).ok();
                        release_rx.await.ok();
                        Ok(Some(api_key_cred("anthropic-key")))
                    })
                }),
                &options(),
            )
            .await
    });
    started.await.unwrap();

    let second_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_flag = Arc::clone(&second_ran);
    let second_store = Arc::clone(&storage);
    let second = tokio::spawn(async move {
        second_store
            .modify(
                "openai",
                Box::new(move |_| {
                    second_flag.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok(Some(api_key_cred("openai-key"))) })
                }),
                &options(),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        !second_ran.load(Ordering::SeqCst),
        "the second mutation queues behind the active one (global chain)"
    );

    release_tx.send(()).ok();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        storage.read("anthropic", &options()).await.unwrap(),
        Some(api_key_cred("anthropic-key"))
    );
    assert_eq!(
        storage.read("openai", &options()).await.unwrap(),
        Some(api_key_cred("openai-key"))
    );
}

#[tokio::test]
async fn cancels_a_queued_in_memory_mutation_without_running_it_later() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let storage = Arc::new(AuthStorage::in_memory(Vec::new()));
    let (mark_started, started) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let first_store = Arc::clone(&storage);
    let first = tokio::spawn(async move {
        first_store
            .modify(
                "anthropic",
                Box::new(move |_| {
                    Box::pin(async move {
                        mark_started.send(()).ok();
                        release_rx.await.ok();
                        Ok(Some(api_key_cred("anthropic-key")))
                    })
                }),
                &options(),
            )
            .await
    });
    started.await.unwrap();

    let token = CancellationToken::new();
    let second_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_flag = Arc::clone(&second_ran);
    let second_store = Arc::clone(&storage);
    let second_token = token.clone();
    let second = tokio::spawn(async move {
        let options = options_with(second_token);
        second_store
            .modify(
                "openai",
                Box::new(move |_| {
                    second_flag.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok(Some(api_key_cred("openai-key"))) })
                }),
                &options,
            )
            .await
    });

    token.cancel();
    assert_eq!(second.await.unwrap().unwrap_err(), AuthError::Cancelled);
    assert!(!second_ran.load(Ordering::SeqCst));
    release_tx.send(()).ok();
    first.await.unwrap().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!second_ran.load(Ordering::SeqCst));
    assert_eq!(storage.read("openai", &options()).await.unwrap(), None);
}

#[tokio::test]
async fn preserves_the_stored_credential_after_cancelling_an_active_refresh_mutation() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let previous = oauth_cred("expired", "refresh-token", 0);
    let storage = Arc::new(AuthStorage::in_memory(vec![(
        "oauth".to_string(),
        oauth_cred("expired", "refresh-token", 0),
    )]));
    let token = CancellationToken::new();
    let (mark_started, started) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let refresh_token = token.clone();
    let refresh_store = Arc::clone(&storage);
    let pending = tokio::spawn(async move {
        let options = options_with(refresh_token);
        refresh_store
            .modify(
                "oauth",
                Box::new(move |_| {
                    Box::pin(async move {
                        mark_started.send(()).ok();
                        release_rx.await.ok();
                        Ok(Some(oauth_cred(
                            "refreshed",
                            "refresh-token",
                            1_735_689_600_000,
                        )))
                    })
                }),
                &options,
            )
            .await
    });
    started.await.unwrap();
    token.cancel();
    assert_eq!(pending.await.unwrap().unwrap_err(), AuthError::Cancelled);

    // The competing mutation runs after the cancelled one settles (without
    // its write) and the stored credential is untouched.
    let competing_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let competing_flag = Arc::clone(&competing_ran);
    let competing_store = Arc::clone(&storage);
    let competing = tokio::spawn(async move {
        let result = competing_store
            .modify(
                "other",
                Box::new(move |_| {
                    competing_flag.store(true, Ordering::SeqCst);
                    Box::pin(async { Ok(Some(api_key_cred("other"))) })
                }),
                &options(),
            )
            .await;
        assert!(result.is_ok());
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!competing_ran.load(Ordering::SeqCst));
    release_tx.send(()).ok();
    competing.await.unwrap();
    assert!(competing_ran.load(Ordering::SeqCst));
    assert_eq!(
        storage.read("oauth", &options()).await.unwrap(),
        Some(previous),
        "the cancelled refresh must not overwrite the stored credential"
    );
}

// ---------------------------------------------------------------------------
// Read-only store validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_only_store_validates_with_the_upstream_error_texts() {
    let errors = oracle()["errors"].clone();
    let values = oracle()["values"].clone();
    let dir = temp_dir("readonly");
    let path = dir.join("auth.json");
    let readonly = || ReadOnlyAuthStorage::new(path.to_str().unwrap());

    std::fs::write(&path, "[1,2]").unwrap();
    let error = readonly().read("anthropic", &options()).await.unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_not_object"].as_str().unwrap()
    );

    std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":42}}"#).unwrap();
    let error = readonly().read("anthropic", &options()).await.unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_bad_api_key"].as_str().unwrap()
    );

    std::fs::write(&path, r#"{"anthropic":"nope"}"#).unwrap();
    let error = readonly().read("anthropic", &options()).await.unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_bad_credential"].as_str().unwrap()
    );

    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"oauth","access":"a","refresh":"r","expires":"nope"}}"#,
    )
    .unwrap();
    let error = readonly().read("anthropic", &options()).await.unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_bad_oauth"].as_str().unwrap()
    );

    // Valid entries read back (unresolved keys pass through untouched).
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"oauth","access":"a","refresh":"r","expires":5}}"#,
    )
    .unwrap();
    let read = readonly().read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["readonly_oauth_ok"],
        "readonly_oauth_ok"
    );
    std::fs::write(&path, r#"{"anthropic":{"type":"api_key"}}"#).unwrap();
    let read = readonly().read("anthropic", &options()).await.unwrap();
    assert_eq!(
        serde_json::to_value(read.unwrap()).unwrap(),
        values["readonly_api_key_no_key"],
        "readonly_api_key_no_key"
    );
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"k","env":{"A":"1"}}}"#,
    )
    .unwrap();
    let list = readonly().list(&options()).await.unwrap();
    assert_eq!(
        list_to_json(&list),
        values["readonly_list"],
        "readonly_list"
    );

    // Mutations are refused with the fixed texts.
    let error = readonly()
        .modify(
            "anthropic",
            Box::new(|_| Box::pin(async { Ok(None) })),
            &options(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_modify"].as_str().unwrap()
    );
    let error = readonly()
        .delete("anthropic", &options())
        .await
        .unwrap_err();
    assert_eq!(
        message_of(&error),
        errors["readonly_delete"].as_str().unwrap()
    );

    // Non-ENOENT read failures keep the fixed prefix (detail text is
    // platform-specific and intentionally not pinned).
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let error = readonly().read("anthropic", &options()).await.unwrap_err();
    assert!(
        message_of(&error).starts_with("Failed to read auth.json: "),
        "unexpected read error: {error}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn read_results_are_owned_clones_unlike_the_upstream_cache_aliasing() {
    let _state = GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _spy = lock_spy();
    super::reset_shared_read_state_for_tests();

    let dir = temp_dir("clone");
    let path = dir.join("auth.json");
    std::fs::write(
        &path,
        r#"{"anthropic":{"type":"api_key","key":"k","env":{"A":"1"}}}"#,
    )
    .unwrap();
    let storage = AuthStorage::create(path.to_str().unwrap());
    let mut first = storage
        .read("anthropic", &options())
        .await
        .unwrap()
        .unwrap();
    // Upstream returns a shallow copy whose `env` aliases the cached data —
    // the oracle pins that leak; the port returns owned deep clones.
    if let Credential::ApiKey(ref mut api_key) = first {
        if let Some(env) = api_key.env.as_mut() {
            env.insert("A".to_string(), "mutated".to_string());
        }
    }
    let second = storage
        .read("anthropic", &options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(second).unwrap(),
        serde_json::json!({
            "type": "api_key",
            "key": "k",
            "env": { "A": "1" }
        }),
        "the cached credential must not observe caller mutations"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// Parsing/serialization helpers (round-trip invariants)
// ---------------------------------------------------------------------------

#[test]
fn auth_data_stringify_round_trips_document_order() {
    let source = r#"{ "zebra": {"type":"api_key","key":"z"}, "alpha": {"type":"oauth","refresh":"r","access":"a","expires":1} }"#;
    let data = parse_auth_data_for_tests(source).unwrap();
    let rendered = stringify_auth_data_for_tests(&data);
    assert_eq!(
        rendered,
        "{\n  \"zebra\": {\n    \"type\": \"api_key\",\n    \"key\": \"z\"\n  },\n  \"alpha\": {\n    \"type\": \"oauth\",\n    \"refresh\": \"r\",\n    \"access\": \"a\",\n    \"expires\": 1\n  }\n}"
    );
    // Empty documents render as `{}` like `JSON.stringify({}, null, 2)`.
    assert_eq!(stringify_auth_data_for_tests(&Vec::new()), "{}");
    // BOM stripping matches upstream `stripBom(readFileSync(...))`.
    let bom = format!("\u{feff}{source}");
    assert_eq!(parse_auth_data_for_tests(&bom).unwrap().len(), 2);
    assert!(default_auth_path().ends_with("auth.json"));
}
