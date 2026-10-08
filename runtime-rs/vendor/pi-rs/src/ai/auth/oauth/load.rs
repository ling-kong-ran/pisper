//! OAuth flow loader registry ported from upstream
//! `packages/ai/src/auth/oauth/load.ts`: one factory per login flow.
//!
//! Port notes (disclosed divergences):
//! - Upstream loads each flow through a variable dynamic `import()` so
//!   bundlers cannot follow the import into Node-only flow code
//!   (`node:http` callback servers, `node:crypto` PKCE), plus a
//!   `registerBundledOAuthFlowLoaders` registry for standalone Bun binaries.
//!   Rust has no browser bundle target and links everything statically, so
//!   the loader is a plain factory registry: each function constructs the
//!   flow it names and there is no bundled-loader override.

use std::sync::Arc;

use super::anthropic::AnthropicOAuth;
use super::github_copilot::GitHubCopilotOAuth;
use super::kimi_coding::KimiCodingOAuth;
use super::meta::MetaOAuth;
use super::openai_chatgpt::OpenAIChatGptOAuth;
use super::openai_codex::OpenAICodexOAuth;
use super::openrouter::OpenRouterOAuth;
use super::radius::{create_radius_oauth, RadiusOAuthOptions};
use super::xai::XaiOAuth;
use crate::ai::auth::types::OAuthAuth;

/// Upstream `loadAnthropicOAuth` (load.ts:31-34).
pub fn load_anthropic_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(AnthropicOAuth::new())
}

/// Upstream `loadOpenAICodexOAuth` (load.ts:36-39).
pub fn load_openai_codex_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(OpenAICodexOAuth::new())
}

/// Upstream `loadOpenAIChatGPTOAuth` (load.ts:43-47, the Sign-in-with-
/// ChatGPT delta): the flow constructs like its sibling loaders.
pub fn load_openai_chatgpt_oauth() -> Result<Arc<dyn OAuthAuth>, String> {
    Ok(Arc::new(OpenAIChatGptOAuth::new()))
}

/// Upstream `loadMetaOAuth` (load.ts:63-67, the Meta Muse delta).
pub fn load_meta_oauth() -> Result<Arc<dyn OAuthAuth>, String> {
    Ok(Arc::new(MetaOAuth::new()))
}

/// Upstream `loadGitHubCopilotOAuth` (load.ts:41-44).
pub fn load_github_copilot_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(GitHubCopilotOAuth::new())
}

/// Upstream `loadOpenRouterOAuth` (load.ts:46-49).
pub fn load_openrouter_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(OpenRouterOAuth::new())
}

/// Upstream `loadKimiCodingOAuth` (load.ts:51-54).
pub fn load_kimi_coding_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(KimiCodingOAuth::new())
}

/// Upstream `loadXaiOAuth` (load.ts:56-59).
pub fn load_xai_oauth() -> Arc<dyn OAuthAuth> {
    Arc::new(XaiOAuth::new())
}

/// Upstream `loadRadiusOAuth` (load.ts:61-68): the Radius flow is created
/// per provider from its display name and gateway URL.
pub fn load_radius_oauth(options: RadiusOAuthOptions) -> Arc<dyn OAuthAuth> {
    Arc::new(create_radius_oauth(options))
}

/// Provider ids with an OAuth login flow, in the upstream `builtinProviders()`
/// filter order (providers/*.ts `auth.oauth` presence): upstream cli.ts
/// derives its login surface from the same filter.
pub const OAUTH_LOGIN_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "openai-codex",
    "github-copilot",
    "openrouter",
    "meta",
    "xai",
    "kimi-coding",
    "radius",
];

/// Build the OAuth login flow for a provider id (upstream cli.ts resolves
/// `provider.auth.oauth` off the builtin provider record; the port's
/// providers are not first-class values yet, so the registry dispatches by
/// id). Radius uses the upstream default gateway
/// (`radius-config.ts` `DEFAULT_RADIUS_GATEWAY`). `None` = no OAuth flow
/// for the id.
pub fn oauth_flow_for(provider_id: &str) -> Option<Arc<dyn OAuthAuth>> {
    match provider_id {
        "anthropic" => Some(load_anthropic_oauth()),
        "openai" => load_openai_chatgpt_oauth().ok(),
        "meta" => load_meta_oauth().ok(),
        "openai-codex" => Some(load_openai_codex_oauth()),
        "github-copilot" => Some(load_github_copilot_oauth()),
        "openrouter" => Some(load_openrouter_oauth()),
        "xai" => Some(load_xai_oauth()),
        "kimi-coding" => Some(load_kimi_coding_oauth()),
        "radius" => Some(load_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            // providers/radius-config.ts:4 DEFAULT_RADIUS_GATEWAY.
            gateway: "https://radius.pi.dev".to_string(),
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every loader constructs the flow it names (upstream resolves the
    /// module export by the same name).
    #[test]
    fn loaders_construct_their_flows() {
        assert_eq!(load_anthropic_oauth().name(), "Anthropic (Claude Pro/Max)");
        assert_eq!(
            load_openai_codex_oauth().name(),
            "OpenAI (ChatGPT Plus/Pro)"
        );
        assert_eq!(
            load_openai_chatgpt_oauth().unwrap().name(),
            "OpenAI (ChatGPT subscription)"
        );
        assert_eq!(
            load_meta_oauth().unwrap().name(),
            "Meta (Muse subscription)"
        );
        assert_eq!(load_github_copilot_oauth().name(), "GitHub Copilot");
        assert_eq!(load_openrouter_oauth().name(), "OpenRouter OAuth");
        assert_eq!(load_kimi_coding_oauth().name(), "Kimi Code (subscription)");
        assert_eq!(load_xai_oauth().name(), "xAI (Grok/X subscription)");
        assert_eq!(
            load_radius_oauth(RadiusOAuthOptions {
                name: "Radius".to_string(),
                gateway: "https://radius.example".to_string(),
            })
            .name(),
            "Radius"
        );
    }

    /// The login registry covers exactly the advertised ids and rejects
    /// everything else (upstream cli.ts `PROVIDERS.some(...) == providerId`).
    #[test]
    fn oauth_flow_registry_matches_the_advertised_providers() {
        for provider in OAUTH_LOGIN_PROVIDERS {
            assert!(
                oauth_flow_for(provider).is_some(),
                "{provider} should dispatch"
            );
        }
        for absent in ["openai-compat", "google", "amazon-bedrock", "nope", ""] {
            assert!(oauth_flow_for(absent).is_none(), "{absent}");
        }
    }
}
