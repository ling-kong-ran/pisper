//! Shared test doubles for the provider-factory suites (upstream
//! providers.test.ts `fakeAuthContext` and the scripted login interaction).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::ai::auth::types::{AuthContext, AuthError, AuthEvent, AuthPrompt, Credential};
use crate::ai::types::ProviderEnv;

/// Upstream `fakeAuthContext(env, files)` (providers.test.ts:25-30).
pub(crate) struct FakeAuthContext {
    env: BTreeMap<String, String>,
    files: Vec<String>,
}

impl FakeAuthContext {
    pub(crate) fn env(vars: &[(&str, &str)]) -> Self {
        FakeAuthContext {
            env: vars
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            files: Vec::new(),
        }
    }

    pub(crate) fn with_files(mut self, files: &[&str]) -> Self {
        self.files = files.iter().map(|file| (*file).to_string()).collect();
        self
    }

    pub(crate) fn with_env(mut self, vars: &[(&str, &str)]) -> Self {
        for (name, value) in vars {
            self.env.insert((*name).to_string(), (*value).to_string());
        }
        self
    }
}

impl AuthContext for FakeAuthContext {
    fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.env.get(name).cloned() })
    }

    fn file_exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.files.iter().any(|file| file == path) })
    }
}

/// Auth-input context from a plain map, for direct `ApiKeyAuth::resolve`
/// calls.
pub(crate) fn env_context(vars: &[(&str, &str)]) -> FakeAuthContext {
    FakeAuthContext::env(vars)
}

/// An [`AuthContext`] whose file-existence answers are scripted (upstream's
/// `files` argument).
pub(crate) fn file_context(files: &[&str]) -> FakeAuthContext {
    FakeAuthContext::env(&[]).with_files(files)
}

/// A scripted login interaction: prompts pop queued answers in order, events
/// are recorded (upstream tests script `prompt: async () => answers.shift()`).
pub(crate) struct ScriptedInteraction {
    answers: Mutex<VecDeque<&'static str>>,
    pub(crate) events: Mutex<Vec<AuthEvent>>,
    token: CancellationToken,
}

impl ScriptedInteraction {
    pub(crate) fn new(answers: &[&'static str]) -> Self {
        ScriptedInteraction {
            answers: Mutex::new(answers.iter().copied().collect()),
            events: Mutex::new(Vec::new()),
            token: CancellationToken::new(),
        }
    }

    pub(crate) fn recorded_events(&self) -> Vec<AuthEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl crate::ai::auth::types::AuthInteraction for ScriptedInteraction {
    fn signal(&self) -> Option<CancellationToken> {
        Some(self.token.clone())
    }

    fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(async move {
            self.answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .map(str::to_string)
                .ok_or(AuthError::Cancelled)
        })
    }

    fn notify(&self, event: AuthEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

/// Build a normalized [`crate::ai::auth::types::ProviderAuthInteraction`].
pub(crate) fn interaction(
    scripted: &std::sync::Arc<ScriptedInteraction>,
) -> crate::ai::auth::types::ProviderAuthInteraction {
    crate::ai::auth::types::ProviderAuthInteraction::new(
        std::sync::Arc::clone(scripted)
            as std::sync::Arc<dyn crate::ai::auth::types::AuthInteraction>,
        CancellationToken::new(),
    )
}

/// An api-key credential with no key and the given provider env.
pub(crate) fn env_credential(env: &[(&str, &str)]) -> Credential {
    Credential::ApiKey(crate::ai::auth::types::ApiKeyCredential {
        key: None,
        env: Some(
            env.iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<ProviderEnv>(),
        ),
        extra: BTreeMap::new(),
    })
}

/// A minimal catalog-shaped [`Model`] with the given base URL (stand-in for
/// the generated Cloudflare entries with placeholder URLs).
pub(crate) fn gateway_model(base_url: &str) -> crate::ai::types::Model {
    crate::ai::types::Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "gateway-model".to_string(),
        name: "Gateway Model".to_string(),
        api: "openai-completions".to_string(),
        provider: "cloudflare-workers-ai".to_string(),
        base_url: base_url.to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![crate::ai::types::ModelInput::Text],
        cost: crate::ai::types::primitives::ModelCost::default(),
        context_window: 10_000,
        max_tokens: 1_000,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

/// A minimal catalog-shaped [`Model`] with the given provider and api.
pub(crate) fn api_model(provider: &str, api: &str) -> crate::ai::types::Model {
    crate::ai::types::Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        provider: provider.to_string(),
        api: api.to_string(),
        ..gateway_model("https://example.test")
    }
}
