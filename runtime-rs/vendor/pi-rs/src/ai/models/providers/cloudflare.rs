//! Upstream `providers/cloudflare-auth.ts` + `providers/cloudflare-stream.ts`
//! plus the two factories (`cloudflare-workers-ai.ts`,
//! `cloudflare-ai-gateway.ts`).
//!
//! Cloudflare needs three values: an API key plus the account id (and, for AI
//! Gateway, the gateway id). Values merge per field — a stored credential's
//! key/env wins, ambient env fills the rest — and the resolved ids ride back
//! as provider-scoped env, which the stream wrapper uses to materialize the
//! `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}` placeholders in the
//! catalog baseUrls before dispatch. The gateway flavor authenticates with a
//! `cf-aig-authorization` Bearer header and suppresses the standard
//! `Authorization`/`x-api-key` headers.

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::api::openai_completions::OpenAiCompletions;
use crate::ai::api::openai_responses::OpenAiResponses;
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthError, AuthInteraction as _,
    AuthPrompt, AuthPromptKind, AuthResult, ModelAuth, ProviderAuth, ProviderAuthInteraction,
};
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::providers::{arc, per_api};
use crate::ai::models::Provider;
use crate::ai::transcript::TranscriptContext;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::options::{ProviderEnv, SimpleStreamOptions, StreamOptions};
use crate::ai::types::Model;
use crate::ai::{ApiImpl, ProviderConfig};

const CLOUDFLARE_API_KEY: &str = "CLOUDFLARE_API_KEY";
const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

/// Upstream `CloudflareAuthKind` (cloudflare-auth.ts:11).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudflareAuthKind {
    WorkersAi,
    AiGateway,
}

/// Upstream `resolveValue` (cloudflare-auth.ts:13-29): per-field merge —
/// prefer the credential value, fall back to ambient env. A credential
/// carrying only the API key must still pick up the account/gateway id from
/// the environment.
async fn resolve_value<'a>(
    name: &'a str,
    ctx: &'a dyn AuthContext,
    credential: Option<&'a ApiKeyCredential>,
    options: &'a crate::ai::auth::types::AuthOperationOptions,
) -> Result<Option<String>, AuthError> {
    let from_credential = credential.and_then(|credential| {
        if name == CLOUDFLARE_API_KEY {
            credential.key.clone()
        } else {
            credential
                .env
                .as_ref()
                .and_then(|env| env.get(name).cloned())
        }
    });
    if from_credential.is_some() {
        return Ok(from_credential);
    }
    options.check()?;
    Ok(ctx.env(name).await)
}

struct ResolvedCloudflareEnv {
    api_key: String,
    env: ProviderEnv,
    source: &'static str,
}

/// Upstream `resolveCloudflareEnv` (cloudflare-auth.ts:31-59).
async fn resolve_cloudflare_env<'a>(
    kind: CloudflareAuthKind,
    ctx: &'a dyn AuthContext,
    credential: Option<&'a ApiKeyCredential>,
    options: &'a crate::ai::auth::types::AuthOperationOptions,
) -> Result<Option<ResolvedCloudflareEnv>, AuthError> {
    let api_key = resolve_value(CLOUDFLARE_API_KEY, ctx, credential, options).await?;
    let account_id = resolve_value(CLOUDFLARE_ACCOUNT_ID, ctx, credential, options).await?;
    let gateway_id = match kind {
        CloudflareAuthKind::AiGateway => {
            resolve_value(CLOUDFLARE_GATEWAY_ID, ctx, credential, options).await?
        }
        CloudflareAuthKind::WorkersAi => None,
    };

    // Truthiness on every value, like the upstream `!apiKey || !accountId ...`.
    let present = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.is_empty());
    if !present(&api_key)
        || !present(&account_id)
        || (kind == CloudflareAuthKind::AiGateway && !present(&gateway_id))
    {
        return Ok(None);
    }

    let mut env = ProviderEnv::new();
    env.insert(
        CLOUDFLARE_ACCOUNT_ID.to_string(),
        account_id.unwrap_or_default(),
    );
    if let Some(gateway_id) = gateway_id.filter(|gateway| !gateway.is_empty()) {
        env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
    }
    Ok(Some(ResolvedCloudflareEnv {
        api_key: api_key.unwrap_or_default(),
        env,
        source: if credential.is_some() {
            "stored credential"
        } else {
            CLOUDFLARE_API_KEY
        },
    }))
}

/// Upstream `cloudflareWorkersAIAuth` (cloudflare-auth.ts:62-83).
struct CloudflareWorkersAiAuth;

impl ApiKeyAuth for CloudflareWorkersAiAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "Enter Cloudflare API key".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let account_id = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Cloudflare account ID".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let mut env = ProviderEnv::new();
            env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
            Ok(ApiKeyCredential {
                key: Some(key),
                env: Some(env),
                extra: Default::default(),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let resolved = resolve_cloudflare_env(
                CloudflareAuthKind::WorkersAi,
                input.ctx,
                input.credential,
                input.options,
            )
            .await?;
            Ok(resolved.map(|resolved| AuthResult {
                auth: ModelAuth {
                    api_key: Some(resolved.api_key),
                    ..ModelAuth::default()
                },
                env: Some(resolved.env),
                source: Some(resolved.source.to_string()),
            }))
        })
    }
}

/// Upstream `cloudflareAIGatewayAuth` (cloudflare-auth.ts:85-115).
struct CloudflareAiGatewayAuth;

impl ApiKeyAuth for CloudflareAiGatewayAuth {
    fn name(&self) -> &str {
        "Cloudflare API key"
    }

    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: "Enter Cloudflare API key".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let account_id = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Cloudflare account ID".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let gateway_id = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Cloudflare AI Gateway ID".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let mut env = ProviderEnv::new();
            env.insert(CLOUDFLARE_ACCOUNT_ID.to_string(), account_id);
            env.insert(CLOUDFLARE_GATEWAY_ID.to_string(), gateway_id);
            Ok(ApiKeyCredential {
                key: Some(key),
                env: Some(env),
                extra: Default::default(),
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let resolved = resolve_cloudflare_env(
                CloudflareAuthKind::AiGateway,
                input.ctx,
                input.credential,
                input.options,
            )
            .await?;
            Ok(resolved.map(|resolved| {
                let mut headers = crate::ai::types::ProviderHeaders::new();
                headers.insert(
                    "cf-aig-authorization".to_string(),
                    Some(format!("Bearer {}", resolved.api_key)),
                );
                // `null` suppresses the standard auth headers at dispatch.
                headers.insert("Authorization".to_string(), None);
                headers.insert("x-api-key".to_string(), None);
                AuthResult {
                    auth: ModelAuth {
                        headers: Some(headers),
                        ..ModelAuth::default()
                    },
                    env: Some(resolved.env),
                    source: Some(resolved.source.to_string()),
                }
            }))
        })
    }
}

/// Upstream `resolveCloudflareModel` (cloudflare-stream.ts:6-15): replace the
/// `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}` placeholders from the
/// resolved provider env (unknown values keep their placeholder). `None` =
/// nothing to resolve (no env, or the URL was unchanged).
fn resolve_cloudflare_model(model: &Model, env: Option<&ProviderEnv>) -> Option<Model> {
    let env = env?;
    let base_url = replace_placeholders(&model.base_url, env);
    if base_url == model.base_url {
        return None;
    }
    let mut model = model.clone();
    model.base_url = base_url;
    Some(model)
}

/// Upstream `resolveCloudflareModel` generalized to any model shape with a
/// base URL (providers/cloudflare-stream.ts:8-24, the #9948 widening): the
/// placeholder-materialized URL, or the input unchanged without env.
pub(crate) fn resolve_cloudflare_base_url(base_url: &str, env: Option<&ProviderEnv>) -> String {
    match env {
        Some(env) => replace_placeholders(base_url, env),
        None => base_url.to_string(),
    }
}

/// The `replaceAll` pair: env value or the placeholder itself.
fn placeholder_value(env: &ProviderEnv, name: &str) -> String {
    match env.get(name) {
        Some(value) => value.clone(),
        None => format!("{{{name}}}"),
    }
}

/// The placeholder replacement, applied to any URL (the port additionally
/// resolves `ProviderConfig.base_url`, which is where the port's API impls
/// read the request endpoint from).
fn replace_placeholders(url: &str, env: &ProviderEnv) -> String {
    url.replace(
        &format!("{{{CLOUDFLARE_ACCOUNT_ID}}}"),
        &placeholder_value(env, CLOUDFLARE_ACCOUNT_ID),
    )
    .replace(
        &format!("{{{CLOUDFLARE_GATEWAY_ID}}}"),
        &placeholder_value(env, CLOUDFLARE_GATEWAY_ID),
    )
}

/// Upstream `cloudflareStreams` (cloudflare-stream.ts:17-29): wrap an API
/// implementation so Cloudflare account/gateway endpoint placeholders
/// materialize from the resolved provider env before dispatch.
struct CloudflareStreams {
    inner: Arc<dyn ApiImpl>,
}

impl ApiImpl for CloudflareStreams {
    fn stream(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &StreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let mut cfg = cfg.clone();
        if let Some(env) = options.env.as_ref() {
            cfg.base_url = replace_placeholders(&cfg.base_url, env);
        }
        let model =
            resolve_cloudflare_model(model, options.env.as_ref()).unwrap_or_else(|| model.clone());
        self.inner.stream(&cfg, &model, ctx, options)
    }

    fn stream_simple(
        &self,
        cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let mut cfg = cfg.clone();
        if let Some(env) = options.stream.env.as_ref() {
            cfg.base_url = replace_placeholders(&cfg.base_url, env);
        }
        let model = resolve_cloudflare_model(model, options.stream.env.as_ref())
            .unwrap_or_else(|| model.clone());
        self.inner.stream_simple(&cfg, &model, ctx, options)
    }
}

/// Upstream `cloudflareAIGatewayProvider` (cloudflare-ai-gateway.ts:14-36):
/// the api map is pinned to all three APIs — models.dev's gateway catalog
/// drops and restores `workers-ai/*` (openai-completions) entries over time,
/// and inference from `models` alone would otherwise reject the
/// openai-completions entry whenever the generated catalog happens to contain
/// none.
pub fn cloudflare_ai_gateway_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "cloudflare-ai-gateway".to_string(),
        name: Some("Cloudflare AI Gateway".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(CloudflareAiGatewayAuth)),
            oauth: None,
        },
        models: embedded_provider_catalog("cloudflare-ai-gateway")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: per_api(&[
            (
                "anthropic-messages",
                Arc::new(CloudflareStreams {
                    inner: arc(AnthropicMessages),
                }) as Arc<dyn ApiImpl>,
            ),
            (
                "openai-completions",
                Arc::new(CloudflareStreams {
                    inner: arc(OpenAiCompletions),
                }) as Arc<dyn ApiImpl>,
            ),
            (
                "openai-responses",
                Arc::new(CloudflareStreams {
                    inner: arc(OpenAiResponses),
                }) as Arc<dyn ApiImpl>,
            ),
        ]),
    })
}

/// Upstream `cloudflareWorkersAIProvider` (cloudflare-workers-ai.ts:8-17).
pub fn cloudflare_workers_ai_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "cloudflare-workers-ai".to_string(),
        name: Some("Cloudflare Workers AI".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(CloudflareWorkersAiAuth)),
            oauth: None,
        },
        models: embedded_provider_catalog("cloudflare-workers-ai")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: ApiImpls::Single(Arc::new(CloudflareStreams {
            inner: arc(OpenAiCompletions),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::{AuthOperationOptions, Credential};
    use crate::ai::models::providers::test_support::{api_model, gateway_model, FakeAuthContext};

    fn resolve_workers_ai(
        ctx: &dyn AuthContext,
        credential: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let options = AuthOperationOptions::NONE;
        let api_credential = credential.map(|credential| match credential {
            Credential::ApiKey(credential) => credential,
            Credential::OAuth(_) => panic!("cloudflare tests use api-key credentials"),
        });
        futures::executor::block_on(CloudflareWorkersAiAuth.resolve(ApiKeyAuthInput {
            ctx,
            credential: api_credential,
            options: &options,
        }))
    }

    fn resolve_gateway(
        ctx: &dyn AuthContext,
        credential: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let options = AuthOperationOptions::NONE;
        let api_credential = credential.map(|credential| match credential {
            Credential::ApiKey(credential) => credential,
            Credential::OAuth(_) => panic!("cloudflare tests use api-key credentials"),
        });
        futures::executor::block_on(CloudflareAiGatewayAuth.resolve(ApiKeyAuthInput {
            ctx,
            credential: api_credential,
            options: &options,
        }))
    }

    fn map(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    /// Upstream providers.test.ts "requires Cloudflare Workers AI account
    /// config and returns scoped env" (providers.test.ts:293-306).
    #[test]
    fn workers_ai_requires_account_config_and_returns_scoped_env() {
        let missing_account = FakeAuthContext::env(&[(CLOUDFLARE_API_KEY, "cf-key")]);
        assert_eq!(resolve_workers_ai(&missing_account, None).unwrap(), None);

        let configured = FakeAuthContext::env(&[
            (CLOUDFLARE_API_KEY, "cf-key"),
            (CLOUDFLARE_ACCOUNT_ID, "account-id"),
        ]);
        let result = resolve_workers_ai(&configured, None).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("cf-key"));
        assert_eq!(
            result.env.as_ref(),
            Some(&map(&[(CLOUDFLARE_ACCOUNT_ID, "account-id")]))
        );
        assert_eq!(result.source.as_deref(), Some("CLOUDFLARE_API_KEY"));
    }

    /// Upstream providers.test.ts "requires Cloudflare AI Gateway account and
    /// gateway config and returns scoped env headers" (providers.test.ts:308-336).
    #[test]
    fn gateway_requires_account_and_gateway_and_returns_scoped_env_headers() {
        let missing_gateway = FakeAuthContext::env(&[
            (CLOUDFLARE_API_KEY, "cf-key"),
            (CLOUDFLARE_ACCOUNT_ID, "account-id"),
        ]);
        assert_eq!(resolve_gateway(&missing_gateway, None).unwrap(), None);

        let configured = FakeAuthContext::env(&[
            (CLOUDFLARE_API_KEY, "cf-key"),
            (CLOUDFLARE_ACCOUNT_ID, "account-id"),
            (CLOUDFLARE_GATEWAY_ID, "gateway-id"),
        ]);
        let result = resolve_gateway(&configured, None).unwrap().unwrap();
        let headers = result.auth.headers.as_ref().unwrap();
        assert_eq!(
            headers.get("cf-aig-authorization"),
            Some(&Some("Bearer cf-key".to_string()))
        );
        // `null` upstream = suppression entries here.
        assert_eq!(headers.get("Authorization"), Some(&None));
        assert_eq!(headers.get("x-api-key"), Some(&None));
        assert_eq!(result.auth.api_key, None);
        assert_eq!(
            result.env.as_ref(),
            Some(&map(&[
                (CLOUDFLARE_ACCOUNT_ID, "account-id"),
                (CLOUDFLARE_GATEWAY_ID, "gateway-id"),
            ]))
        );
    }

    /// Per-field merge (cloudflare-auth.ts:13-29): a credential carrying only
    /// the key picks the ids up from ambient env, and vice versa; stored
    /// values relabel the source.
    #[test]
    fn values_merge_per_field_between_credential_and_env() {
        let ctx = FakeAuthContext::env(&[(CLOUDFLARE_ACCOUNT_ID, "ambient-account")]);
        let credential = Credential::ApiKey(ApiKeyCredential {
            key: Some("stored-key".to_string()),
            env: None,
            extra: Default::default(),
        });
        let result = resolve_workers_ai(&ctx, Some(&credential))
            .unwrap()
            .unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(
            result
                .env
                .as_ref()
                .unwrap()
                .get(CLOUDFLARE_ACCOUNT_ID)
                .map(String::as_str),
            Some("ambient-account")
        );
        assert_eq!(result.source.as_deref(), Some("stored credential"));

        // A stored account id overrides the ambient one.
        let ctx = FakeAuthContext::env(&[(CLOUDFLARE_ACCOUNT_ID, "ambient-account")]);
        let credential = Credential::ApiKey(ApiKeyCredential {
            key: Some("stored-key".to_string()),
            env: Some(map(&[(CLOUDFLARE_ACCOUNT_ID, "stored-account")])),
            extra: Default::default(),
        });
        let result = resolve_workers_ai(&ctx, Some(&credential))
            .unwrap()
            .unwrap();
        assert_eq!(
            result
                .env
                .as_ref()
                .unwrap()
                .get(CLOUDFLARE_ACCOUNT_ID)
                .map(String::as_str),
            Some("stored-account")
        );
    }

    /// Upstream `resolveCloudflareModel` (cloudflare-stream.ts:6-15):
    /// placeholders resolve from env and missing values keep the placeholder.
    #[test]
    fn model_placeholders_resolve_from_env() {
        let model = gateway_model(
            "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/workers-ai",
        );

        // No env: the model is unchanged (the upstream early return).
        assert!(resolve_cloudflare_model(&model, None).is_none());

        // Missing gateway value keeps its placeholder.
        let env = map(&[(CLOUDFLARE_ACCOUNT_ID, "acc")]);
        let resolved = resolve_cloudflare_model(&model, Some(&env)).unwrap();
        assert_eq!(
            resolved.base_url,
            "https://gateway.ai.cloudflare.com/v1/acc/{CLOUDFLARE_GATEWAY_ID}/workers-ai"
        );

        // Both values present: fully resolved.
        let env = map(&[
            (CLOUDFLARE_ACCOUNT_ID, "acc"),
            (CLOUDFLARE_GATEWAY_ID, "gw"),
        ]);
        let resolved = resolve_cloudflare_model(&model, Some(&env)).unwrap();
        assert_eq!(
            resolved.base_url,
            "https://gateway.ai.cloudflare.com/v1/acc/gw/workers-ai"
        );

        // Nothing to replace: unchanged.
        let plain = gateway_model("https://example.test");
        assert!(resolve_cloudflare_model(&plain, Some(&env)).is_none());
    }

    /// A capturing stub to observe what the wrapper hands to the inner API.
    struct CapturingApi {
        seen: std::sync::Mutex<(Option<ProviderConfig>, Option<String>)>,
    }

    impl CapturingApi {
        fn new() -> Arc<Self> {
            Arc::new(CapturingApi {
                seen: std::sync::Mutex::new((None, None)),
            })
        }

        fn observed(&self) -> (Option<ProviderConfig>, Option<String>) {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ApiImpl for CapturingApi {
        fn stream(
            &self,
            cfg: &ProviderConfig,
            model: &Model,
            _ctx: &TranscriptContext,
            _options: &StreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            seen.0 = Some(cfg.clone());
            seen.1 = Some(model.base_url.clone());
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            rx
        }

        fn stream_simple(
            &self,
            cfg: &ProviderConfig,
            model: &Model,
            _ctx: &TranscriptContext,
            _options: &SimpleStreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            self.stream(
                cfg,
                model,
                &TranscriptContext::default(),
                &StreamOptions::default(),
            )
        }
    }

    /// The wrapper resolves both the model URL and the config endpoint the
    /// port's API impls dispatch through.
    #[test]
    fn stream_wrapper_resolves_config_and_model_urls() {
        let inner = CapturingApi::new();
        let wrapper = CloudflareStreams {
            inner: Arc::clone(&inner) as Arc<dyn ApiImpl>,
        };
        let model = gateway_model(
            "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/gw/workers-ai",
        );
        let cfg = ProviderConfig {
            base_url: model.base_url.clone(),
            api_key: String::new(),
            max_tokens: 0,
        };
        let options = StreamOptions {
            env: Some(map(&[(CLOUDFLARE_ACCOUNT_ID, "acc-42")])),
            ..StreamOptions::default()
        };
        let mut rx = wrapper.stream(&cfg, &model, &TranscriptContext::default(), &options);
        assert!(futures::executor::block_on(rx.recv()).is_none());

        let (observed_cfg, model_url) = inner.observed();
        assert_eq!(
            observed_cfg.unwrap().base_url,
            "https://gateway.ai.cloudflare.com/v1/acc-42/gw/workers-ai"
        );
        assert_eq!(
            model_url.unwrap(),
            "https://gateway.ai.cloudflare.com/v1/acc-42/gw/workers-ai"
        );

        // Without env the URLs pass through untouched.
        let mut rx = wrapper.stream(
            &cfg,
            &model,
            &TranscriptContext::default(),
            &StreamOptions::default(),
        );
        assert!(futures::executor::block_on(rx.recv()).is_none());
        let (cfg, model_url) = inner.observed();
        assert_eq!(cfg.unwrap().base_url, model.base_url);
        assert_eq!(model_url.unwrap(), model.base_url);
    }

    /// The factories: ids, names, catalogs, and pinned API maps (the gateway
    /// map is pinned to all three APIs — cloudflare-ai-gateway.ts:17-21).
    #[test]
    fn factories_build_both_cloudflare_providers() {
        let gateway = cloudflare_ai_gateway_provider();
        assert_eq!(gateway.id(), "cloudflare-ai-gateway");
        assert_eq!(gateway.name(), "Cloudflare AI Gateway");
        assert_eq!(gateway.base_url(), None);
        assert!(!gateway.get_models().unwrap().is_empty());
        let apis = [
            "anthropic-messages",
            "openai-completions",
            "openai-responses",
        ];
        for model in gateway.get_models().unwrap() {
            assert!(apis.contains(&model.api.as_str()), "{}", model.api);
            assert!(gateway.api_for(&model).is_some());
        }
        // The map serves every pinned api even when the catalog has no entry.
        let ghost = api_model("cloudflare-ai-gateway", "openai-completions");
        assert!(gateway.api_for(&ghost).is_some());

        let workers = cloudflare_workers_ai_provider();
        assert_eq!(workers.id(), "cloudflare-workers-ai");
        assert_eq!(workers.name(), "Cloudflare Workers AI");
        for model in workers.get_models().unwrap() {
            assert_eq!(model.api, "openai-completions");
            assert!(workers.api_for(&model).is_some());
        }
    }
}
