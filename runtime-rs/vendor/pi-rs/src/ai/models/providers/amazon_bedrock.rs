//! Upstream `providers/amazon-bedrock.ts`: the Bedrock factory. Bedrock
//! accepts a bearer token or the AWS SDK's default credential chain — the
//! login flow stores a token/profile choice, and resolve detects ambient AWS
//! credentials *without copying them into pi's credential store*: ambient
//! results carry `auth: {}` (the Bedrock API impl reads the AWS env itself),
//! only the source label and any stored provider env.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::api::bedrock::BedrockConverseStream;
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthEvent, AuthInfoLink,
    AuthInteraction as _, AuthPrompt, AuthPromptKind, AuthPromptOption, AuthResult, ModelAuth,
    ProviderAuth, ProviderAuthInteraction,
};
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::Provider;

/// Upstream anthropic.ts `interaction.signal.throwIfAborted()` equivalent.
fn aborted(interaction: &ProviderAuthInteraction) -> Option<AuthError> {
    interaction
        .signal
        .is_cancelled()
        .then_some(AuthError::Cancelled)
}

/// The `AWS_PROFILE` credential name.
const AWS_PROFILE: &str = "AWS_PROFILE";

/// Upstream `bedrockAuth` (amazon-bedrock.ts:12-79).
struct BedrockAuth;

impl BedrockAuth {
    /// Upstream login's shared "how does the credential chain work" notice.
    fn chain_notice() -> AuthEvent {
        AuthEvent::Info {
            message:
                "Amazon Bedrock supports AWS profiles, IAM credentials, and role-based credentials."
                    .to_string(),
            links: Some(vec![AuthInfoLink {
                label: Some("AWS credential provider chain".to_string()),
                url:
                    "https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html"
                        .to_string(),
            }]),
        }
    }
}

/// The upstream resolve's local `env` helper (amazon-bedrock.ts:51-56): an
/// abort-checked context read (checked before and after).
async fn env_read<'a>(
    input: &'a ApiKeyAuthInput<'a>,
    name: &'a str,
) -> Result<Option<String>, AuthError> {
    input.options.check()?;
    let value = input.ctx.env(name).await;
    input.options.check()?;
    Ok(value)
}

impl ApiKeyAuth for BedrockAuth {
    fn name(&self) -> &str {
        "AWS credentials or bearer token"
    }

    /// Upstream amazon-bedrock.ts:14-48: a select over bearer token / AWS
    /// profile / existing credential chain, then the method-specific prompts.
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            let method = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Select {
                        message: "Select Amazon Bedrock authentication method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: "bearer-token".to_string(),
                                label: "Bearer token".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "aws-profile".to_string(),
                                label: "AWS profile".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "credential-chain".to_string(),
                                label: "Existing AWS credential chain".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            match method.as_str() {
                "bearer-token" => {
                    let key = interaction
                        .prompt(AuthPrompt {
                            signal: None,
                            kind: AuthPromptKind::Secret {
                                message: "Enter Amazon Bedrock bearer token".to_string(),
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
                }
                "aws-profile" | "credential-chain" => {
                    interaction.notify(Self::chain_notice());
                    if method == "aws-profile" {
                        let profile = interaction
                            .prompt(AuthPrompt {
                                signal: None,
                                kind: AuthPromptKind::Text {
                                    message: "Enter AWS profile name".to_string(),
                                    placeholder: None,
                                },
                            })
                            .await?;
                        if let Some(error) = aborted(&interaction) {
                            return Err(error);
                        }
                        let mut env = crate::ai::types::ProviderEnv::new();
                        env.insert(AWS_PROFILE.to_string(), profile);
                        Ok(ApiKeyCredential {
                            key: None,
                            env: Some(env),
                            extra: Default::default(),
                        })
                    } else {
                        interaction
                            .prompt(AuthPrompt {
                                signal: None,
                                kind: AuthPromptKind::Text {
                                    message:
                                        "Configure AWS credentials, then press Enter to continue"
                                            .to_string(),
                                    placeholder: None,
                                },
                            })
                            .await?;
                        if let Some(error) = aborted(&interaction) {
                            return Err(error);
                        }
                        Ok(ApiKeyCredential::default())
                    }
                }
                other => Err(AuthError::Operation(format!(
                    "Unknown Amazon Bedrock auth method: {other}"
                ))),
            }
        }))
    }

    /// Upstream amazon-bedrock.ts:50-77: the ambient chain, in order — stored
    /// key, `AWS_BEARER_TOKEN_BEDROCK`, profile, IAM access keys, ECS task
    /// roles, web identity.
    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
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
            if env_read(&input, "AWS_BEARER_TOKEN_BEDROCK")
                .await?
                .is_some()
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: None,
                    source: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
                }));
            }

            let stored_profile = input
                .credential
                .and_then(|credential| credential.env.as_ref())
                .and_then(|env| env.get(AWS_PROFILE))
                .filter(|profile| !profile.is_empty())
                .cloned();
            let ambient_profile = env_read(&input, AWS_PROFILE).await?;
            if stored_profile.is_some() || ambient_profile.is_some() {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: input
                        .credential
                        .and_then(|credential| credential.env.clone()),
                    source: Some(
                        if stored_profile.is_some() {
                            "stored credential"
                        } else {
                            "AWS_PROFILE"
                        }
                        .to_string(),
                    ),
                }));
            }

            let access_key = env_read(&input, "AWS_ACCESS_KEY_ID").await?;
            let secret_key = env_read(&input, "AWS_SECRET_ACCESS_KEY").await?;
            if access_key.is_some() && secret_key.is_some() {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: None,
                    source: Some("AWS access keys".to_string()),
                }));
            }
            for (name, source) in [
                ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "ECS task role"),
                ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "ECS task role"),
                ("AWS_WEB_IDENTITY_TOKEN_FILE", "web identity token"),
            ] {
                if env_read(&input, name).await?.is_some() {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth::default(),
                        env: None,
                        source: Some(source.to_string()),
                    }));
                }
            }
            Ok(None)
        })
    }
}

/// Upstream `amazonBedrockProvider` (amazon-bedrock.ts:82-95).
pub fn amazon_bedrock_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "amazon-bedrock".to_string(),
        name: Some("Amazon Bedrock".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(BedrockAuth)),
            oauth: None,
        },
        models: embedded_provider_catalog("amazon-bedrock")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: ApiImpls::Single(Arc::new(BedrockConverseStream)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::{AuthOperationOptions, Credential};
    use crate::ai::models::providers::test_support::{
        env_credential, interaction, FakeAuthContext, ScriptedInteraction,
    };

    fn resolve_with(
        ctx: &dyn crate::ai::auth::types::AuthContext,
        credential: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let options = AuthOperationOptions::NONE;
        let api_credential = credential.map(|credential| match credential {
            Credential::ApiKey(credential) => credential,
            Credential::OAuth(_) => panic!("bedrock tests use api-key credentials"),
        });
        futures::executor::block_on(BedrockAuth.resolve(ApiKeyAuthInput {
            ctx,
            credential: api_credential,
            options: &options,
        }))
    }

    /// Upstream providers.test.ts "runs provider-owned Bedrock bearer token
    /// and AWS profile login flows" (providers.test.ts:244-277).
    #[test]
    fn login_runs_the_bearer_token_and_profile_flows() {
        let scripted = Arc::new(ScriptedInteraction::new(&["bearer-token", "bedrock-token"]));
        let credential = futures::executor::block_on(
            BedrockAuth
                .login(interaction(&scripted))
                .expect("bedrock auth has a login"),
        )
        .unwrap();
        assert_eq!(credential.key.as_deref(), Some("bedrock-token"));
        assert_eq!(credential.env, None);

        let scripted = Arc::new(ScriptedInteraction::new(&["aws-profile", "work"]));
        let credential =
            futures::executor::block_on(BedrockAuth.login(interaction(&scripted)).unwrap())
                .unwrap();
        assert_eq!(credential.key, None);
        let env = credential.env.unwrap();
        assert_eq!(env.get(AWS_PROFILE).map(String::as_str), Some("work"));
        // The credential-chain notice fires with its link (upstream expects
        // the info event with the "AWS credential provider chain" link).
        match scripted.recorded_events().as_slice() {
            [AuthEvent::Info {
                links: Some(links), ..
            }] => {
                assert_eq!(
                    links[0].label.as_deref(),
                    Some("AWS credential provider chain")
                );
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    /// The credential-chain choice stores nothing (ambient resolution only).
    #[test]
    fn login_credential_chain_stores_an_empty_credential() {
        let scripted = Arc::new(ScriptedInteraction::new(&["credential-chain", ""]));
        let credential =
            futures::executor::block_on(BedrockAuth.login(interaction(&scripted)).unwrap())
                .unwrap();
        assert_eq!(credential.key, None);
        assert_eq!(credential.env, None);
    }

    /// Upstream providers.test.ts "reports bedrock as configured from ambient
    /// AWS credentials without an api key" (providers.test.ts:279-291), plus
    /// the remaining chain order.
    #[test]
    fn resolve_walks_the_ambient_chain_in_order() {
        // AWS_PROFILE (ambient) -> auth {}, source "AWS_PROFILE".
        let ctx = FakeAuthContext::env(&[(AWS_PROFILE, "dev")]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.auth, ModelAuth::default());
        assert_eq!(result.source.as_deref(), Some("AWS_PROFILE"));
        assert_eq!(result.env, None);

        // Nothing ambient -> unconfigured.
        let bare = FakeAuthContext::env(&[]);
        assert_eq!(resolve_with(&bare, None).unwrap(), None);

        // Bearer token beats the profile.
        let ctx =
            FakeAuthContext::env(&[("AWS_BEARER_TOKEN_BEDROCK", "bearer"), (AWS_PROFILE, "dev")]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.source.as_deref(), Some("AWS_BEARER_TOKEN_BEDROCK"));

        // A stored profile env wins the source label and passes its env
        // through (upstream `toMatchObject({ auth: {}, env: { AWS_PROFILE ... } })`).
        // The ambient bearer env is gone here: it is checked before profiles,
        // so its presence would keep the source "AWS_BEARER_TOKEN_BEDROCK".
        let profile_ctx = FakeAuthContext::env(&[(AWS_PROFILE, "ambient")]);
        let credential = env_credential(&[(AWS_PROFILE, "work")]);
        let result = resolve_with(&profile_ctx, Some(&credential))
            .unwrap()
            .unwrap();
        assert_eq!(result.auth, ModelAuth::default());
        assert_eq!(result.source.as_deref(), Some("stored credential"));
        assert_eq!(
            result
                .env
                .as_ref()
                .unwrap()
                .get(AWS_PROFILE)
                .map(String::as_str),
            Some("work")
        );

        // IAM access keys.
        let ctx = FakeAuthContext::env(&[
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
        ]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.source.as_deref(), Some("AWS access keys"));

        // ECS and web identity sources.
        for (name, source) in [
            ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "ECS task role"),
            ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "ECS task role"),
            ("AWS_WEB_IDENTITY_TOKEN_FILE", "web identity token"),
        ] {
            let ctx = FakeAuthContext::env(&[(name, "value")]);
            let result = resolve_with(&ctx, None).unwrap().unwrap();
            assert_eq!(result.source.as_deref(), Some(source), "{name}");
        }

        // A stored bearer key wins over everything ambient.
        let ctx =
            FakeAuthContext::env(&[("AWS_BEARER_TOKEN_BEDROCK", "bearer"), (AWS_PROFILE, "dev")]);
        let credential = Credential::ApiKey(ApiKeyCredential {
            key: Some("stored-token".to_string()),
            env: None,
            extra: Default::default(),
        });
        let result = resolve_with(&ctx, Some(&credential)).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("stored-token"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));
    }

    /// The factory pins: id/name, no baseUrl, Bedrock catalog, converse API.
    #[test]
    fn factory_builds_the_bedrock_provider() {
        let provider = amazon_bedrock_provider();
        assert_eq!(provider.id(), "amazon-bedrock");
        assert_eq!(provider.name(), "Amazon Bedrock");
        assert_eq!(provider.base_url(), None);
        assert!(!provider.get_models().unwrap().is_empty());
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("AWS credentials or bearer token")
        );
        assert!(provider.auth().oauth.is_none());
        let model = provider.get_models().unwrap()[0].clone();
        assert!(provider.api_for(&model).is_some());
    }
}
