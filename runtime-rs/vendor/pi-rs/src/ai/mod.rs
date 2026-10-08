//! The `pi-ai` surface: full upstream type system (re-exported from
//! [`types`]), auth credentials and stores ([`auth`]), transcript
//! normalization ([`transcript`]), argument validation ([`validation`]),
//! request costing ([`cost`]), and the wire-protocol API implementations
//! ([`api`]) behind the uniform [`ApiImpl`] stream contract.
//!
//! Every role, block, event, and provider request runs on the
//! upstream-compatible types: request bodies derive from the REPLAYED
//! transcript and session JSONL is upstream wire format. One API
//! implementation per wire protocol lives in [`api`] (`openai-completions`,
//! `anthropic-messages`, `openai-responses`); there is no per-endpoint
//! provider state — implementations are unit structs and every request
//! receives its [`ProviderConfig`] and [`types::Model`] explicitly.

pub mod api;
pub mod auth;
pub mod cli_auth;
pub mod cost;
pub mod estimate;
pub mod frame;
pub mod images;
pub mod json_parse;
pub mod model_operations;
pub mod models;
pub mod models_error;
pub mod overflow;
pub mod retry;
pub mod transcript;
pub mod types;
pub mod uuid;
pub mod validation;

pub use api::{http_client, pi_user_agent, ApiImpl};
pub use auth::*;
pub use cost::calculate_cost;
pub use transcript::{Context, TranscriptContext};
pub use types::*;

/// Current Unix time in milliseconds (upstream message timestamps are
/// `number` millis; `i64` here, `0` if the clock is before the epoch).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Connection details for one provider endpoint, passed to every
/// [`ApiImpl::stream`] call. Who is answering rides on the [`types::Model`]
/// (`provider`/`id` stamp `AssistantMessage.provider`/`model`), so one
/// endpoint config can serve any model.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: u64,
}
