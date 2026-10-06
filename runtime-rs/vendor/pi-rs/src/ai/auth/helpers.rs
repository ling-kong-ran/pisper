//! Provider auth helpers ported from upstream
//! `packages/ai/src/auth/helpers.ts`: [`env_api_key_auth`] — the standard
//! api-key auth (stored credential wins, otherwise the first set env var) —
//! and [`lazy_oauth`] — the lazy wrapper that lets provider definitions
//! advertise OAuth without importing the flow implementation.

use std::sync::Arc;

use futures::future::BoxFuture;

use super::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthPrompt, AuthPromptKind,
    AuthResult, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use super::types::{AuthInteraction, AuthOperationOptions};

/// Upstream `envApiKeyAuth` (helpers.ts:9-31): standard api-key auth — a
/// stored credential key wins, otherwise the first set env var resolves.
/// Includes a `login` that prompts for the key. Providers with non-standard
/// resolution (provider env, ambient files, IAM) write their own
/// `ApiKeyAuth`.
pub fn env_api_key_auth(name: impl Into<String>, env_vars: &[&str]) -> Arc<dyn ApiKeyAuth> {
    Arc::new(EnvApiKeyAuth {
        name: name.into(),
        env_vars: env_vars.iter().map(|var| (*var).to_string()).collect(),
    })
}

/// The concrete api-key auth behind [`env_api_key_auth`].
pub struct EnvApiKeyAuth {
    name: String,
    env_vars: Vec<String>,
}

impl ApiKeyAuth for EnvApiKeyAuth {
    fn name(&self) -> &str {
        &self.name
    }

    /// Upstream helpers.ts:12-17: prompt for the key (`secret`), returning
    /// `{ type: "api_key", key }`.
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if interaction.signal.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: format!("Enter {}", self.name),
                        placeholder: None,
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
                extra: Default::default(),
            })
        }))
    }

    /// Upstream helpers.ts:18-29: the stored credential key wins (the
    /// upstream `credential?.key` truthiness means an empty key falls
    /// through to env), otherwise the first set env var. `undefined` = not
    /// configured.
    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            input.options.check()?;
            if let Some(key) = input
                .credential
                .and_then(|credential| credential.key.as_deref())
                .filter(|key| !key.is_empty())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key.to_string()),
                        ..ModelAuth::default()
                    },
                    env: input
                        .credential
                        .and_then(|credential| credential.env.clone()),
                    source: Some("stored credential".to_string()),
                }));
            }
            for env_var in &self.env_vars {
                let value = input.ctx.env(env_var).await;
                input.options.check()?;
                if let Some(value) = value {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth {
                            api_key: Some(value),
                            ..ModelAuth::default()
                        },
                        env: None,
                        source: Some(env_var.clone()),
                    }));
                }
            }
            Ok(None)
        })
    }
}

/// Upstream `lazyOAuth` loader (helpers.ts:44): loads the flow on first use.
/// A failing load stays memoized, like the upstream memoized rejected
/// promise.
pub type OAuthLoader =
    dyn Fn() -> BoxFuture<'static, Result<Arc<dyn OAuthAuth>, AuthError>> + Send + Sync;

type LoadedFlow = Result<Arc<dyn OAuthAuth>, AuthError>;

/// Upstream `lazyOAuth(input)` (helpers.ts:40): the factory form — build the
/// wrapper directly as a [`LazyOAuth`] value.
pub fn lazy_oauth(
    name: impl Into<String>,
    is_subscription: bool,
    login_label: Option<String>,
    load: Arc<OAuthLoader>,
) -> LazyOAuth {
    LazyOAuth::new(name, is_subscription, login_label, load)
}

/// Upstream `lazyOAuth` (helpers.ts:40-59): wraps a dynamically loaded
/// `OAuthAuth` so provider definitions can advertise OAuth without importing
/// the implementation. The flow loads on first `login`/`refresh`/`to_auth`
/// call and is memoized (successes and failures alike).
pub struct LazyOAuth {
    name: String,
    is_subscription: bool,
    login_label: Option<String>,
    load: Arc<OAuthLoader>,
    loaded: tokio::sync::OnceCell<LoadedFlow>,
}

impl LazyOAuth {
    pub fn new(
        name: impl Into<String>,
        is_subscription: bool,
        login_label: Option<String>,
        load: Arc<OAuthLoader>,
    ) -> Self {
        LazyOAuth {
            name: name.into(),
            is_subscription,
            login_label,
            load,
            loaded: tokio::sync::OnceCell::new(),
        }
    }

    async fn loaded(&self) -> Result<Arc<dyn OAuthAuth>, AuthError> {
        self.loaded.get_or_init(|| (self.load)()).await.clone()
    }
}

impl OAuthAuth for LazyOAuth {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_subscription(&self) -> bool {
        self.is_subscription
    }

    fn login_label(&self) -> Option<&str> {
        self.login_label.as_deref()
    }

    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { self.loaded().await?.login(interaction).await })
    }

    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { self.loaded().await?.refresh(credential, options).await })
    }

    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move { self.loaded().await?.to_auth(credential).await })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::ai::auth::types::{
        AuthEvent, AuthInteraction, AuthOperationOptions, AuthPrompt, AuthPromptKind,
        ProviderAuthInteraction,
    };

    /// Minimal interaction: answers every prompt with a fixed string and
    /// records the prompts it saw.
    struct FakeInteraction {
        answer: String,
        prompts: Mutex<Vec<AuthPromptKind>>,
    }

    impl AuthInteraction for FakeInteraction {
        fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
            None
        }

        fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
            Box::pin(async move {
                self.prompts.lock().unwrap().push(prompt.kind);
                Ok(self.answer.clone())
            })
        }

        fn notify(&self, _event: AuthEvent) {}
    }

    fn interaction(answer: &str) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
            answer: answer.to_string(),
            prompts: Mutex::new(Vec::new()),
        });
        let normalized = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            tokio_util::sync::CancellationToken::new(),
        );
        (fake, normalized)
    }

    fn input<'a>(
        ctx: &'a dyn super::super::types::AuthContext,
        credential: Option<&'a ApiKeyCredential>,
        options: &'a AuthOperationOptions,
    ) -> ApiKeyAuthInput<'a> {
        ApiKeyAuthInput {
            ctx,
            credential,
            options,
        }
    }

    #[tokio::test]
    async fn stored_key_wins_and_env_vars_resolve_in_table_order() {
        struct ConstContext {
            value: Option<String>,
        }
        impl super::super::types::AuthContext for ConstContext {
            fn env<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Option<String>> {
                Box::pin(async move { self.value.clone() })
            }
            fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
                Box::pin(async { false })
            }
        }

        let options = AuthOperationOptions::default();
        let auth = env_api_key_auth("Fake API key", &["FIRST_VAR", "SECOND_VAR"]);
        let ctx = ConstContext {
            value: Some("env-value".to_string()),
        };

        // Stored credential: key and provider-scoped env surface; no env var
        // is consulted.
        let credential = ApiKeyCredential {
            key: Some("stored-key".to_string()),
            env: None,
            extra: Default::default(),
        };
        let result = auth
            .resolve(input(&ctx, Some(&credential), &options))
            .await
            .unwrap();
        let result = result.unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));

        // No credential: first env var in table order resolves.
        let result = auth
            .resolve(input(&ctx, None, &options))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("env-value"));
        assert_eq!(result.source.as_deref(), Some("FIRST_VAR"));

        // Empty stored key falls through to env (upstream `credential?.key`
        // truthiness).
        let credential = ApiKeyCredential {
            key: Some(String::new()),
            env: None,
            extra: Default::default(),
        };
        let result = auth
            .resolve(input(&ctx, Some(&credential), &options))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("env-value"));
        assert_eq!(result.source.as_deref(), Some("FIRST_VAR"));

        // Unconfigured: None.
        let ctx = ConstContext { value: None };
        assert_eq!(auth.resolve(input(&ctx, None, &options)).await, Ok(None));
    }

    #[tokio::test]
    async fn login_prompts_for_a_secret_and_returns_the_credential() {
        let auth = env_api_key_auth("Fake API key", &["FAKE_VAR"]);
        let (fake, interaction) = interaction("typed-key");

        let credential = auth
            .login(interaction)
            .expect("env api-key auth has a login")
            .await
            .unwrap();
        assert_eq!(credential.key.as_deref(), Some("typed-key"));
        assert_eq!(
            fake.prompts.lock().unwrap().clone(),
            vec![AuthPromptKind::Secret {
                message: "Enter Fake API key".to_string(),
                placeholder: None,
            }]
        );
    }

    #[tokio::test]
    async fn login_aborts_when_cancelled_before_prompting() {
        let auth = env_api_key_auth("Fake API key", &["FAKE_VAR"]);
        let fake = Arc::new(FakeInteraction {
            answer: "unused".to_string(),
            prompts: Mutex::new(Vec::new()),
        });
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        let interaction =
            ProviderAuthInteraction::new(Arc::clone(&fake) as Arc<dyn AuthInteraction>, token);
        let result = auth.login(interaction).unwrap().await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(fake.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn ambient_only_api_key_auth_has_no_login_or_check() {
        struct AmbientOnlyAuth;
        impl ApiKeyAuth for AmbientOnlyAuth {
            fn name(&self) -> &str {
                "Ambient"
            }
            fn resolve<'a>(
                &'a self,
                _input: ApiKeyAuthInput<'a>,
            ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
                Box::pin(async { Ok(None) })
            }
        }
        let auth: Arc<dyn ApiKeyAuth> = Arc::new(AmbientOnlyAuth);
        let (fake, interaction) = interaction("x");
        assert!(auth.login(interaction).is_none());
        assert!(fake.prompts.lock().unwrap().is_empty());
        let options = AuthOperationOptions::default();
        let ctx = crate::ai::auth::context::DefaultAuthContext;
        assert!(auth.check(input(&ctx, None, &options)).is_none());
    }

    #[tokio::test]
    async fn lazy_oauth_loads_once_and_memorizes_failures() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct LoadedFlowStub;
        impl OAuthAuth for LoadedFlowStub {
            fn name(&self) -> &str {
                "Loaded Flow"
            }
            fn is_subscription(&self) -> bool {
                true
            }
            fn login_label(&self) -> Option<&str> {
                Some("Sign in with Fake")
            }
            fn login<'a>(
                &'a self,
                _interaction: ProviderAuthInteraction,
            ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
                Box::pin(async { Err(AuthError::Operation("no browser".to_string())) })
            }
            fn refresh<'a>(
                &'a self,
                credential: OAuthCredential,
                _options: &'a AuthOperationOptions,
            ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
                Box::pin(async move { Ok(credential) })
            }
            fn to_auth<'a>(
                &'a self,
                credential: OAuthCredential,
            ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
                Box::pin(async move {
                    Ok(ModelAuth {
                        api_key: Some(credential.access),
                        ..ModelAuth::default()
                    })
                })
            }
        }

        let loads = Arc::new(AtomicUsize::new(0));
        let failing_loads = Arc::new(AtomicUsize::new(0));
        let failing_loads_for_closure = Arc::clone(&failing_loads);

        let loads_for_closure = Arc::clone(&loads);
        let lazy: Arc<dyn OAuthAuth> = Arc::new(LazyOAuth::new(
            "Fake OAuth",
            true,
            Some("Sign in with Fake".to_string()),
            Arc::new(move || {
                let loads = Arc::clone(&loads_for_closure);
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    let stub: Arc<dyn OAuthAuth> = Arc::new(LoadedFlowStub);
                    Ok(stub)
                }) as BoxFuture<'static, LoadedFlow>
            }),
        ));

        // Metadata forwards without loading.
        assert_eq!(lazy.name(), "Fake OAuth");
        assert!(lazy.is_subscription());
        assert_eq!(lazy.login_label(), Some("Sign in with Fake"));
        assert_eq!(loads.load(Ordering::SeqCst), 0);

        // First to_auth loads; the second call reuses the memoized flow.
        let credential = OAuthCredential {
            refresh: "r".to_string(),
            access: "access-token".to_string(),
            expires: 0,
            extra: Default::default(),
        };
        let auth = lazy.to_auth(credential.clone()).await.unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("access-token"));
        let auth = lazy.to_auth(credential).await.unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("access-token"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        // A failing load stays memoized: both refresh attempts see the same
        // failure, and the loader ran once (upstream memoized rejected
        // promise).
        let failing: Arc<dyn OAuthAuth> = Arc::new(LazyOAuth::new(
            "Broken OAuth",
            false,
            None,
            Arc::new(move || {
                let failing_loads = Arc::clone(&failing_loads_for_closure);
                Box::pin(async move {
                    failing_loads.fetch_add(1, Ordering::SeqCst);
                    Err(AuthError::Operation("flow crashed".to_string()))
                }) as BoxFuture<'static, LoadedFlow>
            }),
        ));
        let credential = OAuthCredential {
            refresh: "r".to_string(),
            access: "a".to_string(),
            expires: 0,
            extra: Default::default(),
        };
        assert_eq!(
            failing
                .refresh(credential.clone(), &AuthOperationOptions::default())
                .await,
            Err(AuthError::Operation("flow crashed".to_string()))
        );
        assert_eq!(
            failing
                .refresh(credential, &AuthOperationOptions::default())
                .await,
            Err(AuthError::Operation("flow crashed".to_string()))
        );
        assert_eq!(failing_loads.load(Ordering::SeqCst), 1);
    }
}
