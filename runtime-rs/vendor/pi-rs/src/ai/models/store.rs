//! Upstream `packages/ai/src/models-store.ts` in full: the
//! [`ModelsStoreEntry`] shape, the [`ModelsStore`] trait, and the default
//! [`InMemoryModelsStore`]. Persistent model catalogs keyed by provider ID —
//! the storage dynamic providers' overlays are restored from and published
//! to by the `Models` refresh ([`super::Models::refresh`]).
//!
//! Upstream store promises reject with arbitrary errors; the port types the
//! channel as [`ModelsStoreError`] (cancellation is separate, never a
//! message). `structuredClone` on the read/write paths becomes ownership:
//! entries handed to [`ModelsStore::write`] are moved in and reads return
//! fresh clones, so no caller ever aliases store state.

use std::collections::BTreeMap;
use std::sync::RwLock;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::ai::types::AnyModel;

/// Upstream `ModelsStoreEntry` (models-store.ts:3-14): one provider's
/// persisted catalog plus the remote-validation metadata its fetcher keeps
/// alongside it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelsStoreEntry {
    pub models: Vec<AnyModel>,
    /// Unix timestamp from the remote catalog's Last-Modified header.
    pub last_modified: Option<i64>,
    /// Unix timestamp of the last completed remote check.
    pub checked_at: Option<i64>,
    /// Opaque validator from the remote catalog's ETag header, stored
    /// verbatim (quotes included) and echoed back as If-None-Match.
    pub etag: Option<String>,
}

/// Upstream `ModelsStoreOperationOptions` (models-store.ts:16-18): optional
/// cancellation for store operations. `CancellationToken` replaces the
/// upstream `AbortSignal` (see `AuthOperationOptions` for the same shape on
/// the auth surface).
#[derive(Debug, Clone, Default)]
pub struct ModelsStoreOperationOptions {
    pub signal: Option<CancellationToken>,
}

impl ModelsStoreOperationOptions {
    /// No cancellation, the common case for direct callers.
    pub const NONE: Self = Self { signal: None };

    pub fn new(signal: CancellationToken) -> Self {
        Self {
            signal: Some(signal),
        }
    }

    /// Upstream `signal?.throwIfAborted()`: `Err` once the token has fired.
    pub fn check(&self) -> Result<(), ModelsStoreError> {
        if self
            .signal
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        {
            Err(ModelsStoreError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Port error channel for [`ModelsStore`] operations. Upstream store promises
/// reject with arbitrary errors (surfaced raw in the refresh result); the
/// port separates cancellation — which the refresh machinery never records —
/// from storage failure messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsStoreError {
    /// The operation's cancellation token fired (upstream `AbortError`).
    Cancelled,
    /// Storage failure inside a store implementation.
    Storage(String),
}

impl std::fmt::Display for ModelsStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelsStoreError::Cancelled => write!(f, "models store operation cancelled"),
            ModelsStoreError::Storage(message) => write!(f, "models store failure: {message}"),
        }
    }
}

impl std::error::Error for ModelsStoreError {}

/// Upstream `ModelsStore` (models-store.ts:21-25): persistent model catalogs
/// keyed by provider ID. `read` resolves `Ok(None)` for missing entries;
/// methods error only on storage failure or cancellation.
pub trait ModelsStore: Send + Sync {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>>;

    fn write<'a>(
        &'a self,
        provider_id: &'a str,
        entry: ModelsStoreEntry,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>>;

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>>;
}

/// Default in-memory models store (upstream `InMemoryModelsStore`,
/// models-store.ts:27-45). Apps inject persistent stores.
#[derive(Default)]
pub struct InMemoryModelsStore {
    entries: RwLock<BTreeMap<String, ModelsStoreEntry>>,
}

impl ModelsStore for InMemoryModelsStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            let entries = self
                .entries
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(entries.get(provider_id).cloned())
        })
    }

    fn write<'a>(
        &'a self,
        provider_id: &'a str,
        entry: ModelsStoreEntry,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            let mut entries = self
                .entries
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.insert(provider_id.to_string(), entry);
            Ok(())
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            let mut entries = self
                .entries
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.remove(provider_id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::Model;

    fn test_model(provider: &str, id: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: id.to_string(),
            api: "test-api".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.test/v1".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![crate::ai::types::ModelInput::Text],
            cost: crate::ai::types::ModelCost::default(),
            context_window: 10_000,
            max_tokens: 1000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    fn entry(provider: &str, id: &str) -> ModelsStoreEntry {
        ModelsStoreEntry {
            models: vec![AnyModel::Chat(test_model(provider, id))],
            last_modified: Some(1),
            checked_at: Some(2),
            etag: Some("\"e\"".to_string()),
        }
    }

    /// Upstream models-store semantics: write stores an independent copy,
    /// read hands back an independent copy, delete removes.
    #[tokio::test]
    async fn in_memory_store_read_write_delete_are_value_isolated() {
        let store = InMemoryModelsStore::default();
        let options = ModelsStoreOperationOptions::NONE;

        assert_eq!(store.read("p1", &options).await.unwrap(), None);

        let written = entry("p1", "m1");
        store.write("p1", written, &options).await.unwrap();
        let mut read_back = store.read("p1", &options).await.unwrap().unwrap();
        assert_eq!(read_back, entry("p1", "m1"));

        // Mutating the read copy never reaches the store...
        read_back.models[0] = crate::ai::types::AnyModel::Chat(Model {
            id: "mutated".to_string(),
            ..read_back.models[0].as_chat().unwrap().clone()
        });
        assert_eq!(
            store.read("p1", &options).await.unwrap().unwrap(),
            entry("p1", "m1")
        );

        // ...and the store's default is `None` for absent optional metadata
        // only when it was written that way.
        store
            .write(
                "p2",
                ModelsStoreEntry {
                    models: vec![AnyModel::Chat(test_model("p2", "m2"))],
                    ..ModelsStoreEntry::default()
                },
                &options,
            )
            .await
            .unwrap();
        let bare = store.read("p2", &options).await.unwrap().unwrap();
        assert_eq!(bare.last_modified, None);
        assert_eq!(bare.checked_at, None);
        assert_eq!(bare.etag, None);

        store.delete("p1", &options).await.unwrap();
        assert_eq!(store.read("p1", &options).await.unwrap(), None);
        // Deleting a missing entry is a no-op.
        store.delete("never-there", &options).await.unwrap();
    }

    /// Upstream `signal?.throwIfAborted()` at each operation's entry.
    #[tokio::test]
    async fn store_operations_reject_when_cancelled_at_entry() {
        let store = InMemoryModelsStore::default();
        let token = CancellationToken::new();
        let options = ModelsStoreOperationOptions::new(token.clone());
        token.cancel();
        assert_eq!(
            store.read("p1", &options).await,
            Err(ModelsStoreError::Cancelled)
        );
        assert_eq!(
            store
                .write("p1", ModelsStoreEntry::default(), &options)
                .await,
            Err(ModelsStoreError::Cancelled)
        );
        assert_eq!(
            store.delete("p1", &options).await,
            Err(ModelsStoreError::Cancelled)
        );
        assert_eq!(
            ModelsStoreError::Cancelled.to_string(),
            "models store operation cancelled"
        );
    }
}
