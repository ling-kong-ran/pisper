//! Upstream `providers/google-vertex.ts`: the Vertex factory. Vertex accepts
//! an explicit API key or Application Default Credentials
//! (`gcloud auth application-default login`); ADC additionally requires
//! project and location env vars, which the Google Vertex API impl reads
//! itself — so a successful ADC resolve carries `auth: {}` plus the source
//! label, and never copies ambient env into the result.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::ai::api::google_vertex::GoogleVertex;
use crate::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthEvent, AuthInfoLink,
    AuthInteraction as _, AuthPrompt, AuthPromptKind, AuthPromptOption, AuthResult, ModelAuth,
    ProviderAuth, ProviderAuthInteraction,
};
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::Provider;

/// Upstream `VERTEX_ADC_PATH` (google-vertex.ts:9).
const VERTEX_ADC_PATH: &str = "~/.config/gcloud/application_default_credentials.json";

fn aborted(interaction: &ProviderAuthInteraction) -> Option<AuthError> {
    interaction
        .signal
        .is_cancelled()
        .then_some(AuthError::Cancelled)
}

/// The upstream resolve's local `env` helper (google-vertex.ts:51-56): an
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

/// The credential.env value for `name`, if set.
fn stored_env<'a>(input: &ApiKeyAuthInput<'a>, name: &str) -> Option<String> {
    input
        .credential
        .and_then(|credential| credential.env.as_ref())
        .and_then(|env| env.get(name))
        .filter(|value| !value.is_empty())
        .cloned()
}

/// Upstream `vertexAuth` (google-vertex.ts:12-100).
struct VertexAuth;

impl ApiKeyAuth for VertexAuth {
    fn name(&self) -> &str {
        "Google Cloud credentials"
    }

    /// Upstream google-vertex.ts:15-57: a select over API key / ADC /
    /// service-account, then the shared project/location prompts.
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
                        message: "Select Google Vertex AI authentication method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: "api-key".to_string(),
                                label: "Google Cloud API key".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "adc".to_string(),
                                label: "Application Default Credentials".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "service-account".to_string(),
                                label: "Service account credentials file".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            if method == "api-key" {
                let key = interaction
                    .prompt(AuthPrompt {
                        signal: None,
                        kind: AuthPromptKind::Secret {
                            message: "Enter Google Cloud API key".to_string(),
                            placeholder: None,
                        },
                    })
                    .await?;
                if let Some(error) = aborted(&interaction) {
                    return Err(error);
                }
                return Ok(ApiKeyCredential {
                    key: Some(key),
                    env: None,
                    extra: Default::default(),
                });
            }
            if method != "adc" && method != "service-account" {
                return Err(AuthError::Operation(format!(
                    "Unknown Google Vertex AI auth method: {method}"
                )));
            }
            interaction.notify(AuthEvent::Info {
                message: if method == "adc" {
                    "Run `gcloud auth application-default login`, then provide the project and location."
                        .to_string()
                } else {
                    "Provide a service account credentials file, project, and location.".to_string()
                },
                links: Some(vec![AuthInfoLink {
                    label: Some("Application Default Credentials".to_string()),
                    url: "https://cloud.google.com/docs/authentication/provide-credentials-adc"
                        .to_string(),
                }]),
            });
            let project = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Google Cloud project ID".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let location = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Google Cloud location".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let credentials_path = if method == "service-account" {
                Some(
                    interaction
                        .prompt(AuthPrompt {
                            signal: None,
                            kind: AuthPromptKind::Text {
                                message: "Enter service account credentials file path".to_string(),
                                placeholder: None,
                            },
                        })
                        .await?,
                )
            } else {
                None
            };
            if let Some(error) = aborted(&interaction) {
                return Err(error);
            }
            let mut env = crate::ai::types::ProviderEnv::new();
            env.insert("GOOGLE_CLOUD_PROJECT".to_string(), project);
            env.insert("GOOGLE_CLOUD_LOCATION".to_string(), location);
            if let Some(credentials_path) = credentials_path {
                env.insert(
                    "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                    credentials_path,
                );
            }
            Ok(ApiKeyCredential {
                key: None,
                env: Some(env),
                extra: Default::default(),
            })
        }))
    }

    /// Upstream google-vertex.ts:58-99: explicit key first, then ADC with
    /// project and location.
    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            // `credential?.key ?? await env(...)` — an empty stored key stays
            // an empty key and fails the truthiness check like upstream.
            let key = match input
                .credential
                .and_then(|credential| credential.key.clone())
            {
                Some(key) => Some(key),
                None => env_read(&input, "GOOGLE_CLOUD_API_KEY").await?,
            };
            if let Some(key) = key.filter(|key| !key.is_empty()) {
                let from_credential = input
                    .credential
                    .and_then(|credential| credential.key.as_deref())
                    .is_some_and(|key| !key.is_empty());
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: Some(
                        if from_credential {
                            "stored credential"
                        } else {
                            "GOOGLE_CLOUD_API_KEY"
                        }
                        .to_string(),
                    ),
                }));
            }

            let adc_path = match stored_env(&input, "GOOGLE_APPLICATION_CREDENTIALS") {
                Some(path) => path,
                None => env_read(&input, "GOOGLE_APPLICATION_CREDENTIALS")
                    .await?
                    .unwrap_or_else(|| VERTEX_ADC_PATH.to_string()),
            };
            input.options.check()?;
            let has_credentials = input.ctx.file_exists(&adc_path).await;
            input.options.check()?;
            let project = match stored_env(&input, "GOOGLE_CLOUD_PROJECT") {
                Some(project) => Some(project),
                None => match env_read(&input, "GOOGLE_CLOUD_PROJECT").await? {
                    Some(project) => Some(project),
                    None => env_read(&input, "GCLOUD_PROJECT").await?,
                },
            };
            let location = match stored_env(&input, "GOOGLE_CLOUD_LOCATION") {
                Some(location) => Some(location),
                None => env_read(&input, "GOOGLE_CLOUD_LOCATION").await?,
            };
            // Truthiness on the strings, like upstream.
            let configured = has_credentials
                && project
                    .as_deref()
                    .is_some_and(|project| !project.is_empty())
                && location
                    .as_deref()
                    .is_some_and(|location| !location.is_empty());
            if configured {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: input
                        .credential
                        .and_then(|credential| credential.env.clone()),
                    source: Some(
                        if input.credential.is_some() {
                            "stored credential"
                        } else {
                            "gcloud application default credentials"
                        }
                        .to_string(),
                    ),
                }));
            }
            Ok(None)
        })
    }
}

/// Upstream `googleVertexProvider` (google-vertex.ts:102-111).
pub fn google_vertex_provider() -> Arc<dyn Provider> {
    create_provider(CreateProviderOptions {
        id: "google-vertex".to_string(),
        name: Some("Google Vertex AI".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(VertexAuth)),
            oauth: None,
        },
        models: embedded_provider_catalog("google-vertex")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: ApiImpls::Single(Arc::new(GoogleVertex)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::{AuthOperationOptions, Credential};
    use crate::ai::models::providers::test_support::{
        env_credential, file_context, interaction, FakeAuthContext, ScriptedInteraction,
    };

    fn resolve_with(
        ctx: &dyn crate::ai::auth::types::AuthContext,
        credential: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let options = AuthOperationOptions::NONE;
        let api_credential = credential.map(|credential| match credential {
            Credential::ApiKey(credential) => credential,
            Credential::OAuth(_) => panic!("vertex tests use api-key credentials"),
        });
        futures::executor::block_on(VertexAuth.resolve(ApiKeyAuthInput {
            ctx,
            credential: api_credential,
            options: &options,
        }))
    }

    /// Upstream providers.test.ts "runs provider-owned Vertex API key and ADC
    /// login flows" (providers.test.ts:338-380).
    #[test]
    fn login_runs_the_api_key_and_adc_flows() {
        let scripted = Arc::new(ScriptedInteraction::new(&["api-key", "vertex-key"]));
        let credential = futures::executor::block_on(
            VertexAuth
                .login(interaction(&scripted))
                .expect("vertex auth has a login"),
        )
        .unwrap();
        assert_eq!(credential.key.as_deref(), Some("vertex-key"));
        assert_eq!(credential.env, None);

        let scripted = Arc::new(ScriptedInteraction::new(&[
            "adc",
            "project-id",
            "us-central1",
        ]));
        let credential =
            futures::executor::block_on(VertexAuth.login(interaction(&scripted)).unwrap()).unwrap();
        assert_eq!(credential.key, None);
        let env = credential.env.unwrap();
        assert_eq!(
            env.get("GOOGLE_CLOUD_PROJECT").map(String::as_str),
            Some("project-id")
        );
        assert_eq!(
            env.get("GOOGLE_CLOUD_LOCATION").map(String::as_str),
            Some("us-central1")
        );
        assert_eq!(env.get("GOOGLE_APPLICATION_CREDENTIALS"), None);
        match scripted.recorded_events().as_slice() {
            [AuthEvent::Info {
                links: Some(links), ..
            }] => {
                assert_eq!(
                    links[0].label.as_deref(),
                    Some("Application Default Credentials")
                );
            }
            other => panic!("unexpected events: {other:?}"),
        }
    }

    /// The service-account variant also prompts for the credentials file.
    #[test]
    fn login_service_account_prompts_for_the_credentials_file() {
        let scripted = Arc::new(ScriptedInteraction::new(&[
            "service-account",
            "proj",
            "us-central1",
            "/path/sa.json",
        ]));
        let credential =
            futures::executor::block_on(VertexAuth.login(interaction(&scripted)).unwrap()).unwrap();
        let env = credential.env.unwrap();
        assert_eq!(
            env.get("GOOGLE_APPLICATION_CREDENTIALS")
                .map(String::as_str),
            Some("/path/sa.json")
        );
    }

    /// Upstream providers.test.ts "resolves vertex via ADC file plus project
    /// and location" (providers.test.ts:382-403).
    #[test]
    fn resolves_via_adc_file_plus_project_and_location() {
        let adc = VERTEX_ADC_PATH;
        let ctx = file_context(&[adc]).with_env(&[
            ("GOOGLE_CLOUD_PROJECT", "proj"),
            ("GOOGLE_CLOUD_LOCATION", "us-central1"),
        ]);
        let result = resolve_with(&ctx, None).unwrap().unwrap();
        assert_eq!(result.auth, ModelAuth::default());
        assert!(
            result
                .source
                .as_deref()
                .unwrap()
                .contains("application default"),
            "{result:?}"
        );
        assert_eq!(result.env, None);

        // ADC without location is not configured.
        let partial = file_context(&[adc]).with_env(&[("GOOGLE_CLOUD_PROJECT", "proj")]);
        assert_eq!(resolve_with(&partial, None).unwrap(), None);

        // An explicit key wins over ADC.
        let keyed = FakeAuthContext::env(&[("GOOGLE_CLOUD_API_KEY", "vertex-key")]);
        let result = resolve_with(&keyed, None).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("vertex-key"));
        assert_eq!(result.source.as_deref(), Some("GOOGLE_CLOUD_API_KEY"));
    }

    /// Stored credentials: a key resolves as the stored credential; an env
    /// credential resolves ADC from the stored env and labels the source
    /// "stored credential" (upstream google-vertex.ts:90-97).
    #[test]
    fn stored_credentials_drive_adc_and_key_sources() {
        let adc = VERTEX_ADC_PATH;
        let ctx = file_context(&[adc]);
        let credential = env_credential(&[
            ("GOOGLE_CLOUD_PROJECT", "proj"),
            ("GOOGLE_CLOUD_LOCATION", "us-central1"),
        ]);
        let result = resolve_with(&ctx, Some(&credential)).unwrap().unwrap();
        assert_eq!(result.auth, ModelAuth::default());
        assert_eq!(result.source.as_deref(), Some("stored credential"));
        // The stored env passes through verbatim.
        assert_eq!(
            result
                .env
                .as_ref()
                .unwrap()
                .get("GOOGLE_CLOUD_PROJECT")
                .map(String::as_str),
            Some("proj")
        );

        // A stored key beats ambient env.
        let ctx = FakeAuthContext::env(&[("GOOGLE_CLOUD_API_KEY", "ambient")]);
        let credential = Credential::ApiKey(ApiKeyCredential {
            key: Some("stored".to_string()),
            env: None,
            extra: Default::default(),
        });
        let result = resolve_with(&ctx, Some(&credential)).unwrap().unwrap();
        assert_eq!(result.auth.api_key.as_deref(), Some("stored"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));
    }

    /// The factory pins: id/name, no baseUrl (the API impl resolves regional
    /// endpoints), Vertex catalog, vertex API.
    #[test]
    fn factory_builds_the_vertex_provider() {
        let provider = google_vertex_provider();
        assert_eq!(provider.id(), "google-vertex");
        assert_eq!(provider.name(), "Google Vertex AI");
        assert_eq!(provider.base_url(), None);
        assert!(!provider.get_models().unwrap().is_empty());
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("Google Cloud credentials")
        );
        assert!(provider.auth().oauth.is_none());
        let model = provider.get_models().unwrap()[0].clone();
        assert!(provider.api_for(&model).is_some());
    }
}
