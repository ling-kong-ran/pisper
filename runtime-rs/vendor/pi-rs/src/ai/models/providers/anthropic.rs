//! Upstream `providers/anthropic.ts`: the Anthropic factory with its custom
//! api-key resolution — a stored key wins, then `ANTHROPIC_AUTH_TOKEN` is
//! passed as `Authorization: Bearer` (it is a passthrough auth token, not an
//! `x-api-key`), then `ANTHROPIC_OAUTH_TOKEN`/`ANTHROPIC_API_KEY` resolve as
//! plain api keys — plus the Claude Pro/Max OAuth flow.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::auth::env_api_keys::{
    ANTHROPIC_API_KEY_ENV, ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV,
};
use crate::ai::auth::helpers::LazyOAuth;
use crate::ai::auth::oauth::load::load_anthropic_oauth;
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthInteraction as _, AuthPrompt,
    AuthPromptKind, AuthResult, ModelAuth, ProviderAuth, ProviderAuthInteraction,
};
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::providers::lazy_flow;
use crate::ai::models::Provider;
use crate::ai::types::ProviderHeaders;

/// Upstream `anthropicApiKeyAuth` (anthropic.ts:16-51).
struct AnthropicApiKeyAuth;

fn aborted(interaction: &ProviderAuthInteraction) -> Option<AuthError> {
    interaction
        .signal
        .is_cancelled()
        .then_some(AuthError::Cancelled)
}

impl ApiKeyAuth for AnthropicApiKeyAuth {
    fn name(&self) -> &str {
        "Anthropic API key"
    }

    /// Upstream anthropic.ts:18-25: prompt for the key (`secret`).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "Enter Anthropic API key".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
                extra: Default::default(),
            })
        }))
    }

    /// Upstream anthropic.ts:26-50: stored credential, then the auth-token
    /// Bearer header, then the OAuth/API-key env vars in that order.
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

            let auth_token = input.ctx.env(ANTHROPIC_AUTH_TOKEN_ENV).await;
            if auth_token.as_deref().is_some_and(|token| !token.is_empty()) {
                let mut headers: ProviderHeaders = ProviderHeaders::new();
                headers.insert(
                    "Authorization".to_string(),
                    Some(format!("Bearer {}", auth_token.unwrap_or_default())),
                );
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        headers: Some(headers),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: Some(ANTHROPIC_AUTH_TOKEN_ENV.to_string()),
                }));
            }

            for env_var in [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV] {
                let api_key = input.ctx.env(env_var).await;
                if api_key.as_deref().is_some_and(|key| !key.is_empty()) {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth {
                            api_key,
                            ..ModelAuth::default()
                        },
                        env: None,
                        source: Some(env_var.to_string()),
                    }));
                }
            }
            Ok(None)
        })
    }
}

/// The upstream `lazyOAuth` wrapper over the anthropic flow loader.
fn anthropic_oauth() -> LazyOAuth {
    lazy_flow(
        "Anthropic (Claude Pro/Max)",
        true,
        None,
        load_anthropic_oauth,
    )
}

/// Upstream `anthropicProvider` (anthropic.ts:54-70).
pub fn anthropic_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "anthropic".to_string(),
        name: Some("Anthropic".to_string()),
        base_url: Some("https://api.anthropic.com".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(AnthropicApiKeyAuth)),
            oauth: Some(Arc::new(anthropic_oauth())),
        },
        models: embedded_provider_catalog("anthropic")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: ApiImpls::Single(Arc::new(AnthropicMessages)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::AuthOperationOptions;
    use crate::ai::models::providers::test_support::{env_context, FakeAuthContext};

    fn resolve_with(
        ctx: &dyn crate::ai::auth::types::AuthContext,
        credential: Option<&ApiKeyCredential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let options = AuthOperationOptions::NONE;
        futures::executor::block_on(AnthropicApiKeyAuth.resolve(ApiKeyAuthInput {
            ctx,
            credential,
            options: &options,
        }))
    }

    fn credential_with_key(key: &str) -> ApiKeyCredential {
        ApiKeyCredential {
            key: Some(key.to_string()),
            env: None,
            extra: Default::default(),
        }
    }

    /// Upstream providers.test.ts "resolves Anthropic bearer auth from env
    /// with auth token precedence" (providers.test.ts:217-231): the auth
    /// token becomes an Authorization Bearer header, not an api key, and wins
    /// over both other env vars.
    #[test]
    fn resolves_bearer_auth_from_env_with_auth_token_precedence() {
        let ctx = FakeAuthContext::env(&[
            (ANTHROPIC_AUTH_TOKEN_ENV, "auth-token"),
            (ANTHROPIC_OAUTH_TOKEN_ENV, "oauth-token"),
            (ANTHROPIC_API_KEY_ENV, "api-key"),
        ]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.auth.api_key, None);
        assert_eq!(
            result.auth.headers.as_ref().unwrap().get("Authorization"),
            Some(&Some("Bearer auth-token".to_string()))
        );
        assert_eq!(result.source.as_deref(), Some("ANTHROPIC_AUTH_TOKEN"));
        assert_eq!(result.env, None);
    }

    /// Upstream providers.test.ts "preserves Anthropic OAuth token precedence
    /// over the API key" (providers.test.ts:233-242).
    #[test]
    fn preserves_oauth_token_precedence_over_the_api_key() {
        let ctx = FakeAuthContext::env(&[
            (ANTHROPIC_API_KEY_ENV, "key"),
            (ANTHROPIC_OAUTH_TOKEN_ENV, "oauth-token"),
        ]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("oauth-token"));
        assert_eq!(result.source.as_deref(), Some("ANTHROPIC_OAUTH_TOKEN"));
    }

    /// Stored credentials own the provider; empty keys fall through to env.
    #[test]
    fn stored_key_wins_and_empty_key_falls_through() {
        let ctx = env_context(&[(ANTHROPIC_API_KEY_ENV, "env-key")]);

        let result = resolve_with(&ctx, Some(&credential_with_key("stored")))
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("stored"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));

        let empty = credential_with_key("");
        let result = resolve_with(&ctx, Some(&empty)).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("env-key"));

        let bare = FakeAuthContext::env(&[]);
        assert_eq!(resolve_with(&bare, None).unwrap(), None);
    }

    /// The advertised OAuth flow: Claude Pro/Max subscription.
    #[test]
    fn oauth_is_the_claude_subscription_flow() {
        let provider = anthropic_provider();
        assert_eq!(provider.id(), "anthropic");
        assert_eq!(provider.name(), "Anthropic");
        assert_eq!(provider.base_url(), Some("https://api.anthropic.com"));
        assert!(!provider.get_models().unwrap().is_empty());
        let oauth = provider.auth().oauth.as_ref().unwrap();
        assert_eq!(oauth.name(), "Anthropic (Claude Pro/Max)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), None);
    }
}
