//! Port of upstream `coding-agent/src/core/runtime-credentials.ts` (vendored
//! into the W3.6 slice as the direct dependency of
//! [`super::model_runtime`]): the async credential-store overlay for
//! non-persistent runtime API keys.
//!
//! Reads, listing, and deletes consult the override map first/last exactly
//! like upstream; `modify` passes through to the backing store (upstream
//! semantics — runtime keys are visible through `read`/`list` but a stored
//! write always targets the underlying file).

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::ai::auth::credential_store::{CredentialStore, ModifyCallback};
use crate::ai::auth::types::{
    ApiKeyCredential, AuthError, AuthOperationOptions, Credential, CredentialInfo,
};

/// Upstream `RuntimeCredentials implements CredentialStore`.
pub struct RuntimeCredentials {
    store: Arc<dyn CredentialStore>,
    /// Upstream `Map<string, string>` overrides (insertion order kept for
    /// `list`).
    overrides: Mutex<Vec<(String, String)>>,
}

impl RuntimeCredentials {
    /// Upstream `new RuntimeCredentials(store)`.
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            overrides: Mutex::new(Vec::new()),
        }
    }

    /// Upstream `setRuntimeApiKey`.
    pub fn set_runtime_api_key(&self, provider_id: &str, api_key: &str) {
        let mut overrides = self
            .overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match overrides.iter_mut().find(|(id, _)| id == provider_id) {
            Some(entry) => entry.1 = api_key.to_string(),
            None => overrides.push((provider_id.to_string(), api_key.to_string())),
        }
    }

    /// Upstream `removeRuntimeApiKey`.
    pub fn remove_runtime_api_key(&self, provider_id: &str) {
        self.overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| id != provider_id);
    }

    /// Upstream `hasRuntimeApiKey`.
    pub fn has_runtime_api_key(&self, provider_id: &str) -> bool {
        self.overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|(id, _)| id == provider_id)
    }
}

impl CredentialStore for RuntimeCredentials {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let override_key = self
                .overrides
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, key)| key.clone());
            if let Some(key) = override_key {
                // Upstream: `{ type: "api_key", key: override }`.
                return Ok(Some(Credential::ApiKey(ApiKeyCredential {
                    key: Some(key),
                    env: None,
                    extra: Default::default(),
                })));
            }
            self.store.read(provider_id, options).await
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            let stored = self.store.list(options).await?;
            options.check()?;
            let overrides = self
                .overrides
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Upstream: seed a Map from the stored entries, then set one
            // `{ providerId, type: "api_key" }` entry per override.
            let mut entries: Vec<(String, CredentialInfo)> = stored
                .into_iter()
                .map(|entry| (entry.provider_id.clone(), entry))
                .collect();
            for (provider_id, _) in overrides.iter() {
                let entry = CredentialInfo {
                    provider_id: provider_id.clone(),
                    r#type: crate::ai::auth::types::AuthType::ApiKey,
                };
                match entries.iter_mut().find(|(id, _)| id == provider_id) {
                    Some(slot) => slot.1 = entry,
                    None => entries.push((provider_id.clone(), entry)),
                }
            }
            Ok(entries.into_iter().map(|(_, entry)| entry).collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        self.store.modify(provider_id, f, options)
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        Box::pin(async move {
            options.check()?;
            self.store.delete(provider_id, options).await?;
            self.remove_runtime_api_key(provider_id);
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "runtime_credentials_tests.rs"]
mod tests;
