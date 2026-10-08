//! Credential storage ported from upstream
//! `packages/ai/src/auth/credential-store.ts`: the app-owned [`CredentialStore`]
//! trait and the default [`InMemoryCredentialStore`].
//!
//! Storage is keyed by `Provider.id`, one credential per provider. `modify`
//! is the only write path, so every mutation is a serialized read-modify-write:
//! the callback sees the current credential, and returning `None` leaves the
//! entry unchanged. Upstream serializes writers per provider id through a
//! promise chain; here each provider has a `tokio::sync::Mutex` (one tiny
//! allocation per provider that ever received a write — bounded by the
//! provider-id cardinality, never cleaned up, like upstream's chain map which
//! deletes settled tails only opportunistically).
//!
//! Cancellation ([`AuthOperationOptions`], upstream `AbortSignal`): `read`
//! and `list` check the token at entry only; `modify` and `delete` race the
//! token against lock acquisition and against the callback, and `modify`
//! re-checks after the callback returns but before applying the write, so an
//! operation cancelled at any point never writes. Unlike upstream's
//! `raceWithAbortSignal` — which rejects the caller early while the abandoned
//! work keeps running — Rust cancellation drops the callback future at the
//! await point; the no-write-after-cancel guarantee is the same.

use futures::future::BoxFuture;
use tokio::sync::{Mutex, RwLock};

use super::types::{AuthError, AuthOperationOptions, Credential, CredentialInfo};

/// Upstream `modify` callback (types.ts:88): sees the current credential and
/// returns the new one, or `None` to leave the entry unchanged. `Send +
/// 'static` so [`CredentialStore`] stays object-safe (`dyn`), the way apps
/// inject stores into `Models`.
pub type ModifyCallback = Box<
    dyn FnOnce(Option<Credential>) -> BoxFuture<'static, Result<Option<Credential>, AuthError>>
        + Send,
>;

/// Upstream `CredentialStore` (types.ts:65-94): app-owned credential storage,
/// keyed by `Provider.id`, one credential per provider. Login/logout
/// orchestration is app-owned.
///
/// Error semantics: `read` resolves `Ok(None)` for missing entries; methods
/// error only on storage failure or cancellation.
pub trait CredentialStore: Send + Sync {
    /// Read the stored credential, possibly expired. Display/status use;
    /// resolved request auth comes from `Models.get_auth()`.
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>>;

    /// List stored credential metadata without resolving or exposing secrets.
    /// Implementations must not execute configured API-key commands while
    /// listing. Order follows insertion (upstream `Map` iteration order).
    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>>;

    /// Serialized write — the only write path. `f` sees the current credential
    /// because correct writes (refresh, login-during-refresh) depend on it;
    /// return the new credential, or `None` to leave the entry unchanged.
    /// Mutual exclusion is per provider id. Resolves with the post-write
    /// credential (`None` when the entry is untouched and absent). Errors
    /// from `f` propagate and leave the entry unchanged.
    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>>;

    /// Remove a credential (logout). Serialized against `modify` per provider.
    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>>;
}

/// Default in-memory credential store (upstream `InMemoryCredentialStore`).
/// Apps inject persistent stores. Insertion order is preserved, mirroring the
/// upstream JS `Map` (an upsert keeps the original position).
#[derive(Default)]
pub struct InMemoryCredentialStore {
    credentials: RwLock<Vec<(String, Credential)>>,
    locks: Mutex<std::collections::HashMap<String, std::sync::Arc<Mutex<()>>>>,
}

fn find_entry<'a>(
    entries: &'a [(String, Credential)],
    provider_id: &str,
) -> Option<&'a Credential> {
    entries
        .iter()
        .find(|(id, _)| id == provider_id)
        .map(|(_, credential)| credential)
}

fn upsert(entries: &mut Vec<(String, Credential)>, provider_id: &str, credential: Credential) {
    if let Some(slot) = entries.iter_mut().find(|(id, _)| id == provider_id) {
        slot.1 = credential;
    } else {
        entries.push((provider_id.to_string(), credential));
    }
}

impl InMemoryCredentialStore {
    /// Acquire this provider's writer lock, racing the operation's
    /// cancellation token (upstream `signal.throwIfAborted()` when a queued
    /// task is dequeued). An already-cancelled token rejects without waiting
    /// for the lock, deterministically.
    async fn lock_provider(
        &self,
        provider_id: &str,
        options: &AuthOperationOptions,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, AuthError> {
        options.check()?;
        let entry = {
            let mut locks = self.locks.lock().await;
            std::sync::Arc::clone(locks.entry(provider_id.to_string()).or_default())
        };
        tokio::select! {
            guard = Mutex::lock_owned(entry) => Ok(guard),
            _ = options.cancelled() => Err(AuthError::Cancelled),
        }
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let credentials = self.credentials.read().await;
            Ok(find_entry(&credentials, provider_id).cloned())
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let credentials = self.credentials.read().await;
            Ok(credentials
                .iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id: provider_id.clone(),
                    r#type: credential.auth_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            let _guard = self.lock_provider(provider_id, options).await?;
            let current = {
                let credentials = self.credentials.read().await;
                find_entry(&credentials, provider_id).cloned()
            };
            let next = {
                let input = current.clone();
                tokio::select! {
                    result = f(input) => result?,
                    _ = options.cancelled() => return Err(AuthError::Cancelled),
                }
            };
            // Upstream re-checks the signal after the callback so an abort
            // during the callback never applies its write.
            options.check()?;
            if let Some(next) = &next {
                let mut credentials = self.credentials.write().await;
                upsert(&mut credentials, provider_id, next.clone());
            }
            Ok(next.or(current))
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        Box::pin(async move {
            let _guard = self.lock_provider(provider_id, options).await?;
            let mut credentials = self.credentials.write().await;
            credentials.retain(|(id, _)| id != provider_id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::AuthType;
    use std::sync::Arc;

    fn api_key(key: &str) -> Credential {
        Credential::ApiKey(super::super::types::ApiKeyCredential {
            key: Some(key.to_string()),
            env: None,
            extra: Default::default(),
        })
    }

    fn oauth(access: &str, refresh: &str, expires: i64) -> Credential {
        Credential::OAuth(super::super::types::OAuthCredential {
            refresh: refresh.to_string(),
            access: access.to_string(),
            expires,
            extra: Default::default(),
        })
    }

    fn no_options() -> AuthOperationOptions {
        AuthOperationOptions::default()
    }

    fn const_callback(result: Result<Option<Credential>, AuthError>) -> ModifyCallback {
        Box::new(move |_| Box::pin(async move { result }))
    }

    #[tokio::test]
    async fn read_resolves_none_for_missing_entries() {
        let store = InMemoryCredentialStore::default();
        assert_eq!(store.read("p1", &no_options()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn modify_stores_and_sees_the_current_credential() {
        let store = InMemoryCredentialStore::default();
        let stored = store
            .modify(
                "p1",
                Box::new(|current| {
                    assert_eq!(current, None);
                    Box::pin(async { Ok(Some(api_key("first"))) })
                }),
                &no_options(),
            )
            .await
            .unwrap();
        assert_eq!(stored, Some(api_key("first")));

        let stored = store
            .modify(
                "p1",
                Box::new(|current| {
                    assert_eq!(current, Some(api_key("first")));
                    Box::pin(async { Ok(Some(oauth("a", "r", 10))) })
                }),
                &no_options(),
            )
            .await
            .unwrap();
        assert_eq!(stored, Some(oauth("a", "r", 10)));
        assert_eq!(
            store.read("p1", &no_options()).await.unwrap(),
            Some(oauth("a", "r", 10))
        );
    }

    #[tokio::test]
    async fn modify_returning_none_leaves_the_entry_unchanged() {
        let store = InMemoryCredentialStore::default();
        // Empty entry: None in, None out, nothing stored.
        let stored = store
            .modify("p1", const_callback(Ok(None)), &no_options())
            .await
            .unwrap();
        assert_eq!(stored, None);
        assert_eq!(store.read("p1", &no_options()).await.unwrap(), None);

        // Existing entry: None keeps it, and the old credential is returned.
        store
            .modify(
                "p1",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("kept"))) })),
                &no_options(),
            )
            .await
            .unwrap();
        let stored = store
            .modify("p1", const_callback(Ok(None)), &no_options())
            .await
            .unwrap();
        assert_eq!(stored, Some(api_key("kept")));
        assert_eq!(
            store.read("p1", &no_options()).await.unwrap(),
            Some(api_key("kept"))
        );
    }

    #[tokio::test]
    async fn modify_callback_errors_propagate_and_leave_the_entry_unchanged() {
        let store = InMemoryCredentialStore::default();
        store
            .modify(
                "p1",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("old"))) })),
                &no_options(),
            )
            .await
            .unwrap();
        let error = store
            .modify(
                "p1",
                const_callback(Err(AuthError::Operation("refresh failed".to_string()))),
                &no_options(),
            )
            .await
            .unwrap_err();
        assert_eq!(error, AuthError::Operation("refresh failed".to_string()));
        assert_eq!(
            store.read("p1", &no_options()).await.unwrap(),
            Some(api_key("old"))
        );
    }

    #[tokio::test]
    async fn delete_removes_and_is_ok_for_missing_entries() {
        let store = InMemoryCredentialStore::default();
        store
            .modify(
                "p1",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("k"))) })),
                &no_options(),
            )
            .await
            .unwrap();
        store.delete("p1", &no_options()).await.unwrap();
        assert_eq!(store.read("p1", &no_options()).await.unwrap(), None);
        store.delete("p1", &no_options()).await.unwrap();
        store.delete("never-there", &no_options()).await.unwrap();
    }

    #[tokio::test]
    async fn list_reports_metadata_in_insertion_order_without_secrets() {
        let store = InMemoryCredentialStore::default();
        store
            .modify(
                "api-provider",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("secret"))) })),
                &no_options(),
            )
            .await
            .unwrap();
        store
            .modify(
                "oauth-provider",
                Box::new(|_| Box::pin(async { Ok(Some(oauth("access", "refresh", 10))) })),
                &no_options(),
            )
            .await
            .unwrap();
        // Insertion order (not alphabetical: oauth-provider < zzz sorts after,
        // so add one that proves ordering is insertion-based).
        store
            .modify(
                "aaa-provider",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("k"))) })),
                &no_options(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.list(&no_options()).await.unwrap(),
            vec![
                CredentialInfo {
                    provider_id: "api-provider".to_string(),
                    r#type: AuthType::ApiKey,
                },
                CredentialInfo {
                    provider_id: "oauth-provider".to_string(),
                    r#type: AuthType::OAuth,
                },
                CredentialInfo {
                    provider_id: "aaa-provider".to_string(),
                    r#type: AuthType::ApiKey,
                },
            ]
        );
    }

    #[tokio::test]
    async fn modify_serializes_per_provider_and_the_queued_write_sees_the_result() {
        let store = Arc::new(InMemoryCredentialStore::default());
        let (release_first, first_wait) = tokio::sync::oneshot::channel::<()>();
        let (first_started, first_started_rx) = tokio::sync::oneshot::channel::<()>();
        let first_store = store.clone();
        let first = tokio::spawn(async move {
            first_store
                .modify(
                    "p1",
                    Box::new(move |current| {
                        assert_eq!(current, None);
                        Box::pin(async move {
                            first_started.send(()).ok();
                            first_wait.await.ok();
                            Ok(Some(api_key("first")))
                        })
                    }),
                    &no_options(),
                )
                .await
        });
        first_started_rx.await.unwrap();

        let second_store = store.clone();
        let second = tokio::spawn(async move {
            second_store
                .modify(
                    "p1",
                    Box::new(|current| {
                        // Queued behind the first write; must see its result.
                        assert_eq!(current, Some(api_key("first")));
                        Box::pin(async { Ok(Some(api_key("second"))) })
                    }),
                    &no_options(),
                )
                .await
        });

        release_first.send(()).ok();
        assert_eq!(first.await.unwrap().unwrap(), Some(api_key("first")));
        assert_eq!(second.await.unwrap().unwrap(), Some(api_key("second")));
        assert_eq!(
            store.read("p1", &no_options()).await.unwrap(),
            Some(api_key("second"))
        );
    }

    #[tokio::test]
    async fn modifies_on_different_providers_do_not_serialize_against_each_other() {
        let store = Arc::new(InMemoryCredentialStore::default());
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let (started, started_rx) = tokio::sync::oneshot::channel::<()>();
        let p1_store = store.clone();
        let p1 = tokio::spawn(async move {
            p1_store
                .modify(
                    "p1",
                    Box::new(move |_| {
                        Box::pin(async move {
                            started.send(()).ok();
                            wait.await.ok();
                            Ok(Some(api_key("p1")))
                        })
                    }),
                    &no_options(),
                )
                .await
        });
        started_rx.await.unwrap();
        // p2's write completes while p1's callback is still parked.
        let p2 = store
            .modify(
                "p2",
                Box::new(|_| Box::pin(async { Ok(Some(api_key("p2"))) })),
                &no_options(),
            )
            .await;
        assert_eq!(p2.unwrap(), Some(api_key("p2")));
        release.send(()).ok();
        p1.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn queued_modification_cancelled_before_dequeue_never_runs() {
        // Port of upstream models-runtime "cancels queued credential
        // mutations without running them later".
        let store = Arc::new(InMemoryCredentialStore::default());
        let (release_first, first_wait) = tokio::sync::oneshot::channel::<()>();
        let (first_started, first_started_rx) = tokio::sync::oneshot::channel::<()>();
        let first_store = store.clone();
        let first = tokio::spawn(async move {
            first_store
                .modify(
                    "p1",
                    Box::new(move |_| {
                        Box::pin(async move {
                            first_started.send(()).ok();
                            first_wait.await.ok();
                            Ok(Some(api_key("first")))
                        })
                    }),
                    &no_options(),
                )
                .await
        });
        first_started_rx.await.unwrap();

        let second_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let token = tokio_util::sync::CancellationToken::new();
        let second_ran_flag = second_ran.clone();
        let second_token = token.clone();
        let second_store = store.clone();
        let second = tokio::spawn(async move {
            let options = AuthOperationOptions::new(second_token);
            second_store
                .modify(
                    "p1",
                    Box::new(move |_| {
                        second_ran_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        Box::pin(async { Ok(Some(api_key("second"))) })
                    }),
                    &options,
                )
                .await
        });

        token.cancel();
        let error = second.await.unwrap().unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        release_first.send(()).ok();
        assert_eq!(first.await.unwrap().unwrap(), Some(api_key("first")));
        tokio::task::yield_now().await;
        assert!(!second_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            store.read("p1", &no_options()).await.unwrap(),
            Some(api_key("first"))
        );
    }

    #[tokio::test]
    async fn read_list_and_queued_delete_reject_when_cancelled() {
        let store = InMemoryCredentialStore::default();
        let token = tokio_util::sync::CancellationToken::new();
        let options = AuthOperationOptions::new(token.clone());
        token.cancel();
        assert_eq!(store.read("p1", &options).await, Err(AuthError::Cancelled));
        assert_eq!(store.list(&options).await, Err(AuthError::Cancelled));
        assert_eq!(
            store.delete("p1", &options).await,
            Err(AuthError::Cancelled)
        );
    }

    #[tokio::test]
    async fn modify_cancelled_while_the_callback_runs_never_writes() {
        let store = Arc::new(InMemoryCredentialStore::default());
        let token = tokio_util::sync::CancellationToken::new();
        let (callback_started, callback_started_rx) = tokio::sync::oneshot::channel::<()>();
        let task_store = store.clone();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            let options = AuthOperationOptions::new(task_token);
            task_store
                .modify(
                    "p1",
                    Box::new(move |_| {
                        Box::pin(async move {
                            callback_started.send(()).ok();
                            // Park until the test cancels the token, which
                            // drops this future at the await point.
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                            Ok(Some(api_key("late")))
                        })
                    }),
                    &options,
                )
                .await
        });
        callback_started_rx.await.unwrap();
        token.cancel();
        assert_eq!(task.await.unwrap(), Err(AuthError::Cancelled));
        assert_eq!(store.read("p1", &no_options()).await.unwrap(), None);
    }
}
