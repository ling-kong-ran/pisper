//! Auth surface ported from upstream `packages/ai/src/auth/` plus
//! `env-api-keys.ts`: the type-tagged [`Credential`] wire format
//! (`types`), app-owned credential storage (`credential_store`), provider
//! API-key env discovery (`env_api_keys`), the `Models`-shared resolution
//! engine (`resolve`), the default env/file auth context (`context`), and
//! the standard provider auth helpers (`helpers`).
//!
//! auth.json wire-compat is a hard goal: [`types::Credential`] round-trips
//! upstream-written files byte-for-byte, including unknown extension fields,
//! and [`file_store::FileCredentialStore`] persists `{providerId: credential}`
//! documents pretty-printed like upstream `saveAuth`. Interactive auth goes
//! through [`types::AuthInteraction`] — flows never touch stdio or a browser
//! directly (M2d controller ruling) — and Models-collection integration
//! lands in M2e.
//!
//! Environment variables here are the process env plus the scoped
//! [`crate::ai::types::ProviderEnv`] overrides, resolved by
//! [`crate::ai::api::azure_openai_responses::get_provider_env_value`]
//! (upstream `getProviderEnvValue`).

pub mod context;
pub mod credential_store;
pub mod env_api_keys;
pub mod file_store;
/// Port-only module (no upstream `auth/` counterpart): the Vertex ADC
/// service-account token minting behind `google_vertex`'s ADC branch
/// (google-auth-library JWT/gtoken semantics). Internal surface.
pub(crate) mod google_adc;
pub mod helpers;
pub mod oauth;
pub mod resolve;
pub mod types;

pub use context::{default_provider_auth_context, DefaultAuthContext};
pub use credential_store::{CredentialStore, InMemoryCredentialStore, ModifyCallback};
pub use env_api_keys::{
    find_env_keys, get_api_key_env_vars, get_env_api_key, ANTHROPIC_API_KEY_ENV,
    ANTHROPIC_AUTH_TOKEN_ENV, ANTHROPIC_OAUTH_TOKEN_ENV,
};
pub use file_store::FileCredentialStore;
pub use helpers::{env_api_key_auth, lazy_oauth, EnvApiKeyAuth, LazyOAuth, OAuthLoader};
pub use oauth::{AnthropicOAuth, GitHubCopilotOAuth, OpenAICodexOAuth};
pub use resolve::{
    resolve_provider_auth, AuthResolutionOverrides, ModelsError, ModelsErrorCode,
    DEFAULT_OAUTH_MINIMUM_VALIDITY_MS, DEFAULT_OAUTH_REFRESH_TIMEOUT_MS,
};
pub use types::*;
