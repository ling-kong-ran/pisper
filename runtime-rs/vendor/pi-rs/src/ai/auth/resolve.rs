//! Auth resolution ported from upstream `packages/ai/src/auth/resolve.ts`:
//! [`ModelsError`], [`AuthResolutionOverrides`], and
//! [`resolve_provider_auth`] — the precedence engine shared by the `Models`
//! and `ImagesModels` collections (M2e).
//!
//! Precedence contract (types.ts + the pi-ai README "How Auth Resolves"): a
//! stored credential owns the provider — ambient env is consulted only when
//! nothing is stored, and a failed refresh NEVER falls back to env (the
//! credential is preserved for re-login). Failures are [`ModelsError`] with
//! code `"oauth"` (refresh/derivation failed; credential preserved) or
//! `"auth"` (key resolution or credential store failure). Cancellation
//! surfaces as [`AuthError::Cancelled`], never as a `ModelsError`: upstream
//! aborts reject with a raw `AbortError` and the outer
//! `raceWithAbortSignal` lets that win over any internal wrap.
//!
//! Structural deviations, all documented at the site:
//! - upstream `resolveProviderAuth(provider: { id, auth }, ...)` takes
//!   `provider_id` and [`ProviderAuth`] separately (no `Provider` type exists
//!   in this crate layer);
//! - the 15s refresh timeout composes as a `tokio::select!` arm around
//!   `OAuthAuth::refresh` instead of `AbortSignal.any` + `AbortSignal.timeout`
//!   (dropping the refresh future is the Rust abort), with the same
//!   observable error;
//! - the `ModelsError` thrown by the refresh callback travels through the
//!   store's error channel as [`AuthError::Models`] so the
//!   "re-wrap other store failures" distinction survives.

use std::sync::Arc;

use futures::future::BoxFuture;

use super::credential_store::{CredentialStore, ModifyCallback};
use super::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthOperationOptions,
    AuthResult, Credential, OAuthAuth, OAuthCredential, ProviderAuth,
};
use crate::ai::{now_ms, types::ProviderEnv};

// Upstream resolve.ts re-exports `ModelsError` from `utils/models-error.ts`
// (the unified-catalog delta moved it there); every existing import path
// keeps working.
pub use crate::ai::models_error::{ModelsError, ModelsErrorCode};

/// Upstream `AuthResolutionOverrides` (resolve.ts:18-24): per-call overrides
/// for [`resolve_provider_auth`].
#[derive(Debug, Clone, Default)]
pub struct AuthResolutionOverrides {
    /// Explicit key — wins over anything the provider would resolve.
    pub api_key: Option<String>,
    /// Provider-scoped env overlay: consulted before the base auth context.
    pub env: Option<ProviderEnv>,
    /// Require this much remaining OAuth-token validity; defaults to five
    /// minutes (the default window triggers a refresh but does not impose a
    /// provider contract after the refresh — only explicit callers do).
    pub min_oauth_validity_ms: Option<i64>,
    /// Caller cancellation (upstream `signal?: AbortSignal`).
    pub signal: Option<tokio_util::sync::CancellationToken>,
}

/// Upstream `DEFAULT_OAUTH_MINIMUM_VALIDITY_MS` (resolve.ts:119).
pub const DEFAULT_OAUTH_MINIMUM_VALIDITY_MS: i64 = 5 * 60 * 1000;

/// Upstream `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS` (resolve.ts:120).
pub const DEFAULT_OAUTH_REFRESH_TIMEOUT_MS: u64 = 15_000;

/// Upstream `resolveProviderAuth` (resolve.ts:50-61): auth resolution shared
/// by the `Models` and `ImagesModels` collections. A stored credential owns
/// the provider: ambient/env is consulted only when nothing is stored. No
/// silent env fallback after a failed refresh or for a credential type
/// without a matching handler.
///
/// Upstream passes `provider: { id: string; auth: ProviderAuth }`; the port
/// takes the id and the [`ProviderAuth`] separately. The whole operation is
/// raced against the overrides' cancellation token (upstream
/// `raceWithAbortSignal`): an abort wins over any result, and the abandoned
/// work is dropped at its await point.
pub fn resolve_provider_auth<'a>(
    provider_id: &'a str,
    auth: &'a ProviderAuth,
    credentials: &'a dyn CredentialStore,
    auth_context: &'a dyn AuthContext,
    overrides: Option<&'a AuthResolutionOverrides>,
) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
    Box::pin(async move {
        let options = AuthOperationOptions {
            signal: overrides.and_then(|overrides| overrides.signal.clone()),
        };
        options.check()?;
        let cancelled = options.cancelled();
        tokio::select! {
            result = resolve_with_signal(provider_id, auth, credentials, auth_context, overrides, &options) => result,
            _ = cancelled => Err(AuthError::Cancelled),
        }
    })
}

/// Upstream `resolveProviderAuthWithSignal` (resolve.ts:63-110).
async fn resolve_with_signal(
    provider_id: &str,
    auth: &ProviderAuth,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    overrides: Option<&AuthResolutionOverrides>,
    options: &AuthOperationOptions,
) -> Result<Option<AuthResult>, AuthError> {
    options.check()?;
    // overrides.env overlays the base auth context (resolve.ts:71,112-117).
    let overlay = overrides
        .and_then(|overrides| overrides.env.as_ref())
        .map(|env| OverlayEnvAuthContext {
            base: auth_context,
            env,
        });
    let request_auth_context: &dyn AuthContext = match &overlay {
        Some(overlay) => overlay,
        None => auth_context,
    };

    // Explicit option apiKey wins (resolve.ts:73-85). Upstream ignores it when
    // the provider has no api-key auth handler and falls through to storage.
    if let (Some(api_key_override), Some(api_key_auth)) = (
        overrides.and_then(|overrides| overrides.api_key.clone()),
        auth.api_key.as_deref(),
    ) {
        let credential = ApiKeyCredential {
            key: Some(api_key_override),
            env: overrides.and_then(|overrides| overrides.env.clone()),
            extra: Default::default(),
        };
        return resolve_api_key(
            request_auth_context,
            api_key_auth,
            provider_id,
            Some(&credential),
            options,
        )
        .await;
    }

    let stored = read_credential(credentials, provider_id, options).await?;
    if let Some(stored) = stored {
        return match stored {
            Credential::OAuth(stored) => match auth.oauth.clone() {
                Some(oauth) => {
                    resolve_stored_oauth(
                        credentials,
                        provider_id,
                        oauth,
                        stored,
                        options,
                        overrides.and_then(|overrides| overrides.min_oauth_validity_ms),
                    )
                    .await
                }
                // Credential type without a matching handler: no env fallback.
                None => Ok(None),
            },
            Credential::ApiKey(mut stored) => match auth.api_key.as_deref() {
                Some(api_key_auth) => {
                    if let Some(overrides_env) =
                        overrides.and_then(|overrides| overrides.env.as_ref())
                    {
                        // `{ ...stored.env, ...overrides.env }` (resolve.ts:100).
                        let env = stored.env.get_or_insert_with(Default::default);
                        for (name, value) in overrides_env {
                            env.insert(name.clone(), value.clone());
                        }
                    }
                    resolve_api_key(
                        request_auth_context,
                        api_key_auth,
                        provider_id,
                        Some(&stored),
                        options,
                    )
                    .await
                }
                // Credential type without a matching handler: no env fallback.
                None => Ok(None),
            },
        };
    }

    // Ambient (env vars, AWS profiles, ADC files) — resolve.ts:106-109.
    match auth.api_key.as_deref() {
        Some(api_key_auth) => {
            resolve_api_key(
                request_auth_context,
                api_key_auth,
                provider_id,
                None,
                options,
            )
            .await
        }
        None => Ok(None),
    }
}

/// Upstream `overlayEnvAuthContext` (resolve.ts:112-117): the override env is
/// consulted first; an empty override value falls through to the base (the
/// upstream `env[name] || (await base.env(name))` chain). File existence
/// delegates to the base context.
struct OverlayEnvAuthContext<'a> {
    base: &'a dyn AuthContext,
    env: &'a ProviderEnv,
}

impl AuthContext for OverlayEnvAuthContext<'_> {
    fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            match self.env.get(name) {
                Some(value) if !value.is_empty() => Some(value.clone()),
                _ => self.base.env(name).await,
            }
        })
    }

    fn file_exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool> {
        self.base.file_exists(path)
    }
}

/// Upstream `resolveStoredOAuth` (resolve.ts:127-179): OAuth resolution with
/// double-checked locking — tokens with less than five minutes remaining
/// lock, re-check expiry under the lock, refresh once globally, and persist
/// the rotated credential before release.
async fn resolve_stored_oauth(
    credentials: &dyn CredentialStore,
    provider_id: &str,
    oauth: Arc<dyn OAuthAuth>,
    stored: OAuthCredential,
    options: &AuthOperationOptions,
    min_oauth_validity_ms: Option<i64>,
) -> Result<Option<AuthResult>, AuthError> {
    let minimum_validity_ms =
        DEFAULT_OAUTH_MINIMUM_VALIDITY_MS.max(min_oauth_validity_ms.unwrap_or(0));
    let expires_soon =
        |credential: &OAuthCredential| now_ms() + minimum_validity_ms >= credential.expires;
    let mut credential = stored;

    if expires_soon(&credential) {
        // Optimistic check said expired; the authoritative check runs under
        // the lock (resolve.ts:139-172).
        let oauth_for_callback = Arc::clone(&oauth);
        let options_for_callback = options.clone();
        let provider_id_for_callback = provider_id.to_string();
        let callback: ModifyCallback = Box::new(move |current| {
            Box::pin(async move {
                let current = match current {
                    // Logged out meanwhile, or the entry changed type.
                    Some(Credential::OAuth(current)) => current,
                    _ => return Ok(None),
                };
                // Another process/request refreshed.
                if now_ms() + minimum_validity_ms < current.expires {
                    return Ok(None);
                }
                refresh_under_lock(
                    oauth_for_callback,
                    provider_id_for_callback,
                    current,
                    options_for_callback,
                )
                .await
            })
        });
        let post = match credentials.modify(provider_id, callback, options).await {
            Ok(post) => post,
            Err(AuthError::Cancelled) => return Err(AuthError::Cancelled),
            // The refresh callback's typed error propagates unchanged
            // (upstream `if (error instanceof ModelsError) throw error`).
            Err(AuthError::Models(error)) => return Err(AuthError::Models(error)),
            Err(error) => {
                return Err(AuthError::Models(ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("Credential store modify failed for {provider_id}"),
                    error,
                )))
            }
        };
        // Logged out meanwhile (resolve.ts:164).
        credential = match post {
            Some(Credential::OAuth(post)) => post,
            _ => return Ok(None),
        };
        // The normal five-minute window triggers a refresh but does not
        // impose a provider contract. Explicit callers (such as bearer-token
        // export) do require the requested minimum after the refresh
        // (resolve.ts:166-171).
        if min_oauth_validity_ms.is_some() && expires_soon(&credential) {
            return Err(AuthError::Models(ModelsError::new(
                ModelsErrorCode::OAuth,
                format!("OAuth refresh returned a token that expires too soon for {provider_id}"),
            )));
        }
    }

    match oauth.to_auth(credential).await {
        Ok(auth) => Ok(Some(AuthResult {
            auth,
            env: None,
            source: Some("OAuth".to_string()),
        })),
        Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
        Err(error) => Err(AuthError::Models(ModelsError::with_cause(
            ModelsErrorCode::OAuth,
            format!("OAuth auth derivation failed for {provider_id}"),
            error,
        ))),
    }
}

/// The refresh step of the locked callback (resolve.ts:148-156): race the
/// flow's refresh against the operation token and the 15s timeout, and wrap
/// every failure in a `ModelsError("oauth", ...)` so the store propagates the
/// typed error while other failures stay re-wrappable.
async fn refresh_under_lock(
    oauth: Arc<dyn OAuthAuth>,
    provider_id: String,
    credential: OAuthCredential,
    options: AuthOperationOptions,
) -> Result<Option<Credential>, AuthError> {
    let refresh = oauth.refresh(credential, &options);
    tokio::select! {
        result = refresh => match result {
            Ok(refreshed) => Ok(Some(Credential::OAuth(refreshed))),
            Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
            Err(error) => Err(AuthError::Models(ModelsError::with_cause(
                ModelsErrorCode::OAuth,
                format!("OAuth refresh failed for {provider_id}"),
                error,
            ))),
        },
        _ = options.cancelled() => Err(AuthError::Cancelled),
        // Upstream composes `AbortSignal.timeout(15s)` into the refresh
        // signal; the abort rejects the refresh with a `TimeoutError`
        // DOMException whose message (`AbortSignal.timeout`) the wrap below
        // reproduces.
        _ = tokio::time::sleep(std::time::Duration::from_millis(
            DEFAULT_OAUTH_REFRESH_TIMEOUT_MS,
        )) => Err(AuthError::Models(ModelsError::with_cause(
            ModelsErrorCode::OAuth,
            format!("OAuth refresh failed for {provider_id}"),
            "The operation was aborted due to timeout",
        ))),
    }
}

/// Upstream `resolveApiKey` (resolve.ts:181-193).
async fn resolve_api_key(
    auth_context: &dyn AuthContext,
    api_key: &dyn ApiKeyAuth,
    provider_id: &str,
    credential: Option<&ApiKeyCredential>,
    options: &AuthOperationOptions,
) -> Result<Option<AuthResult>, AuthError> {
    let input = ApiKeyAuthInput {
        ctx: auth_context,
        credential,
        options,
    };
    match api_key.resolve(input).await {
        Ok(result) => Ok(result),
        Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
        Err(error) => Err(AuthError::Models(ModelsError::with_cause(
            ModelsErrorCode::Auth,
            format!("API key auth failed for provider {provider_id}"),
            error,
        ))),
    }
}

/// Upstream `readCredential` (resolve.ts:195-205). Also the credential-read
/// wrap the refresh paths share: the models collection's
/// `read_refresh_credential` delegates here, so store-read failures carry the
/// same code-`"auth"` `ModelsError` on both flows.
pub(crate) async fn read_credential(
    credentials: &dyn CredentialStore,
    provider_id: &str,
    options: &AuthOperationOptions,
) -> Result<Option<Credential>, AuthError> {
    match credentials.read(provider_id, options).await {
        Ok(stored) => Ok(stored),
        Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
        Err(error) => Err(AuthError::Models(ModelsError::with_cause(
            ModelsErrorCode::Auth,
            format!("Credential store read failed for {provider_id}"),
            error,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    use super::*;
    use crate::ai::auth::credential_store::{InMemoryCredentialStore, ModifyCallback};
    use crate::ai::auth::helpers::env_api_key_auth;
    use crate::ai::auth::types::{CredentialInfo, ModelAuth, ProviderAuthInteraction};

    const PROVIDER: &str = "fake-provider";
    const ENV_VAR: &str = "FAKE_PROVIDER_API_KEY";

    // ---------------------------------------------------------------- fakes

    /// In-memory env lookup — avoids process-env global state (upstream tests
    /// inject `{ env: async () => ... }` contexts).
    struct MapAuthContext {
        vars: BTreeMap<String, String>,
    }

    impl MapAuthContext {
        fn new(vars: &[(&str, &str)]) -> Self {
            MapAuthContext {
                vars: vars
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
            }
        }
    }

    impl AuthContext for MapAuthContext {
        fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
            Box::pin(async move { self.vars.get(name).cloned() })
        }

        fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async { false })
        }
    }

    /// Api-key auth that always fails resolution (upstream throwing resolvers).
    struct FailingApiKeyAuth {
        name: String,
    }

    impl ApiKeyAuth for FailingApiKeyAuth {
        fn name(&self) -> &str {
            &self.name
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async { Err(AuthError::Operation("boom".to_string())) })
        }
    }

    /// OAuth stub: `to_auth` derives `apiKey` from the access token; the
    /// refresh behavior is configured per test and call-counted.
    struct FakeOAuth {
        name: String,
        to_auth_error: Option<String>,
        to_auth_base_url_from_access: bool,
        refresh_result: Option<Result<OAuthCredential, String>>,
        refresh_delay_ms: u64,
        refresh_hangs: bool,
        refresh_calls: AtomicUsize,
    }

    impl FakeOAuth {
        fn no_refresh() -> Self {
            FakeOAuth {
                name: "Fake OAuth".to_string(),
                to_auth_error: None,
                to_auth_base_url_from_access: false,
                refresh_result: None,
                refresh_delay_ms: 0,
                refresh_hangs: false,
                refresh_calls: AtomicUsize::new(0),
            }
        }

        fn refreshing(credential: OAuthCredential) -> Self {
            FakeOAuth {
                refresh_result: Some(Ok(credential)),
                ..FakeOAuth::no_refresh()
            }
        }

        fn calls(&self) -> usize {
            self.refresh_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl OAuthAuth for FakeOAuth {
        fn name(&self) -> &str {
            &self.name
        }

        fn login<'a>(
            &'a self,
            _interaction: ProviderAuthInteraction,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            Box::pin(async { Err(AuthError::Operation("no login in this test".to_string())) })
        }

        fn to_auth<'a>(
            &'a self,
            credential: OAuthCredential,
        ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
            Box::pin(async move {
                if let Some(error) = &self.to_auth_error {
                    return Err(AuthError::Operation(error.clone()));
                }
                let base_url = if self.to_auth_base_url_from_access
                    && credential
                        .access
                        .contains("proxy-ep=proxy.business.githubcopilot.com")
                {
                    Some("https://api.business.githubcopilot.com".to_string())
                } else {
                    None
                };
                Ok(ModelAuth {
                    api_key: Some(credential.access),
                    headers: None,
                    base_url,
                })
            })
        }

        fn refresh<'a>(
            &'a self,
            _credential: OAuthCredential,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            Box::pin(async move {
                self.refresh_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if self.refresh_hangs {
                    std::future::pending::<()>().await;
                }
                if self.refresh_delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(self.refresh_delay_ms))
                        .await;
                }
                match &self.refresh_result {
                    Some(Ok(credential)) => Ok(credential.clone()),
                    Some(Err(message)) => Err(AuthError::Operation(message.clone())),
                    None => Err(AuthError::Operation("no refresh configured".to_string())),
                }
            })
        }
    }

    // -------------------------------------------------------------- helpers

    fn api_key_credential(key: &str) -> Credential {
        Credential::ApiKey(ApiKeyCredential {
            key: Some(key.to_string()),
            env: None,
            extra: Default::default(),
        })
    }

    fn oauth_credential(access: &str, expires_in_ms: i64) -> Credential {
        Credential::OAuth(OAuthCredential {
            refresh: "refresh-token".to_string(),
            access: access.to_string(),
            expires: now_ms() + expires_in_ms,
            extra: BTreeMap::new(),
        })
    }

    fn refreshed_credential(access: &str) -> OAuthCredential {
        OAuthCredential {
            refresh: "new-refresh-token".to_string(),
            access: access.to_string(),
            expires: now_ms() + 60 * 60 * 1000,
            extra: BTreeMap::new(),
        }
    }

    async fn stored(provider: &str, credential: Credential) -> InMemoryCredentialStore {
        let store = InMemoryCredentialStore::default();
        let credential = std::sync::Mutex::new(Some(credential));
        store
            .modify(
                provider,
                Box::new(move |_| {
                    let credential = credential.lock().unwrap().take();
                    Box::pin(async move { Ok(credential) })
                }),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        store
    }

    fn api_key_only() -> ProviderAuth {
        ProviderAuth {
            api_key: Some(env_api_key_auth("Fake API key", &[ENV_VAR])),
            oauth: None,
        }
    }

    fn oauth_only(oauth: FakeOAuth) -> ProviderAuth {
        ProviderAuth {
            api_key: None,
            oauth: Some(Arc::new(oauth)),
        }
    }

    fn dual(oauth: FakeOAuth) -> ProviderAuth {
        ProviderAuth {
            api_key: Some(env_api_key_auth("Fake API key", &[ENV_VAR])),
            oauth: Some(Arc::new(oauth)),
        }
    }

    fn no_overrides() -> AuthResolutionOverrides {
        AuthResolutionOverrides::default()
    }

    async fn resolve(
        auth: &ProviderAuth,
        store: &dyn CredentialStore,
        ctx: &MapAuthContext,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<AuthResult>, AuthError> {
        resolve_provider_auth(PROVIDER, auth, store, ctx, overrides).await
    }

    fn models_error(result: Result<Option<AuthResult>, AuthError>) -> ModelsError {
        match result {
            Err(AuthError::Models(error)) => error,
            other => panic!("expected AuthError::Models, got {other:?}"),
        }
    }

    fn assert_cancelled(result: Result<Option<AuthResult>, AuthError>) {
        assert_eq!(result, Err(AuthError::Cancelled));
    }

    fn auth_key(result: Option<AuthResult>) -> String {
        result
            .expect("expected a resolution")
            .auth
            .api_key
            .expect("expected an apiKey")
    }

    // ---------------------------------------------------------------- tests

    // Precedence: explicit option apiKey wins over stored and ambient.

    #[tokio::test]
    async fn explicit_api_key_wins_over_stored_credential_and_env() {
        let store = stored(PROVIDER, api_key_credential("stored-key")).await;
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);
        let overrides = AuthResolutionOverrides {
            api_key: Some("explicit-key".to_string()),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides))
            .await
            .unwrap();
        assert_eq!(auth_key(result), "explicit-key");
    }

    #[tokio::test]
    async fn explicit_api_key_wins_over_stored_oauth_credential() {
        let store = stored(PROVIDER, oauth_credential("oauth-access", 600_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let overrides = AuthResolutionOverrides {
            api_key: Some("explicit-key".to_string()),
            ..no_overrides()
        };
        let result = resolve(
            &dual(FakeOAuth::no_refresh()),
            &store,
            &ctx,
            Some(&overrides),
        )
        .await
        .unwrap();
        assert_eq!(auth_key(result), "explicit-key");
    }

    #[tokio::test]
    async fn explicit_api_key_credential_carries_the_override_env() {
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[]);
        let mut env = ProviderEnv::new();
        env.insert("ACCOUNT_ID".to_string(), "override-account".to_string());
        let overrides = AuthResolutionOverrides {
            api_key: Some("explicit-key".to_string()),
            env: Some(env),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides))
            .await
            .unwrap();
        assert_eq!(auth_key(result.clone()), "explicit-key");
        assert_eq!(
            result
                .unwrap()
                .env
                .unwrap()
                .get("ACCOUNT_ID")
                .map(String::as_str),
            Some("override-account")
        );
    }

    #[tokio::test]
    async fn explicit_api_key_is_ignored_when_the_provider_has_no_api_key_auth() {
        // Upstream resolve.ts:73 — the override only applies when
        // `provider.auth.apiKey` exists; the stored OAuth flow still runs.
        let store = stored(PROVIDER, oauth_credential("oauth-access", 600_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let overrides = AuthResolutionOverrides {
            api_key: Some("explicit-key".to_string()),
            ..no_overrides()
        };
        let result = resolve(
            &oauth_only(FakeOAuth::no_refresh()),
            &store,
            &ctx,
            Some(&overrides),
        )
        .await
        .unwrap();
        assert_eq!(auth_key(result), "oauth-access");
    }

    // Precedence: stored credential owns the provider; env is never consulted.

    #[tokio::test]
    async fn stored_api_key_credential_wins_over_env() {
        let store = stored(PROVIDER, api_key_credential("stored-key")).await;
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);
        let result = resolve(&api_key_only(), &store, &ctx, None).await.unwrap();
        assert_eq!(auth_key(result.clone()), "stored-key");
        assert_eq!(result.unwrap().source.as_deref(), Some("stored credential"));
    }

    #[tokio::test]
    async fn stored_api_key_credential_env_surfaces_and_override_env_wins_per_field() {
        let mut env = ProviderEnv::new();
        env.insert("ACCOUNT_ID".to_string(), "stored-account".to_string());
        let store = stored(
            PROVIDER,
            Credential::ApiKey(ApiKeyCredential {
                key: Some("stored-key".to_string()),
                env: Some(env),
                extra: Default::default(),
            }),
        )
        .await;
        let ctx = MapAuthContext::new(&[]);
        let mut override_env = ProviderEnv::new();
        override_env.insert("ACCOUNT_ID".to_string(), "override-account".to_string());
        override_env.insert("GATEWAY".to_string(), "override-gateway".to_string());
        let overrides = AuthResolutionOverrides {
            env: Some(override_env),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides))
            .await
            .unwrap()
            .unwrap();
        let env = result.env.unwrap();
        assert_eq!(
            env.get("ACCOUNT_ID").map(String::as_str),
            Some("override-account")
        );
        assert_eq!(
            env.get("GATEWAY").map(String::as_str),
            Some("override-gateway")
        );
    }

    #[tokio::test]
    async fn stored_oauth_resolves_the_access_token_with_the_oauth_source_without_refreshing() {
        // Port of the oauth-auth.test.ts oracle "resolves stored anthropic
        // oauth credentials via the lazy flow import".
        let store = stored(PROVIDER, oauth_credential("oauth-access-token", 600_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let result = resolve(&oauth_only(FakeOAuth::no_refresh()), &store, &ctx, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("oauth-access-token"));
        assert_eq!(result.source.as_deref(), Some("OAuth"));
    }

    #[tokio::test]
    async fn stored_oauth_resolves_per_credential_base_url() {
        // Port of the oracle "resolves stored github-copilot oauth credentials
        // including per-credential baseUrl".
        let mut oauth = FakeOAuth::no_refresh();
        oauth.to_auth_base_url_from_access = true;
        let store = stored(
            PROVIDER,
            oauth_credential(
                "tid=abc;exp=123;proxy-ep=proxy.business.githubcopilot.com;rest",
                600_000,
            ),
        )
        .await;
        let ctx = MapAuthContext::new(&[]);
        let result = resolve(&oauth_only(oauth), &store, &ctx, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.auth.base_url.as_deref(),
            Some("https://api.business.githubcopilot.com")
        );
    }

    #[tokio::test]
    async fn stored_credential_without_a_matching_handler_resolves_none() {
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);

        // Stored api_key, provider only offers OAuth.
        let store = stored(PROVIDER, api_key_credential("stored-key")).await;
        assert_eq!(
            resolve(&oauth_only(FakeOAuth::no_refresh()), &store, &ctx, None).await,
            Ok(None)
        );

        // Stored oauth, provider only offers api-key auth.
        let store = stored(PROVIDER, oauth_credential("access", 600_000)).await;
        assert_eq!(resolve(&api_key_only(), &store, &ctx, None).await, Ok(None));

        // Stored api_key, provider offers nothing.
        let store = stored(PROVIDER, api_key_credential("stored-key")).await;
        assert_eq!(
            resolve(&ProviderAuth::default(), &store, &ctx, None).await,
            Ok(None)
        );
    }

    // Precedence: ambient env only when nothing is stored.

    #[tokio::test]
    async fn nothing_stored_resolves_from_env() {
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);
        let result = resolve(&api_key_only(), &store, &ctx, None).await.unwrap();
        assert_eq!(auth_key(result.clone()), "env-key");
        assert_eq!(result.unwrap().source.as_deref(), Some(ENV_VAR));
    }

    #[tokio::test]
    async fn nothing_stored_and_no_env_resolves_none() {
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[]);
        assert_eq!(resolve(&api_key_only(), &store, &ctx, None).await, Ok(None));
        assert_eq!(
            resolve(&oauth_only(FakeOAuth::no_refresh()), &store, &ctx, None).await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn override_env_overlays_the_base_context_and_empty_values_fall_through() {
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[(ENV_VAR, "ambient-key")]);

        // Scoped override wins over the ambient value.
        let mut env = ProviderEnv::new();
        env.insert(ENV_VAR.to_string(), "scoped-key".to_string());
        let overrides = AuthResolutionOverrides {
            env: Some(env),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides))
            .await
            .unwrap();
        assert_eq!(auth_key(result), "scoped-key");

        // An empty override value falls through to the base context (the
        // upstream `env[name] || (await base.env(name))` chain).
        let mut env = ProviderEnv::new();
        env.insert(ENV_VAR.to_string(), String::new());
        let overrides = AuthResolutionOverrides {
            env: Some(env),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides))
            .await
            .unwrap();
        assert_eq!(auth_key(result), "ambient-key");
    }

    // OAuth refresh window and the locked refresh.

    #[tokio::test]
    async fn tokens_beyond_the_default_five_minute_window_do_not_refresh() {
        let oauth = FakeOAuth::no_refresh();
        let store = stored(PROVIDER, oauth_credential("oauth-access", 360_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let result = resolve(&oauth_only(oauth), &store, &ctx, None).await;
        assert_eq!(auth_key(result.unwrap()), "oauth-access");
    }

    #[tokio::test]
    async fn tokens_within_the_default_window_refresh_inside_modify_and_persist() {
        let oauth = FakeOAuth::refreshing(refreshed_credential("new-access"));
        let store = stored(PROVIDER, oauth_credential("old-access", 60_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let result = resolve(&oauth_only(oauth), &store, &ctx, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("new-access"));
        assert_eq!(result.source.as_deref(), Some("OAuth"));
        // The rotated credential is persisted (resolve.ts:153).
        let persisted = store
            .read(PROVIDER, &AuthOperationOptions::default())
            .await
            .unwrap();
        match persisted {
            Some(Credential::OAuth(persisted)) => {
                assert_eq!(persisted.access, "new-access");
                assert_eq!(persisted.refresh, "new-refresh-token");
            }
            other => panic!("expected a stored oauth credential, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_resolutions_refresh_once_through_the_store_lock() {
        // Upstream resolve.ts:122-126 — double-checked locking: the second
        // caller re-checks expiry under the lock and skips its own refresh.
        let store = Arc::new(stored(PROVIDER, oauth_credential("old-access", 60_000)).await);
        let oauth = Arc::new(FakeOAuth {
            refresh_delay_ms: 200,
            ..FakeOAuth::refreshing(refreshed_credential("new-access"))
        });
        let provider = Arc::new(ProviderAuth {
            api_key: None,
            oauth: Some(Arc::clone(&oauth) as Arc<dyn OAuthAuth>),
        });
        let ctx = Arc::new(MapAuthContext::new(&[]));

        let first = {
            let store = Arc::clone(&store);
            let provider = Arc::clone(&provider);
            let ctx = Arc::clone(&ctx);
            tokio::spawn(async move {
                resolve_provider_auth(PROVIDER, &provider, &*store, &*ctx, None).await
            })
        };
        let second = {
            let store = Arc::clone(&store);
            let provider = Arc::clone(&provider);
            let ctx = Arc::clone(&ctx);
            tokio::spawn(async move {
                resolve_provider_auth(PROVIDER, &provider, &*store, &*ctx, None).await
            })
        };

        let first = first.await.unwrap().unwrap().unwrap();
        let second = second.await.unwrap().unwrap().unwrap();
        assert_eq!(first.auth.api_key.as_deref(), Some("new-access"));
        assert_eq!(second.auth.api_key.as_deref(), Some("new-access"));
        assert_eq!(oauth.calls(), 1);
    }

    #[tokio::test]
    async fn explicit_minimum_validity_requires_the_requested_window_after_refresh() {
        // Refresh succeeds but the new token satisfies only the default
        // window; the explicit minimum rejects it (resolve.ts:166-171).
        let mut refreshed = refreshed_credential("new-access");
        refreshed.expires = now_ms() + 8 * 60 * 1000;
        let oauth = FakeOAuth::refreshing(refreshed);
        let store = stored(PROVIDER, oauth_credential("old-access", 60_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let overrides = AuthResolutionOverrides {
            min_oauth_validity_ms: Some(30 * 60 * 1000),
            ..no_overrides()
        };
        let error = models_error(resolve(&oauth_only(oauth), &store, &ctx, Some(&overrides)).await);
        assert_eq!(error.code, ModelsErrorCode::OAuth);
        assert_eq!(
            error.to_string(),
            "OAuth refresh returned a token that expires too soon for fake-provider"
        );
    }

    #[tokio::test]
    async fn explicit_minimum_validity_extends_the_refresh_window() {
        // 8 minutes remaining: beyond the default window, inside the explicit
        // one — refreshes.
        let oauth = FakeOAuth::refreshing(refreshed_credential("new-access"));
        let store = stored(PROVIDER, oauth_credential("old-access", 480_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let overrides = AuthResolutionOverrides {
            min_oauth_validity_ms: Some(30 * 60 * 1000),
            ..no_overrides()
        };
        let result = resolve(&oauth_only(oauth), &store, &ctx, Some(&overrides))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("new-access"));
    }

    // Refresh failure: ModelsError "oauth", credential preserved, NO env fallback.

    #[tokio::test]
    async fn refresh_failure_preserves_the_credential_and_never_falls_back_to_env() {
        let oauth = FakeOAuth {
            refresh_result: Some(Err("invalid_grant".to_string())),
            ..FakeOAuth::no_refresh()
        };
        let store = stored(PROVIDER, oauth_credential("old-access", 60_000)).await;
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);
        let provider = dual(oauth);
        let error = models_error(resolve(&provider, &store, &ctx, None).await);
        assert_eq!(error.code, ModelsErrorCode::OAuth);
        // The cause renders through AuthError's Display (the port's Operation
        // prefix); upstream concatenates the thrown error's message.
        assert_eq!(
            error.to_string(),
            "OAuth refresh failed for fake-provider: auth operation failed: invalid_grant"
        );
        // The credential is preserved for re-login.
        let persisted = store
            .read(PROVIDER, &AuthOperationOptions::default())
            .await
            .unwrap();
        match persisted {
            Some(Credential::OAuth(persisted)) => assert_eq!(persisted.access, "old-access"),
            other => panic!("expected the preserved oauth credential, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_timeout_is_an_oauth_models_error_with_the_abort_cause() {
        let mut oauth = FakeOAuth::no_refresh();
        oauth.refresh_hangs = true;
        let store = stored(PROVIDER, oauth_credential("old-access", 60_000)).await;
        let ctx = MapAuthContext::new(&[]);
        let error = models_error(resolve(&oauth_only(oauth), &store, &ctx, None).await);
        assert_eq!(error.code, ModelsErrorCode::OAuth);
        assert_eq!(
            error.to_string(),
            "OAuth refresh failed for fake-provider: The operation was aborted due to timeout"
        );
    }

    #[tokio::test]
    async fn oauth_derivation_failure_is_an_oauth_models_error() {
        let mut oauth = FakeOAuth::no_refresh();
        oauth.to_auth_error = Some("no org access".to_string());
        let store = stored(PROVIDER, oauth_credential("access", 600_000)).await;
        let ctx = MapAuthContext::new(&[(ENV_VAR, "env-key")]);
        let error = models_error(resolve(&oauth_only(oauth), &store, &ctx, None).await);
        assert_eq!(error.code, ModelsErrorCode::OAuth);
        assert_eq!(
            error.to_string(),
            "OAuth auth derivation failed for fake-provider: auth operation failed: no org access"
        );
    }

    #[tokio::test]
    async fn store_modify_failure_is_an_auth_models_error() {
        // Storage failure while refreshing: wrapped as code "auth", distinct
        // from the callback's typed oauth error.
        struct FailingModifyStore;
        impl CredentialStore for FailingModifyStore {
            fn read<'a>(
                &'a self,
                _provider_id: &'a str,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
                Box::pin(async { Ok(Some(oauth_credential("old-access", 60_000))) })
            }
            fn list<'a>(
                &'a self,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
                Box::pin(async { Ok(Vec::new()) })
            }
            fn modify<'a>(
                &'a self,
                _provider_id: &'a str,
                _f: ModifyCallback,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
                Box::pin(async { Err(AuthError::Storage("disk on fire".to_string())) })
            }
            fn delete<'a>(
                &'a self,
                _provider_id: &'a str,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<(), AuthError>> {
                Box::pin(async { Ok(()) })
            }
        }
        let oauth = FakeOAuth::refreshing(refreshed_credential("new-access"));
        let ctx = MapAuthContext::new(&[]);
        let error = models_error(
            resolve_provider_auth(
                PROVIDER,
                &oauth_only(oauth),
                &FailingModifyStore,
                &ctx,
                None,
            )
            .await,
        );
        assert_eq!(error.code, ModelsErrorCode::Auth);
        assert_eq!(
            error.to_string(),
            "Credential store modify failed for fake-provider: credential storage failure: disk on fire"
        );
    }

    // Api-key resolution and store-read failures: ModelsError "auth".

    #[tokio::test]
    async fn api_key_resolution_failure_is_an_auth_models_error() {
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[]);
        let provider = ProviderAuth {
            api_key: Some(Arc::new(FailingApiKeyAuth {
                name: "Fake API key".to_string(),
            })),
            oauth: None,
        };
        let error = models_error(resolve(&provider, &store, &ctx, None).await);
        assert_eq!(error.code, ModelsErrorCode::Auth);
        assert_eq!(
            error.to_string(),
            "API key auth failed for provider fake-provider: auth operation failed: boom"
        );
    }

    #[tokio::test]
    async fn store_read_failure_is_an_auth_models_error() {
        struct FailingReadStore;
        impl CredentialStore for FailingReadStore {
            fn read<'a>(
                &'a self,
                _provider_id: &'a str,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
                Box::pin(async { Err(AuthError::Storage("disk on fire".to_string())) })
            }
            fn list<'a>(
                &'a self,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
                Box::pin(async { Ok(Vec::new()) })
            }
            fn modify<'a>(
                &'a self,
                _provider_id: &'a str,
                _f: ModifyCallback,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
                Box::pin(async { Ok(None) })
            }
            fn delete<'a>(
                &'a self,
                _provider_id: &'a str,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<(), AuthError>> {
                Box::pin(async { Ok(()) })
            }
        }
        let ctx = MapAuthContext::new(&[]);
        let error = models_error(
            resolve_provider_auth(PROVIDER, &api_key_only(), &FailingReadStore, &ctx, None).await,
        );
        assert_eq!(error.code, ModelsErrorCode::Auth);
        assert_eq!(
            error.to_string(),
            "Credential store read failed for fake-provider: credential storage failure: disk on fire"
        );
    }

    // Cancellation.

    #[tokio::test]
    async fn cancelled_resolution_surfaces_as_cancelled_not_a_models_error() {
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        let store = InMemoryCredentialStore::default();
        let ctx = MapAuthContext::new(&[]);
        let overrides = AuthResolutionOverrides {
            signal: Some(token),
            ..no_overrides()
        };
        let result = resolve(&api_key_only(), &store, &ctx, Some(&overrides)).await;
        assert_cancelled(result);
    }

    #[tokio::test]
    async fn resolution_cancelled_while_awaiting_env_surfaces_as_cancelled() {
        struct HangingAuthContext;
        impl AuthContext for HangingAuthContext {
            fn env<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<String>> {
                Box::pin(std::future::pending())
            }
            fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
                Box::pin(async { false })
            }
        }
        let token = tokio_util::sync::CancellationToken::new();
        let store = Arc::new(InMemoryCredentialStore::default());
        let ctx = Arc::new(HangingAuthContext);
        let overrides = AuthResolutionOverrides {
            signal: Some(token.clone()),
            ..no_overrides()
        };
        let task = {
            let store = Arc::clone(&store);
            let ctx = Arc::clone(&ctx);
            tokio::spawn(async move {
                resolve_provider_auth(PROVIDER, &api_key_only(), &*store, &*ctx, Some(&overrides))
                    .await
            })
        };
        token.cancel();
        assert_cancelled(task.await.unwrap());
    }

    // ModelsError Display keeps the upstream text shape.

    #[test]
    fn models_error_display_and_code_literals_match_upstream() {
        let error = ModelsError::new(ModelsErrorCode::OAuth, "OAuth refresh failed for p");
        assert_eq!(error.to_string(), "OAuth refresh failed for p");
        assert_eq!(ModelsErrorCode::OAuth.as_str(), "oauth");
        assert_eq!(ModelsErrorCode::Auth.as_str(), "auth");
        assert_eq!(ModelsErrorCode::ModelSource.as_str(), "model_source");
        assert_eq!(
            ModelsErrorCode::ModelValidation.as_str(),
            "model_validation"
        );
        assert_eq!(ModelsErrorCode::Provider.as_str(), "provider");
        assert_eq!(ModelsErrorCode::Stream.as_str(), "stream");

        // Cause detail: appended once, trimmed, skipped when already present
        // (upstream `withCauseDetail`).
        let error = ModelsError::with_cause(ModelsErrorCode::Auth, "store failed", "disk on fire");
        assert_eq!(error.to_string(), "store failed: disk on fire");
        let error = ModelsError::with_cause(
            ModelsErrorCode::Auth,
            "store failed: disk on fire",
            "disk on fire",
        );
        assert_eq!(error.to_string(), "store failed: disk on fire");
        let error = ModelsError::with_cause(ModelsErrorCode::Auth, "store failed", "  ");
        assert_eq!(error.to_string(), "store failed");
    }
}
