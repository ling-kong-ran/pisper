use anyhow::{bail, Context as AnyhowContext, Result};
use clap::Parser;
use futures::future::BoxFuture;
use pi_rust::agent_core::{builtin_tools, Agent, AgentInitialState, AgentOptions, SessionWriter};
use pi_rust::ai::api::anthropic::AnthropicMessages;
use pi_rust::ai::api::azure_openai_responses::AzureOpenAiResponses;
use pi_rust::ai::api::bedrock::BedrockConverseStream;
use pi_rust::ai::api::google_generative_ai::GoogleGenerativeAi;
use pi_rust::ai::api::google_vertex::GoogleVertex;
use pi_rust::ai::api::mistral::MistralConversations;
use pi_rust::ai::api::openai_codex_responses::OpenAiCodexResponses;
use pi_rust::ai::api::openai_completions::OpenAiCompletions;
use pi_rust::ai::api::openai_responses::OpenAiResponses;
use pi_rust::ai::api::pi_messages::PiMessages;
use pi_rust::ai::auth::credential_store::CredentialStore;
use pi_rust::ai::auth::file_store::FileCredentialStore;
use pi_rust::ai::auth::oauth::load::{oauth_flow_for, OAUTH_LOGIN_PROVIDERS};
use pi_rust::ai::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthInteraction, AuthOperationOptions, AuthResult,
    ModelAuth, OAuthAuth, ProviderAuth,
};
use pi_rust::ai::cli_auth;
use pi_rust::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use pi_rust::ai::models::{create_models, CreateModelsOptions, Models};
use pi_rust::ai::types::Model;
use pi_rust::ai::ApiImpl;
use pi_rust::cli::console_auth::ConsoleAuthInteraction;
use pi_rust::cli::repl;
use pi_rust::config::{
    auth_json_path, build_model, load_config, resolve_api_key_with_auth, Config, PROVIDERS,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "pirs",
    version,
    about = "Minimal coding agent CLI (Rust rewrite of pi)",
    after_help = "Credentials live in auth.json ({providerId: credential}, upstream-pi format), \
                  default <config_dir>/pi-rust/auth.json (Windows: %APPDATA%\\pi-rust\\auth.json, \
                  macOS: ~/Library/Application Support/pi-rust/auth.json, \
                  Linux: ~/.config/pi-rust/auth.json); override with --auth. \
                  OAuth login: pirs login --provider <id>."
)]
struct Args {
    /// Provider id (see PROVIDERS): anthropic, openai-compat, openai-responses,
    /// azure-openai-responses, openai-codex, google, google-vertex, mistral,
    /// amazon-bedrock, pi-messages
    #[arg(long)]
    provider: Option<String>,
    /// Model id, e.g. claude-sonnet-4-5 or glm-4.6
    #[arg(long)]
    model: Option<String>,
    /// API base URL (required for openai-compat/openai-responses/pi-messages)
    #[arg(long)]
    base_url: Option<String>,
    /// API key (overrides auth.json and env resolution)
    #[arg(long)]
    api_key: Option<String>,
    /// auth.json path (default <config_dir>/pi-rust/auth.json)
    #[arg(long)]
    auth: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum Command {
    /// Run an OAuth login flow and store the credential in auth.json
    Login {
        /// OAuth provider id: anthropic, openai-codex, github-copilot,
        /// openrouter, xai, kimi-coding, radius
        #[arg(long)]
        provider: String,
    },
    /// Remove a provider's stored credential from auth.json
    Logout {
        /// Provider id whose stored credential should be removed
        #[arg(long)]
        provider: String,
    },
}

/// One wire protocol per provider id; the API implementations are unit
/// structs, so selection is a plain constructor pick.
///
/// Auth: `--api-key`/auth.json/env resolve through
/// [`resolve_api_key_with_auth`]. OAuth credentials feed two of the wire
/// providers cleanly:
/// - `anthropic`: the `sk-ant-oat` access token is detected natively
///   (Claude Code identity + oauth beta header).
/// - `openai-codex`: the access token is the full JWT the codex API parses
///   for the `chatgpt_account_id` claim.
fn select_api(provider: &str) -> Result<Arc<dyn ApiImpl>> {
    Ok(match provider {
        "anthropic" => Arc::new(AnthropicMessages),
        "openai-compat" => Arc::new(OpenAiCompletions),
        "openai-responses" => Arc::new(OpenAiResponses),
        "azure-openai-responses" => Arc::new(AzureOpenAiResponses),
        "openai-codex" => Arc::new(OpenAiCodexResponses),
        "google" => Arc::new(GoogleGenerativeAi),
        "google-vertex" => Arc::new(GoogleVertex),
        "mistral" => Arc::new(MistralConversations),
        "amazon-bedrock" => Arc::new(BedrockConverseStream),
        "pi-messages" => Arc::new(PiMessages),
        other => bail!("unknown provider: {other}"),
    })
}

/// Api-key auth carrying the key the CLI resolved itself (explicit `--api-key`,
/// then auth.json, then env; the M2e-T7 decision keeps that config-declared
/// flow). The agent core streams through the [`Models`] collection, whose
/// auth resolution must produce the same key the direct `ProviderConfig` call
/// carried before the M3a swap.
struct ResolvedKeyAuth {
    key: String,
}

impl ApiKeyAuth for ResolvedKeyAuth {
    fn name(&self) -> &str {
        "resolved API key"
    }

    fn resolve<'a>(
        &'a self,
        _input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some(self.key.clone()),
                    ..ModelAuth::default()
                },
                ..AuthResult::default()
            }))
        })
    }
}

/// The `Models` collection for one config-declared provider: it serves the
/// hand-declared model through the selected wire implementation and resolves
/// auth to the pre-resolved key. This is the bridge from the M1-era direct
/// `ApiImpl` + `ProviderConfig` construction onto the agent core.
fn build_models(provider_id: &str, api_key: &str, model: Model, api: Arc<dyn ApiImpl>) -> Models {
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(create_provider(CreateProviderOptions {
        id: provider_id.to_string(),
        name: None,
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(ResolvedKeyAuth {
                key: api_key.to_string(),
            })),
            oauth: None,
        },
        models: vec![pi_rust::ai::types::AnyModel::Chat(model)],
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: ApiImpls::Single(api),
        images: pi_rust::ai::models::provider::ImagesImpls::new(),
        classifiers: pi_rust::ai::models::provider::ClassifiersImpls::new(),
    }));
    models
}

/// `pirs login --provider <id>` (upstream cli.ts `login`): dispatch the
/// provider's OAuth flow through the load registry, render prompts and
/// events on the console, persist through the file store.
async fn login_command(provider_id: &str, auth_path: &Path) -> Result<()> {
    let flow = oauth_flow(provider_id)?;
    let store = FileCredentialStore::new(auth_path);
    perform_login(
        provider_id,
        flow.as_ref(),
        &store,
        Arc::new(ConsoleAuthInteraction),
    )
    .await
}

fn oauth_flow(provider_id: &str) -> Result<Arc<dyn OAuthAuth>> {
    oauth_flow_for(provider_id).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown OAuth provider: {provider_id}; expected one of {OAUTH_LOGIN_PROVIDERS:?}"
        )
    })
}

/// The login body, split from [`login_command`] so tests can script the
/// interaction and flow (upstream cli.ts `login(providerId)`).
async fn perform_login(
    provider_id: &str,
    flow: &dyn OAuthAuth,
    store: &dyn CredentialStore,
    interaction: Arc<dyn AuthInteraction>,
) -> Result<()> {
    cli_auth::login(
        flow,
        provider_id,
        store,
        interaction,
        &AuthOperationOptions::default(),
    )
    .await?;
    Ok(())
}

/// `pirs logout --provider <id>`: remove the stored credential (upstream
/// `CredentialStore.delete`; messaging is the CLI surface's).
async fn logout_command(provider_id: &str, auth_path: &Path) -> Result<()> {
    let store = FileCredentialStore::new(auth_path);
    perform_logout(provider_id, &store).await
}

async fn perform_logout(provider_id: &str, store: &dyn CredentialStore) -> Result<()> {
    cli_auth::logout(store, provider_id, &AuthOperationOptions::default()).await?;
    println!("logged out of {provider_id} (auth.json)");
    Ok(())
}

/// The auth.json path for credential operations: `--auth` wins, else the
/// standard <config_dir>/pi-rust/auth.json.
fn resolve_auth_path(auth: Option<&Path>) -> Result<PathBuf> {
    auth.map(Path::to_path_buf)
        .or_else(auth_json_path)
        .context("could not determine the auth.json path; pass --auth")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Auth commands run standalone: no config, provider validation, or
    // session state (upstream cli.ts main dispatches them the same way).
    match &args.command {
        Some(Command::Login { provider }) => {
            return login_command(provider, &resolve_auth_path(args.auth.as_deref())?).await;
        }
        Some(Command::Logout { provider }) => {
            return logout_command(provider, &resolve_auth_path(args.auth.as_deref())?).await;
        }
        None => {}
    }

    let mut cfg: Config = load_config()?;
    if let Some(p) = &args.provider {
        cfg.provider = p.clone();
    }
    if let Some(m) = &args.model {
        cfg.model = m.clone();
    }
    if args.base_url.is_some() {
        cfg.base_url = args.base_url.clone();
    }

    // Validate after merging CLI overrides (a config-only check would reject
    // valid `--base-url` overrides), and before resolving the API key so an
    // unknown provider reports a provider error, not a missing-key error.
    if !PROVIDERS.contains(&cfg.provider.as_str()) {
        bail!(
            "unknown provider '{}'; expected one of {:?}",
            cfg.provider,
            PROVIDERS
        );
    }
    // pi-messages posts to `{base_url}/messages` on the pi server; azure
    // resolves its endpoint from env instead, so only these three require it.
    if matches!(
        cfg.provider.as_str(),
        "openai-compat" | "openai-responses" | "pi-messages"
    ) && cfg.base_url.is_none()
    {
        bail!("{} requires --base-url or base_url in config", cfg.provider);
    }

    // Explicit --api-key > auth.json credential (OAuth credentials refresh
    // through their flow) > env vars (upstream precedence).
    let store = FileCredentialStore::new(resolve_auth_path(args.auth.as_deref())?);
    let key = resolve_api_key_with_auth(
        &cfg.provider,
        args.api_key.as_deref(),
        &store,
        oauth_flow_for(&cfg.provider),
    )
    .await?
    .context(format!(
        "no API key found: pass --api-key, run `pirs login --provider {}` when it has an \
         OAuth flow ({}), set ANTHROPIC_API_KEY (anthropic), GLM_API_KEY/OPENAI_API_KEY \
         (openai-compat/openai-responses), AZURE_OPENAI_API_KEY (azure-openai-responses), \
         GEMINI_API_KEY (google), GOOGLE_CLOUD_API_KEY (google-vertex), or MISTRAL_API_KEY \
         (mistral); openai-codex/amazon-bedrock/pi-messages otherwise need --api-key",
        cfg.provider,
        OAUTH_LOGIN_PROVIDERS.join(", "),
    ))?;

    let model = build_model(&cfg)?;
    let api = select_api(&cfg.provider)?;
    let models = build_models(&cfg.provider, &key, model.clone(), api);

    let agent = Agent::new(
        AgentOptions {
            initial_state: AgentInitialState {
                system_prompt: Some(repl::SYSTEM_PROMPT.to_string()),
                model: Some(model),
                tools: builtin_tools().into_iter().map(Arc::new).collect(),
                ..AgentInitialState::default()
            },
            ..AgentOptions::default()
        },
        Arc::new(models),
    );
    let sessions_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("pi-rust")
        .join("sessions");
    let mut session = SessionWriter::create(&sessions_dir)?;
    println!("session: {}", session.path().display());

    repl::run(&agent, &mut session, &cfg.model).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use pi_rust::ai::auth::types::{
        AuthError, AuthEvent, AuthPrompt, Credential, ModelAuth, OAuthCredential,
    };
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn select_api_maps_each_provider() {
        // The impls are unit structs; pin the selection succeeds for every
        // supported provider (per-protocol behavior is pinned by the
        // api module and agent-on-API integration tests). Provider ids are
        // the upstream KnownProvider strings.
        for provider in PROVIDERS {
            assert!(select_api(provider).is_ok(), "{provider} should map");
        }
    }

    #[test]
    fn select_api_covers_all_ten_arms() {
        // Pin each arm explicitly so a PROVIDERS/select_api drift fails here.
        let expected = [
            "anthropic",
            "openai-compat",
            "openai-responses",
            "azure-openai-responses",
            "openai-codex",
            "google",
            "google-vertex",
            "mistral",
            "amazon-bedrock",
            "pi-messages",
        ];
        assert_eq!(PROVIDERS, expected);
        for provider in expected {
            assert!(select_api(provider).is_ok());
        }
    }

    #[test]
    fn select_api_rejects_unknown_provider() {
        match select_api("nope") {
            Err(err) => assert!(err.to_string().contains("unknown provider"), "{err}"),
            Ok(_) => panic!("expected unknown provider to be rejected"),
        }
    }

    /// The M3a bridge: the config-declared model is served by the collection
    /// and auth resolution produces the pre-resolved key — the same inputs
    /// the pre-swap code passed straight to the ApiImpl as `ProviderConfig`.
    #[test]
    fn build_models_serves_the_config_model_and_resolves_the_pre_resolved_key() {
        let cfg = Config::default();
        let model = build_model(&cfg).unwrap();
        let api = select_api(&cfg.provider).unwrap();
        let models = build_models(&cfg.provider, "resolved-key", model.clone(), api);

        assert_eq!(models.get_model(&cfg.provider, &model.id), Some(model));

        let resolved = futures::executor::block_on(models.get_auth(&cfg.provider, None))
            .unwrap()
            .expect("auth resolves");
        assert_eq!(resolved.auth.api_key.as_deref(), Some("resolved-key"));
    }

    // ---- Task 9: login/logout CLI surface ----

    #[test]
    fn subcommands_parse_with_provider_and_auth_flags() {
        // `pirs login --provider anthropic [--auth path]`.
        let args = Args::try_parse_from(["pirs", "login", "--provider", "anthropic"]).unwrap();
        assert_eq!(
            args.command,
            Some(Command::Login {
                provider: "anthropic".to_string()
            })
        );
        assert!(args.auth.is_none());
        let args = Args::try_parse_from([
            "pirs",
            "--auth",
            "C:\\tmp\\auth.json",
            "logout",
            "--provider",
            "openai-codex",
        ])
        .unwrap();
        assert_eq!(
            args.command,
            Some(Command::Logout {
                provider: "openai-codex".to_string()
            })
        );
        assert_eq!(args.auth.unwrap(), PathBuf::from("C:\\tmp\\auth.json"));

        // No subcommand: the chat REPL path.
        let args = Args::try_parse_from(["pirs", "--provider", "anthropic"]).unwrap();
        assert!(args.command.is_none());
        assert_eq!(args.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn the_auth_path_flag_overrides_the_standard_location() {
        // Without --auth the standard location resolves (or fails cleanly
        // when the platform has no config dir).
        let standard = resolve_auth_path(None);
        if dirs::config_dir().is_some() {
            let path = standard.unwrap();
            assert!(path.ends_with(Path::new("pi-rust").join("auth.json")));
        } else {
            assert!(standard.is_err());
        }
        // With --auth it wins verbatim.
        assert_eq!(
            resolve_auth_path(Some(Path::new("/tmp/other-auth.json"))).unwrap(),
            PathBuf::from("/tmp/other-auth.json")
        );
    }

    /// Scripted interaction: answers every prompt with `answer`, records
    /// notify events.
    struct ScriptedInteraction {
        answer: String,
        events: Mutex<Vec<AuthEvent>>,
    }

    impl AuthInteraction for ScriptedInteraction {
        fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
            None
        }
        fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
            let answer = self.answer.clone();
            Box::pin(async move { Ok(answer) })
        }
        fn notify(&self, event: AuthEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// OAuth flow returning a canned credential.
    struct FakeFlow {
        credential: OAuthCredential,
    }

    impl OAuthAuth for FakeFlow {
        fn name(&self) -> &str {
            "Fake"
        }
        fn login<'a>(
            &'a self,
            _interaction: pi_rust::ai::auth::types::ProviderAuthInteraction,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            Box::pin(async move { Ok(self.credential.clone()) })
        }
        fn refresh<'a>(
            &'a self,
            _credential: OAuthCredential,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            unreachable!("login never refreshes")
        }
        fn to_auth<'a>(
            &'a self,
            credential: OAuthCredential,
        ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
            Box::pin(async move {
                Ok(ModelAuth {
                    api_key: Some(credential.access),
                    ..Default::default()
                })
            })
        }
    }

    fn canned_credential() -> OAuthCredential {
        OAuthCredential {
            refresh: "r".to_string(),
            access: "a".to_string(),
            expires: pi_rust::ai::now_ms() + 3_600_000,
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn login_wiring_stores_the_credential_in_auth_json() {
        let dir = TempDir::new().unwrap();
        let auth_path = dir.path().join("auth.json");
        let store = FileCredentialStore::new(&auth_path);
        let interaction = Arc::new(ScriptedInteraction {
            answer: "typed".to_string(),
            events: Mutex::new(Vec::new()),
        });
        // One credential value for the flow and the assertion (expires is
        // captured once).
        let credential = canned_credential();
        perform_login(
            "anthropic",
            &FakeFlow {
                credential: credential.clone(),
            },
            &store,
            interaction,
        )
        .await
        .unwrap();
        // The stored document is the upstream format.
        let document: std::collections::BTreeMap<String, Credential> =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(
            document.get("anthropic"),
            Some(&Credential::OAuth(credential))
        );
    }

    #[tokio::test]
    async fn logout_wiring_removes_the_stored_entry() {
        let dir = TempDir::new().unwrap();
        let auth_path = dir.path().join("auth.json");
        let store = FileCredentialStore::new(&auth_path);
        let interaction = Arc::new(ScriptedInteraction {
            answer: "typed".to_string(),
            events: Mutex::new(Vec::new()),
        });
        perform_login(
            "anthropic",
            &FakeFlow {
                credential: canned_credential(),
            },
            &store,
            interaction,
        )
        .await
        .unwrap();
        perform_logout("anthropic", &store).await.unwrap();
        let document: std::collections::BTreeMap<String, Credential> =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert!(!document.contains_key("anthropic"));
    }

    #[tokio::test]
    async fn login_command_rejects_unknown_oauth_providers() {
        let dir = TempDir::new().unwrap();
        let error = login_command("nope", &dir.path().join("auth.json"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("unknown OAuth provider: nope"),
            "{error}"
        );
    }
}
