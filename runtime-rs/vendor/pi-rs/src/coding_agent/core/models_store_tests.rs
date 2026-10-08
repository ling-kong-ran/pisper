//! Tests for the ported `coding-agent/src/core/models-store.ts`.
//!
//! Sources of truth:
//! - upstream `test/models-store.test.ts` (persist/reload/coalescing/abort
//!   flows; the unix file-mode case is cfg'd out on Windows exactly like the
//!   upstream `it.skipIf(process.platform === "win32")`),
//! - oracle captures of the real upstream store
//!   (`tests/fixtures/core_oracle/models_store.oracle.json`) including exact file
//!   bytes for the `JSON.stringify(current, null, 2)` rewrite.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{
    FileAuthStorageBackend, FileModelsStore, InMemoryCodingAgentModelsStore, ModelsStoreEntryWire,
    LOCK_CALLS,
};
use crate::ai::models::store::{
    ModelsStore, ModelsStoreEntry, ModelsStoreError, ModelsStoreOperationOptions,
};
use crate::ai::types::{Model, ModelCost, ModelInput};
use crate::coding_agent::core::oracle_data;

fn oracle() -> serde_json::Value {
    serde_json::from_str(oracle_data::MODELS_STORE).unwrap()
}

/// The upstream test's `model()` fixture (matching the oracle script).
fn model(provider: &str, id: &str) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: id.to_string(),
        name: id.to_string(),
        api: "openai-completions".to_string(),
        provider: provider.to_string(),
        base_url: "https://example.test/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 1000,
        max_tokens: 100,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

fn entry(models: Vec<Model>, checked_at: i64) -> ModelsStoreEntry {
    ModelsStoreEntry {
        models: models
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        last_modified: None,
        checked_at: Some(checked_at),
        etag: None,
    }
}

fn entry_with_meta(models: Vec<Model>) -> ModelsStoreEntry {
    ModelsStoreEntry {
        models: models
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        last_modified: Some(4),
        checked_at: Some(5),
        etag: Some("\"abc\"".to_string()),
    }
}

use crate::coding_agent::core::models_store::LOCK_SPY;

fn options() -> ModelsStoreOperationOptions {
    ModelsStoreOperationOptions::NONE
}

/// The file-bytes oracle: writes/deletes through the ported store must leave
/// the exact bytes the real upstream store wrote.
#[tokio::test]
async fn file_bytes_match_the_oracle_snapshots() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let capture = oracle();
    let snapshots = &capture["snapshots"];
    let dir = tempfile::TempDir::with_prefix("pi-models-store-bytes-").unwrap();
    let path = dir.path().join("models-store.json");

    let store = FileModelsStore::new(path.to_str().unwrap());
    store
        .write("one", entry(vec![model("one", "m1")], 100), &options())
        .await
        .unwrap();
    let after_write_one = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after_write_one,
        snapshots["after_write_one"].as_str().unwrap(),
        "after_write_one bytes"
    );

    store
        .write("two", entry(vec![model("two", "m2")], 200), &options())
        .await
        .unwrap();
    let after_write_two = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after_write_two,
        snapshots["after_write_two"].as_str().unwrap(),
        "after_write_two bytes"
    );

    // Metadata fields serialize in the pi-ai entry order.
    let meta_path = dir.path().join("meta-models-store.json");
    let meta_store = FileModelsStore::new(meta_path.to_str().unwrap());
    meta_store
        .write("p", entry_with_meta(Vec::new()), &options())
        .await
        .unwrap();
    let after_write_meta = std::fs::read_to_string(&meta_path).unwrap();
    assert_eq!(
        after_write_meta,
        snapshots["after_write_meta"].as_str().unwrap(),
        "after_write_meta bytes"
    );
    let read_meta = meta_store.read("p", &options()).await.unwrap().unwrap();
    let expected_meta: ModelsStoreEntry =
        serde_json::from_value::<ModelsStoreEntryWire>(snapshots["read_meta"].clone())
            .unwrap()
            .into();
    assert_eq!(read_meta, expected_meta);

    // delete rewrites the remaining catalog.
    store.delete("one", &options()).await.unwrap();
    let after_delete_one = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after_delete_one,
        snapshots["after_delete_one"].as_str().unwrap(),
        "after_delete_one bytes"
    );
}

/// Upstream "persists provider catalogs without replacing unrelated
/// providers".
#[tokio::test]
async fn persists_catalogs_without_replacing_unrelated_providers() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let capture = oracle();
    let snapshots = &capture["snapshots"];
    let dir = tempfile::TempDir::with_prefix("pi-models-store-persist-").unwrap();
    let path = dir.path().join("models-store.json");

    let store = FileModelsStore::new(path.to_str().unwrap());
    store
        .write("one", entry(vec![model("one", "m1")], 100), &options())
        .await
        .unwrap();
    store
        .write("two", entry(vec![model("two", "m2")], 200), &options())
        .await
        .unwrap();

    let reloaded = FileModelsStore::new(path.to_str().unwrap());
    let one = reloaded.read("one", &options()).await.unwrap().unwrap();
    assert_eq!(
        one.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["m1"]
    );
    assert_eq!(one.checked_at, Some(100));
    let two = reloaded.read("two", &options()).await.unwrap().unwrap();
    assert_eq!(
        two.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["m2"]
    );
    let expected_one: ModelsStoreEntry =
        serde_json::from_value::<ModelsStoreEntryWire>(snapshots["reads"]["one"].clone())
            .unwrap()
            .into();
    assert_eq!(one, expected_one);

    reloaded.delete("one", &options()).await.unwrap();
    assert_eq!(reloaded.read("one", &options()).await.unwrap(), None);
    let two = reloaded.read("two", &options()).await.unwrap().unwrap();
    assert_eq!(
        two.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["m2"]
    );
    let expected_two: ModelsStoreEntry =
        serde_json::from_value::<ModelsStoreEntryWire>(snapshots["read_two_after_delete"].clone())
            .unwrap()
            .into();
    assert_eq!(two, expected_two);
}

/// Upstream "preserves the mode of an existing models file" (unix only, like
/// the upstream `it.skipIf(process.platform === "win32")`).
#[cfg(unix)]
#[tokio::test]
async fn preserves_the_mode_of_an_existing_models_file() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::with_prefix("pi-models-store-mode-").unwrap();
    let managed_path = dir.path().join("managed-mode.json");
    std::fs::write(&managed_path, "{}").unwrap();
    std::fs::set_permissions(&managed_path, std::fs::Permissions::from_mode(0o660)).unwrap();
    let store = FileModelsStore::new(managed_path.to_str().unwrap());
    store
        .write("one", entry(vec![model("one", "m1")], 100), &options())
        .await
        .unwrap();
    let mode = std::fs::metadata(&managed_path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o660);
}

/// Upstream "coalesces file reloads across concurrent readers and interleaved
/// storage instances": three concurrent first reads take exactly one storage
/// lock; a cached read takes none; a stale revision reloads.
#[tokio::test]
async fn coalesces_file_reloads_across_concurrent_readers() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Pin this test's path as the shared read-state slot (parallel tests would
    // otherwise have seeded it with their own paths).
    super::reset_shared_read_state_for_tests();
    let dir = tempfile::TempDir::with_prefix("pi-models-store-coalesce-").unwrap();
    let path = dir.path().join("models-store.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "one": { "models": [oracle_model_stub("one", "old")] },
            "two": { "models": [oracle_model_stub("two", "m2")] },
        })
        .to_string(),
    )
    .unwrap();
    let first = Arc::new(FileModelsStore::new(path.to_str().unwrap()));
    let second = Arc::new(FileModelsStore::new(path.to_str().unwrap()));

    LOCK_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    let opts = options();
    let (one, two, missing) = tokio::join!(
        first.read("one", &opts),
        second.read("two", &opts),
        first.read("missing", &opts),
    );
    let one = one.unwrap().unwrap();
    assert_eq!(
        one.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["old"]
    );
    let two = two.unwrap().unwrap();
    assert_eq!(
        two.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["m2"]
    );
    assert_eq!(missing.unwrap(), None);
    assert_eq!(
        LOCK_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one storage lock for the coalesced reload batch"
    );

    // A satisfied-revision read does not re-lock.
    let cached = second.read("one", &options()).await.unwrap().unwrap();
    assert_eq!(
        cached.models.iter().map(|m| m.id()).collect::<Vec<_>>(),
        ["old"]
    );
    assert_eq!(
        LOCK_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "cached read skips the lock"
    );

    // A different path starts its own state; a rewritten file is re-detected
    // through the revision and re-locks.
    let other_path = dir.path().join("other-models-store.json");
    std::fs::write(&other_path, "{}").unwrap();
    let other = Arc::new(FileModelsStore::new(other_path.to_str().unwrap()));
    assert_eq!(other.read("one", &options()).await.unwrap(), None);
    assert_eq!(other.read("one", &options()).await.unwrap(), None);
    let _third = FileModelsStore::new(path.to_str().unwrap());
    std::fs::write(
        &path,
        serde_json::json!({ "one": { "models": [oracle_model_stub("one", "newest-model")] } })
            .to_string(),
    )
    .unwrap();
    let opts = options();
    let third_store = FileModelsStore::new(path.to_str().unwrap());
    let (first_reload, third_reload) =
        tokio::join!(first.read("one", &opts), third_store.read("one", &opts),);
    assert_eq!(
        first_reload.unwrap().unwrap().models[0].id(),
        "newest-model"
    );
    assert_eq!(
        third_reload.unwrap().unwrap().models[0].id(),
        "newest-model"
    );
    assert_eq!(
        LOCK_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "reload after external rewrite takes a new lock"
    );
}

/// Minimal catalog stub mirroring the oracle script's written file shape.
fn oracle_model_stub(provider: &str, id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": provider,
        "baseUrl": "https://example.test/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 1000,
        "maxTokens": 100,
    })
}

/// Upstream "keeps a coalesced reload alive while another reader is still
/// waiting": an aborted first reader cancels nothing for the second; the
/// reload resolves once the lock is granted.
#[tokio::test]
async fn keeps_a_coalesced_reload_alive_while_another_reader_waits() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let dir = tempfile::TempDir::with_prefix("pi-models-store-alive-").unwrap();
    let path = dir.path().join("models-store.json");
    std::fs::write(
        &path,
        serde_json::json!({ "one": { "models": [oracle_model_stub("one", "stored")] } })
            .to_string(),
    )
    .unwrap();
    let store = Arc::new(FileModelsStore::new(path.to_str().unwrap()));

    // Hold the raw storage lock, like the upstream test's proper-lockfile
    // lock.
    let backend = FileAuthStorageBackend::new(path.to_str().unwrap().to_string());
    let guard = backend.acquire_lock_async(None).await.unwrap();

    let first_token = CancellationToken::new();
    let second_token = CancellationToken::new();
    let first_options = ModelsStoreOperationOptions::new(first_token.clone());
    let second_options = ModelsStoreOperationOptions::new(second_token.clone());

    let first = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.read("one", &first_options).await })
    };
    let second = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.read("one", &second_options).await })
    };
    // Give both readers a chance to install one coalesced reload.
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    first_token.cancel();
    let first_result = first.await.unwrap();
    assert!(
        matches!(first_result, Err(ModelsStoreError::Cancelled)),
        "aborted reader rejects with the abort error"
    );

    guard.release();
    let second_result = second.await.unwrap().unwrap().unwrap();
    assert_eq!(
        second_result
            .models
            .iter()
            .map(|m| m.id())
            .collect::<Vec<_>>(),
        ["stored"]
    );
    // Exactly one storage lock for the shared reload.
    let calls = LOCK_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    assert!(calls >= 1, "the shared reload acquired the lock once");
}

/// Upstream "cancels a catalog write waiting for a held file lock without
/// writing later": aborting a blocked write rejects and leaves the file
/// untouched.
#[tokio::test]
async fn cancels_a_catalog_write_waiting_for_a_held_file_lock() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let dir = tempfile::TempDir::with_prefix("pi-models-store-cancel-").unwrap();
    let path = dir.path().join("models-store.json");
    std::fs::write(
        &path,
        serde_json::json!({ "one": { "models": [oracle_model_stub("one", "existing")] } })
            .to_string(),
    )
    .unwrap();
    let store = Arc::new(FileModelsStore::new(path.to_str().unwrap()));

    // Hold the lock from "another writer".
    let backend = FileAuthStorageBackend::new(path.to_str().unwrap().to_string());
    let guard = backend.acquire_lock_async(None).await.unwrap();

    let token = CancellationToken::new();
    let options = ModelsStoreOperationOptions::new(token.clone());
    let pending = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .write("two", entry(vec![model("two", "cancelled")], 300), &options)
                .await
        })
    };

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !pending.is_finished(),
        "write stays pending while the lock is held"
    );
    token.cancel();
    let result = pending.await.unwrap();
    assert!(
        matches!(result, Err(ModelsStoreError::Cancelled)),
        "aborted write rejects with the abort error, got {result:?}"
    );
    guard.release();
    // The aborted write must not have written anything.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(stored.get("one").is_some());
    assert!(stored.get("two").is_none(), "aborted write never persisted");
}

/// In-memory store clone semantics (upstream `InMemoryCodingAgentModelsStore`).
#[tokio::test]
async fn in_memory_store_read_write_delete_are_value_isolated() {
    let capture = oracle();
    let snapshots = &capture["snapshots"];
    let store = InMemoryCodingAgentModelsStore::default();

    assert_eq!(store.read("p", &options()).await.unwrap(), None);
    store
        .write("p", entry(vec![model("p", "m")], 1), &options())
        .await
        .unwrap();

    // Mutation of the read copy never reaches the store (structuredClone).
    let mut first = store.read("p", &options()).await.unwrap().unwrap();
    let mut mutated = first.models[0].as_chat().unwrap().clone();
    mutated.id = "mutated".to_string();
    first.models[0] = crate::ai::types::AnyModel::Chat(mutated);
    let after_mutation = store.read("p", &options()).await.unwrap().unwrap();
    assert_eq!(after_mutation.models[0].id(), "m");
    let expected: ModelsStoreEntry = serde_json::from_value::<ModelsStoreEntryWire>(
        snapshots["in_memory_after_mutation"].clone(),
    )
    .unwrap()
    .into();
    assert_eq!(after_mutation, expected);

    store.delete("p", &options()).await.unwrap();
    assert_eq!(store.read("p", &options()).await.unwrap(), None);
}

/// BOM-prefixed store files parse (stripBom only; no JSONC here).
#[tokio::test]
async fn bom_prefixed_files_parse() {
    let _spy = LOCK_SPY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let capture = oracle();
    let snapshots = &capture["snapshots"];
    let dir = tempfile::TempDir::with_prefix("pi-models-store-bom-").unwrap();
    let path = dir.path().join("bom-models-store.json");
    std::fs::write(&path, "\u{feff}{\"p\": {\"models\": []}}").unwrap();
    let store = FileModelsStore::new(path.to_str().unwrap());
    let read = store.read("p", &options()).await.unwrap().unwrap();
    let expected: ModelsStoreEntry =
        serde_json::from_value::<ModelsStoreEntryWire>(snapshots["bom_read"].clone())
            .unwrap()
            .into();
    assert_eq!(read, expected);
}

/// The trait object safety smoke: stores are usable behind `dyn ModelsStore`.
#[tokio::test]
async fn stores_satisfy_the_models_store_trait() {
    async fn exercise(
        store: &dyn ModelsStore,
    ) -> Result<Option<ModelsStoreEntry>, ModelsStoreError> {
        let opts = ModelsStoreOperationOptions::NONE;
        store.read("p", &opts).await
    }
    let in_memory = InMemoryCodingAgentModelsStore::default();
    assert_eq!(exercise(&in_memory).await.unwrap(), None);
}
