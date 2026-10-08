use crate::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore};
use crate::ai::auth::types::{AuthOperationOptions, AuthType, Credential, CredentialInfo};
use crate::coding_agent::core::runtime_credentials::RuntimeCredentials;
use std::sync::Arc;

fn credentials() -> Arc<RuntimeCredentials> {
    Arc::new(RuntimeCredentials::new(Arc::new(
        InMemoryCredentialStore::default(),
    )))
}

#[tokio::test]
async fn read_prefers_the_runtime_override() {
    let credentials = credentials();
    credentials
        .modify(
            "p",
            Box::new(|_| Box::pin(async { Ok(Some(serde_wire_key("stored-key"))) })),
            &AuthOperationOptions::NONE,
        )
        .await
        .unwrap();
    assert_eq!(
        credentials
            .read("p", &AuthOperationOptions::NONE)
            .await
            .unwrap(),
        Some(serde_wire_key("stored-key"))
    );

    credentials.set_runtime_api_key("p", "runtime-key");
    assert_eq!(
        credentials
            .read("p", &AuthOperationOptions::NONE)
            .await
            .unwrap(),
        Some(serde_wire_key("runtime-key"))
    );

    // Providers without overrides still read the backing store.
    assert_eq!(
        credentials
            .read("other", &AuthOperationOptions::NONE)
            .await
            .unwrap(),
        None
    );
}

fn serde_wire_key(key: &str) -> Credential {
    Credential::ApiKey(crate::ai::auth::types::ApiKeyCredential {
        key: Some(key.to_string()),
        env: None,
        extra: Default::default(),
    })
}

#[tokio::test]
async fn list_merges_overrides_over_stored_entries() {
    let credentials = credentials();
    for provider in ["a", "b"] {
        credentials
            .modify(
                provider,
                Box::new(|_| Box::pin(async { Ok(Some(serde_wire_key("k"))) })),
                &AuthOperationOptions::NONE,
            )
            .await
            .unwrap();
    }
    credentials.set_runtime_api_key("b", "runtime");
    credentials.set_runtime_api_key("c", "runtime");

    let entries: Vec<CredentialInfo> = credentials.list(&AuthOperationOptions::NONE).await.unwrap();
    let rendered: Vec<(String, String)> = entries
        .into_iter()
        .map(|entry| {
            (
                entry.provider_id,
                if entry.r#type == AuthType::ApiKey {
                    "api_key".to_string()
                } else {
                    "oauth".to_string()
                },
            )
        })
        .collect();
    // Upstream Map merge: stored positions keep their order, new overrides
    // append, overridden entries keep their position.
    assert_eq!(
        rendered,
        vec![
            ("a".to_string(), "api_key".to_string()),
            ("b".to_string(), "api_key".to_string()),
            ("c".to_string(), "api_key".to_string()),
        ]
    );
}

#[tokio::test]
async fn remove_runtime_api_key_and_has_checks() {
    let credentials = credentials();
    assert!(!credentials.has_runtime_api_key("p"));
    credentials.set_runtime_api_key("p", "key");
    assert!(credentials.has_runtime_api_key("p"));
    credentials.remove_runtime_api_key("p");
    assert!(!credentials.has_runtime_api_key("p"));

    // delete clears the override too (upstream delete overrides.delete).
    credentials.set_runtime_api_key("q", "key");
    credentials
        .delete("q", &AuthOperationOptions::NONE)
        .await
        .unwrap();
    assert!(!credentials.has_runtime_api_key("q"));
}

#[tokio::test]
async fn modify_passes_through_to_the_backing_store() {
    let credentials = credentials();
    credentials.set_runtime_api_key("p", "runtime-key");
    credentials
        .modify(
            "p",
            Box::new(|_| Box::pin(async { Ok(Some(serde_wire_key("stored-key"))) })),
            &AuthOperationOptions::NONE,
        )
        .await
        .unwrap();
    // The override still shadows the stored value for reads…
    assert_eq!(
        credentials
            .read("p", &AuthOperationOptions::NONE)
            .await
            .unwrap(),
        Some(serde_wire_key("runtime-key"))
    );
    // …but the backing store received the write.
    credentials.remove_runtime_api_key("p");
    assert_eq!(
        credentials
            .read("p", &AuthOperationOptions::NONE)
            .await
            .unwrap(),
        Some(serde_wire_key("stored-key"))
    );
}
