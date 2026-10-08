//! Environment API-key discovery ported from upstream
//! `packages/ai/src/env-api-keys.ts`: the provider-to-env-var table, the
//! scoped and process-env lookup ([`find_env_keys`]), and the API-key
//! resolver ([`get_env_api_key`]) with the Google Vertex ADC and Amazon
//! Bedrock ambient fallbacks.
//!
//! Only actual API-key variables are reported; ambient credential sources
//! such as AWS profiles, AWS IAM credentials, and Google Application Default
//! Credentials are intentionally excluded from the table and handled by the
//! dedicated `"<authenticated>"` fallbacks.

use std::sync::OnceLock;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::types::ProviderEnv;

pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
pub const ANTHROPIC_OAUTH_TOKEN_ENV: &str = "ANTHROPIC_OAUTH_TOKEN";
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// Upstream workload identity federation variables (v1.0.0,
/// `env-api-keys.ts:32-36`): the SDK exchanges the identity token for a
/// short-lived access token and refreshes it itself. Last in line so keys and
/// `ANTHROPIC_AUTH_TOKEN` keep winning, as in the SDK.
pub const ANTHROPIC_FEDERATION_RULE_ID_ENV: &str = "ANTHROPIC_FEDERATION_RULE_ID";
pub const ANTHROPIC_ORGANIZATION_ID_ENV: &str = "ANTHROPIC_ORGANIZATION_ID";
pub const ANTHROPIC_SERVICE_ACCOUNT_ID_ENV: &str = "ANTHROPIC_SERVICE_ACCOUNT_ID";
pub const ANTHROPIC_IDENTITY_TOKEN_FILE_ENV: &str = "ANTHROPIC_IDENTITY_TOKEN_FILE";
pub const ANTHROPIC_WORKSPACE_ID_ENV: &str = "ANTHROPIC_WORKSPACE_ID";

/// The federation config the three required variables produce (upstream
/// `getAnthropicFederation` in `providers/anthropic.ts` +
/// `anthropic-messages.ts`): `organization_id`, optional `workspace_id`, and
/// the OIDC-federation authentication block. `None` when any required
/// variable is missing. Port divergence (disclosed): upstream hands this to
/// the Anthropic SDK, which performs and caches the token exchange; the
/// port's request assembly does not run the SDK, so the exchange itself is
/// not performed — the resolved config is the port's surface for it.
pub fn anthropic_federation_config(
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<serde_json::Value> {
    use serde_json::json;
    let federation_rule_id = lookup(ANTHROPIC_FEDERATION_RULE_ID_ENV)?;
    let organization_id = lookup(ANTHROPIC_ORGANIZATION_ID_ENV)?;
    let identity_token_file = lookup(ANTHROPIC_IDENTITY_TOKEN_FILE_ENV)?;
    let mut authentication = json!({
        "type": "oidc_federation",
        "federation_rule_id": federation_rule_id,
        "identity_token": { "source": "file", "path": identity_token_file },
    });
    if let Some(service_account_id) = lookup(ANTHROPIC_SERVICE_ACCOUNT_ID_ENV) {
        authentication["service_account_id"] = json!(service_account_id);
    }
    let mut config = json!({
        "organization_id": organization_id,
        "authentication": authentication,
    });
    if let Some(workspace_id) = lookup(ANTHROPIC_WORKSPACE_ID_ENV) {
        config["workspace_id"] = json!(workspace_id);
    }
    Some(config)
}

/// Upstream `getApiKeyEnvVars` (env-api-keys.ts:68-120), the full table.
/// `None` for providers without an API-key env var (OAuth-only, ambient, or
/// keyless providers).
pub fn get_api_key_env_vars(provider: &str) -> Option<&'static [&'static str]> {
    if provider == "github-copilot" {
        return Some(&["COPILOT_GITHUB_TOKEN"]);
    }

    // ANTHROPIC_AUTH_TOKEN participates in env discovery/status, but
    // get_env_api_key skips it because requests must pass it as
    // Authorization: Bearer.
    if provider == "anthropic" {
        return Some(&[
            ANTHROPIC_AUTH_TOKEN_ENV,
            ANTHROPIC_OAUTH_TOKEN_ENV,
            ANTHROPIC_API_KEY_ENV,
        ]);
    }

    let env_vars: &'static [&'static str] = match provider {
        "ant-ling" => &["ANT_LING_API_KEY"],
        "qwen-token-plan" => &["QWEN_TOKEN_PLAN_API_KEY"],
        "qwen-token-plan-cn" => &["QWEN_TOKEN_PLAN_CN_API_KEY"],
        "qwen-token-plan-individual" => &["QWEN_TOKEN_PLAN_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "azure-openai-responses" => &["AZURE_OPENAI_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "google" => &["GEMINI_API_KEY"],
        "google-vertex" => &["GOOGLE_CLOUD_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "typesafe" => &["TYPESAFE_API_KEY"],
        "radius" => &["RADIUS_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "vercel-ai-gateway" => &["AI_GATEWAY_API_KEY"],
        "zai" => &["ZAI_API_KEY"],
        "zai-coding-cn" => &["ZAI_CODING_CN_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "minimax" => &["MINIMAX_API_KEY"],
        "minimax-cn" => &["MINIMAX_CN_API_KEY"],
        "moonshotai" => &["MOONSHOT_API_KEY"],
        "moonshotai-cn" => &["MOONSHOT_API_KEY"],
        "huggingface" => &["HF_TOKEN"],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "baseten" => &["BASETEN_API_KEY"],
        "opencode" => &["OPENCODE_API_KEY"],
        "opencode-go" => &["OPENCODE_API_KEY"],
        "kimi-coding" => &["KIMI_API_KEY"],
        "meta" => &["META_API_KEY"],
        "cloudflare-workers-ai" => &["CLOUDFLARE_API_KEY"],
        "cloudflare-ai-gateway" => &["CLOUDFLARE_API_KEY"],
        "xiaomi" => &["XIAOMI_API_KEY"],
        "xiaomi-token-plan-cn" => &["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
        "xiaomi-token-plan-ams" => &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
        "xiaomi-token-plan-sgp" => &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
        _ => return None,
    };
    Some(env_vars)
}

/// Upstream `hasVertexAdcCredentials` (env-api-keys.ts:35-66): whether Vertex
/// Application Default Credentials exist. An explicit
/// `GOOGLE_APPLICATION_CREDENTIALS` in the scoped env is checked on every
/// call; the ambient default path (`~/.config/gcloud/
/// application_default_credentials.json`) is checked once and cached for the
/// process lifetime, like the upstream module-level cache.
fn has_vertex_adc_credentials(env: Option<&ProviderEnv>) -> bool {
    if let Some(explicit) = env
        .and_then(|env| env.get("GOOGLE_APPLICATION_CREDENTIALS"))
        .filter(|value| !value.is_empty())
    {
        return std::path::Path::new(explicit).exists();
    }

    static CACHED_VERTEX_ADC_CREDENTIALS_EXISTS: OnceLock<bool> = OnceLock::new();
    *CACHED_VERTEX_ADC_CREDENTIALS_EXISTS.get_or_init(|| {
        match get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env) {
            Some(path) => std::path::Path::new(&path).exists(),
            // Fall back to the default ADC path.
            None => dirs::home_dir()
                .map(|home| {
                    home.join(".config")
                        .join("gcloud")
                        .join("application_default_credentials.json")
                })
                .is_some_and(|path| path.exists()),
        }
    })
}

/// Upstream `findEnvKeys` (env-api-keys.ts:129-137): the provider's API-key
/// env vars that are actually configured, in table order; `None` when none
/// are set.
pub fn find_env_keys(provider: &str, env: Option<&ProviderEnv>) -> Option<Vec<String>> {
    let env_vars = get_api_key_env_vars(provider)?;
    let found: Vec<String> = env_vars
        .iter()
        .filter(|env_var| get_provider_env_value(env_var, env).is_some())
        .map(|env_var| env_var.to_string())
        .collect();
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Upstream `getEnvApiKey` (env-api-keys.ts:144-188): API key for a provider
/// from its known environment variables. Will not return API keys for
/// providers that require OAuth tokens (`ANTHROPIC_AUTH_TOKEN` is reported by
/// [`find_env_keys`] but skipped here because requests must pass it as
/// `Authorization: Bearer`).
pub fn get_env_api_key(provider: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(env_keys) = find_env_keys(provider, env) {
        let api_key_env = if provider == "anthropic" {
            env_keys.iter().find(|key| *key != ANTHROPIC_AUTH_TOKEN_ENV)
        } else {
            env_keys.first()
        };
        if let Some(api_key_env) = api_key_env {
            return get_provider_env_value(api_key_env, env);
        }
    }

    // Vertex AI supports either an explicit API key or Application Default
    // Credentials. Auth is configured via `gcloud auth application-default
    // login`.
    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(env);
        let has_project = get_provider_env_value("GOOGLE_CLOUD_PROJECT", env).is_some()
            || get_provider_env_value("GCLOUD_PROJECT", env).is_some();
        let has_location = get_provider_env_value("GOOGLE_CLOUD_LOCATION", env).is_some();

        if has_credentials && has_project && has_location {
            return Some("<authenticated>".to_string());
        }
    }

    if provider == "amazon-bedrock" {
        // Amazon Bedrock supports multiple credential sources:
        // 1. AWS_PROFILE - named profile from ~/.aws/credentials
        // 2. AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY - standard IAM keys
        // 3. AWS_BEARER_TOKEN_BEDROCK - Bedrock bearer token
        // 4. AWS_CONTAINER_CREDENTIALS_RELATIVE_URI - ECS task roles
        // 5. AWS_CONTAINER_CREDENTIALS_FULL_URI - ECS task roles (full URI)
        // 6. AWS_WEB_IDENTITY_TOKEN_FILE - IRSA (IAM Roles for Service Accounts)
        let configured = get_provider_env_value("AWS_PROFILE", env).is_some()
            || (get_provider_env_value("AWS_ACCESS_KEY_ID", env).is_some()
                && get_provider_env_value("AWS_SECRET_ACCESS_KEY", env).is_some())
            || get_provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env).is_some()
            || get_provider_env_value("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", env).is_some()
            || get_provider_env_value("AWS_CONTAINER_CREDENTIALS_FULL_URI", env).is_some()
            || get_provider_env_value("AWS_WEB_IDENTITY_TOKEN_FILE", env).is_some();
        if configured {
            return Some("<authenticated>".to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Process env is process-global; serialize env-mutating tests within this
    /// module and restore the saved values on drop (upstream `afterEach`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnv {
        /// Lock the env and clear the given variables for the test.
        fn clearing(vars: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = vars
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            for name in vars {
                std::env::remove_var(name);
            }
            TestEnv { _lock: lock, saved }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn env_var_table_matches_upstream_entry_for_entry() {
        let expected: &[(&str, Option<&[&str]>)] = &[
            ("github-copilot", Some(&["COPILOT_GITHUB_TOKEN"])),
            (
                "anthropic",
                Some(&[
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_OAUTH_TOKEN",
                    "ANTHROPIC_API_KEY",
                ]),
            ),
            ("ant-ling", Some(&["ANT_LING_API_KEY"])),
            ("qwen-token-plan", Some(&["QWEN_TOKEN_PLAN_API_KEY"])),
            ("qwen-token-plan-cn", Some(&["QWEN_TOKEN_PLAN_CN_API_KEY"])),
            (
                "qwen-token-plan-individual",
                Some(&["QWEN_TOKEN_PLAN_API_KEY"]),
            ),
            ("openai", Some(&["OPENAI_API_KEY"])),
            ("azure-openai-responses", Some(&["AZURE_OPENAI_API_KEY"])),
            ("nvidia", Some(&["NVIDIA_API_KEY"])),
            ("deepseek", Some(&["DEEPSEEK_API_KEY"])),
            ("google", Some(&["GEMINI_API_KEY"])),
            ("google-vertex", Some(&["GOOGLE_CLOUD_API_KEY"])),
            ("groq", Some(&["GROQ_API_KEY"])),
            ("cerebras", Some(&["CEREBRAS_API_KEY"])),
            ("xai", Some(&["XAI_API_KEY"])),
            ("typesafe", Some(&["TYPESAFE_API_KEY"])),
            ("radius", Some(&["RADIUS_API_KEY"])),
            ("openrouter", Some(&["OPENROUTER_API_KEY"])),
            ("vercel-ai-gateway", Some(&["AI_GATEWAY_API_KEY"])),
            ("zai", Some(&["ZAI_API_KEY"])),
            ("zai-coding-cn", Some(&["ZAI_CODING_CN_API_KEY"])),
            ("mistral", Some(&["MISTRAL_API_KEY"])),
            ("minimax", Some(&["MINIMAX_API_KEY"])),
            ("minimax-cn", Some(&["MINIMAX_CN_API_KEY"])),
            ("moonshotai", Some(&["MOONSHOT_API_KEY"])),
            ("moonshotai-cn", Some(&["MOONSHOT_API_KEY"])),
            ("huggingface", Some(&["HF_TOKEN"])),
            ("fireworks", Some(&["FIREWORKS_API_KEY"])),
            ("together", Some(&["TOGETHER_API_KEY"])),
            ("baseten", Some(&["BASETEN_API_KEY"])),
            ("opencode", Some(&["OPENCODE_API_KEY"])),
            ("opencode-go", Some(&["OPENCODE_API_KEY"])),
            ("kimi-coding", Some(&["KIMI_API_KEY"])),
            ("cloudflare-workers-ai", Some(&["CLOUDFLARE_API_KEY"])),
            ("cloudflare-ai-gateway", Some(&["CLOUDFLARE_API_KEY"])),
            ("xiaomi", Some(&["XIAOMI_API_KEY"])),
            (
                "xiaomi-token-plan-cn",
                Some(&["XIAOMI_TOKEN_PLAN_CN_API_KEY"]),
            ),
            (
                "xiaomi-token-plan-ams",
                Some(&["XIAOMI_TOKEN_PLAN_AMS_API_KEY"]),
            ),
            (
                "xiaomi-token-plan-sgp",
                Some(&["XIAOMI_TOKEN_PLAN_SGP_API_KEY"]),
            ),
            // OAuth-only, ambient, and unknown providers have no entry.
            ("openai-codex", None),
            ("amazon-bedrock", None),
            ("pi-messages", None),
            ("not-a-provider", None),
        ];
        for (provider, expected_vars) in expected {
            assert_eq!(
                get_api_key_env_vars(provider),
                *expected_vars,
                "table mismatch for provider {provider}"
            );
        }
    }

    #[test]
    fn does_not_treat_generic_github_tokens_as_github_copilot_credentials() {
        let _env = TestEnv::clearing(&["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]);
        std::env::set_var("GH_TOKEN", "gh-token");
        std::env::set_var("GITHUB_TOKEN", "github-token");

        assert_eq!(find_env_keys("github-copilot", None), None);
        assert_eq!(get_env_api_key("github-copilot", None), None);
    }

    #[test]
    fn resolves_github_copilot_credentials_from_copilot_github_token() {
        let _env = TestEnv::clearing(&["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]);
        std::env::set_var("COPILOT_GITHUB_TOKEN", "copilot-token");
        std::env::set_var("GH_TOKEN", "gh-token");
        std::env::set_var("GITHUB_TOKEN", "github-token");

        assert_eq!(
            find_env_keys("github-copilot", None),
            Some(vec!["COPILOT_GITHUB_TOKEN".to_string()])
        );
        assert_eq!(
            get_env_api_key("github-copilot", None),
            Some("copilot-token".to_string())
        );
    }

    #[test]
    fn resolves_zai_china_coding_plan_credentials_from_zai_coding_cn_api_key() {
        let _env = TestEnv::clearing(&["ZAI_CODING_CN_API_KEY"]);
        std::env::set_var("ZAI_CODING_CN_API_KEY", "zai-coding-cn-token");

        assert_eq!(
            find_env_keys("zai-coding-cn", None),
            Some(vec!["ZAI_CODING_CN_API_KEY".to_string()])
        );
        assert_eq!(
            get_env_api_key("zai-coding-cn", None),
            Some("zai-coding-cn-token".to_string())
        );
    }

    #[test]
    fn reports_anthropic_auth_token_but_preserves_oauth_token_api_key_lookup() {
        let _env = TestEnv::clearing(&[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ]);
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
        std::env::set_var("ANTHROPIC_OAUTH_TOKEN", "oauth-token");
        std::env::set_var("ANTHROPIC_API_KEY", "api-key");

        assert_eq!(
            find_env_keys("anthropic", None),
            Some(vec![
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                "ANTHROPIC_OAUTH_TOKEN".to_string(),
                "ANTHROPIC_API_KEY".to_string()
            ])
        );
        assert_eq!(
            get_env_api_key("anthropic", None),
            Some("oauth-token".to_string())
        );
    }

    #[test]
    fn does_not_return_anthropic_auth_token_as_an_api_key() {
        let _env = TestEnv::clearing(&[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ]);
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");

        assert_eq!(
            find_env_keys("anthropic", None),
            Some(vec!["ANTHROPIC_AUTH_TOKEN".to_string()])
        );
        assert_eq!(get_env_api_key("anthropic", None), None);
    }

    #[test]
    fn preserves_anthropic_oauth_token_as_an_api_key() {
        let _env = TestEnv::clearing(&[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ]);
        std::env::set_var("ANTHROPIC_OAUTH_TOKEN", "oauth-token");

        assert_eq!(
            find_env_keys("anthropic", None),
            Some(vec!["ANTHROPIC_OAUTH_TOKEN".to_string()])
        );
        assert_eq!(
            get_env_api_key("anthropic", None),
            Some("oauth-token".to_string())
        );
    }

    #[test]
    fn falls_back_to_anthropic_api_key_for_api_key_lookup() {
        let _env = TestEnv::clearing(&[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ]);
        std::env::set_var("ANTHROPIC_API_KEY", "api-key");

        assert_eq!(
            get_env_api_key("anthropic", None),
            Some("api-key".to_string())
        );
    }

    #[test]
    fn scoped_env_wins_over_process_env_and_empty_values_are_unset() {
        let mut scoped = ProviderEnv::new();
        scoped.insert("OPENAI_API_KEY".to_string(), "scoped-key".to_string());
        // Empty scoped values fall through like the upstream `||` chain.
        let mut empty = ProviderEnv::new();
        empty.insert("ZAI_API_KEY".to_string(), String::new());

        assert_eq!(
            find_env_keys("openai", Some(&scoped)),
            Some(vec!["OPENAI_API_KEY".to_string()])
        );
        assert_eq!(
            get_env_api_key("openai", Some(&scoped)),
            Some("scoped-key".to_string())
        );
        assert_eq!(find_env_keys("zai", Some(&empty)), None);
    }

    #[test]
    fn bedrock_reports_authenticated_for_ambient_credential_sources() {
        let _env = TestEnv::clearing(&["AWS_PROFILE", "AWS_WEB_IDENTITY_TOKEN_FILE"]);
        // No ambient source: not configured (scoped env is empty and process
        // env was cleared above).
        assert_eq!(get_env_api_key("amazon-bedrock", None), None);

        let mut scoped = ProviderEnv::new();
        scoped.insert(
            "AWS_WEB_IDENTITY_TOKEN_FILE".to_string(),
            "/var/run/token".to_string(),
        );
        assert_eq!(
            get_env_api_key("amazon-bedrock", Some(&scoped)),
            Some("<authenticated>".to_string())
        );

        let mut scoped = ProviderEnv::new();
        scoped.insert("AWS_PROFILE".to_string(), "default".to_string());
        assert_eq!(
            get_env_api_key("amazon-bedrock", Some(&scoped)),
            Some("<authenticated>".to_string())
        );
    }

    #[test]
    fn google_vertex_requires_adc_credentials_project_and_location() {
        let _env = TestEnv::clearing(&[
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_PROJECT",
            "GCLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            "GOOGLE_CLOUD_API_KEY",
        ]);
        // Explicit-but-missing ADC path (bypasses the module cache, like
        // upstream): not configured even with project and location set.
        let mut scoped = ProviderEnv::new();
        scoped.insert(
            "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
            "/definitely/not/a/real/path.json".to_string(),
        );
        scoped.insert("GOOGLE_CLOUD_PROJECT".to_string(), "proj".to_string());
        scoped.insert(
            "GOOGLE_CLOUD_LOCATION".to_string(),
            "us-central1".to_string(),
        );
        assert_eq!(get_env_api_key("google-vertex", Some(&scoped)), None);

        // Explicit ADC path that exists (checked fresh on every call, so the
        // module-level cache never poisons this case): authenticated.
        let file = tempfile::Builder::new().tempfile().unwrap();
        scoped.insert(
            "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
            file.path().display().to_string(),
        );
        assert_eq!(
            get_env_api_key("google-vertex", Some(&scoped)),
            Some("<authenticated>".to_string())
        );
    }

    #[test]
    fn google_vertex_prefers_an_explicit_api_key_over_adc() {
        let mut scoped = ProviderEnv::new();
        scoped.insert("GOOGLE_CLOUD_API_KEY".to_string(), "vertex-key".to_string());
        assert_eq!(
            get_env_api_key("google-vertex", Some(&scoped)),
            Some("vertex-key".to_string())
        );
    }
}
