//! Upstream `providers/github-copilot.ts`: the GitHub Copilot factory. Env
//! token (`COPILOT_GITHUB_TOKEN`) or the Copilot OAuth flow, with a
//! `filterModels` policy that narrows the catalog to the model ids the
//! OAuth credential reports as available
//! (`credential.availableModelIds`) — a non-OAuth credential or a missing /
//! malformed id list leaves the catalog untouched.

use std::collections::HashSet;
use std::sync::Arc;

use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::api::openai_completions::OpenAiCompletions;
use crate::ai::api::openai_responses::OpenAiResponses;
use crate::ai::auth::helpers::LazyOAuth;
use crate::ai::auth::oauth::load::load_github_copilot_oauth;
use crate::ai::auth::types::{Credential, ProviderAuth};
use crate::ai::models::catalog::embedded_provider_catalog;
use crate::ai::models::provider::{create_provider, CreateProviderOptions, FilterModelsFn};
use crate::ai::models::providers::{arc, lazy_flow, per_api};
use crate::ai::models::Provider;
use crate::ai::types::Model;

/// Upstream github-copilot.ts `filterModels` (github-copilot.ts:27-38).
fn copilot_filter_models(models: &[Model], credential: Option<&Credential>) -> Vec<Model> {
    let Some(Credential::OAuth(credential)) = credential else {
        return models.to_vec();
    };
    let Some(serde_json::Value::Array(available_model_ids)) =
        credential.extra.get("availableModelIds")
    else {
        return models.to_vec();
    };
    // `Array.isArray(...) && every(id => typeof id === "string")`.
    if !available_model_ids.iter().all(serde_json::Value::is_string) {
        return models.to_vec();
    }
    let available: HashSet<&str> = available_model_ids
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    models
        .iter()
        .filter(|model| available.contains(model.id.as_str()))
        .cloned()
        .collect()
}

/// The upstream `lazyOAuth` wrapper over the Copilot flow loader.
fn copilot_oauth() -> LazyOAuth {
    lazy_flow("GitHub Copilot", true, None, load_github_copilot_oauth)
}

/// Upstream `githubCopilotProvider` (github-copilot.ts:13-46).
pub fn github_copilot_provider() -> Arc<dyn Provider> {
    let filter: FilterModelsFn = Arc::new(copilot_filter_models);
    create_provider(CreateProviderOptions {
        id: "github-copilot".to_string(),
        name: Some("GitHub Copilot".to_string()),
        base_url: Some("https://api.individual.githubcopilot.com".to_string()),
        headers: None,
        auth: ProviderAuth {
            api_key: Some(crate::ai::auth::helpers::env_api_key_auth(
                "GitHub Copilot token",
                &["COPILOT_GITHUB_TOKEN"],
            )),
            oauth: Some(Arc::new(copilot_oauth())),
        },
        models: embedded_provider_catalog("github-copilot")
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: Some(filter),
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        api: per_api(&[
            ("anthropic-messages", arc(AnthropicMessages)),
            ("openai-completions", arc(OpenAiCompletions)),
            ("openai-responses", arc(OpenAiResponses)),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::OAuthCredential;
    use crate::ai::models::providers::test_support::api_model;

    fn oauth_credential(available_model_ids: serde_json::Value) -> Credential {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("availableModelIds".to_string(), available_model_ids);
        Credential::OAuth(OAuthCredential {
            refresh: "r".to_string(),
            access: "a".to_string(),
            expires: 0,
            extra,
        })
    }

    fn catalog() -> Vec<Model> {
        vec![
            api_model("github-copilot", "openai-responses"),
            api_model("github-copilot", "anthropic-messages"),
        ]
        .into_iter()
        .map(|mut model| {
            model.id = model.api.clone();
            model
        })
        .collect()
    }

    /// The filter narrows to the credential's available ids.
    #[test]
    fn filters_models_to_the_credential_available_ids() {
        let models = catalog();
        let credential = oauth_credential(serde_json::json!(["openai-responses"]));
        let filtered = copilot_filter_models(&models, Some(&credential));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "openai-responses");
    }

    /// Non-OAuth credentials and malformed id lists leave the catalog
    /// untouched (github-copilot.ts:29-36).
    #[test]
    fn non_oauth_or_malformed_credentials_pass_through() {
        let models = catalog();

        let api_key = Credential::ApiKey(crate::ai::auth::types::ApiKeyCredential::default());
        assert_eq!(copilot_filter_models(&models, Some(&api_key)).len(), 2);
        assert_eq!(copilot_filter_models(&models, None).len(), 2);

        // No `availableModelIds` key at all: pass-through.
        let credential = Credential::OAuth(OAuthCredential::default());
        assert_eq!(copilot_filter_models(&models, Some(&credential)).len(), 2);

        // Not an array: pass-through.
        let credential = oauth_credential(serde_json::json!("openai-responses"));
        assert_eq!(copilot_filter_models(&models, Some(&credential)).len(), 2);

        // A non-string entry: pass-through.
        let credential = oauth_credential(serde_json::json!(["openai-responses", 7]));
        assert_eq!(copilot_filter_models(&models, Some(&credential)).len(), 2);

        // A valid empty array filters everything out.
        let credential = oauth_credential(serde_json::json!([]));
        assert!(copilot_filter_models(&models, Some(&credential)).is_empty());
    }

    /// The factory pins: id/name/baseUrl, env token + OAuth, and the
    /// three-API map.
    #[test]
    fn factory_builds_the_copilot_provider() {
        let provider = github_copilot_provider();
        assert_eq!(provider.id(), "github-copilot");
        assert_eq!(provider.name(), "GitHub Copilot");
        assert_eq!(
            provider.base_url(),
            Some("https://api.individual.githubcopilot.com")
        );
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("GitHub Copilot token")
        );
        assert_eq!(
            provider.auth().oauth.as_ref().map(|oauth| oauth.name()),
            Some("GitHub Copilot")
        );
        assert!(!provider.get_models().unwrap().is_empty());
        for model in provider.get_models().unwrap() {
            assert!(
                [
                    "anthropic-messages",
                    "openai-completions",
                    "openai-responses"
                ]
                .contains(&model.api.as_str()),
                "{}",
                model.api
            );
            assert!(provider.api_for(&model).is_some());
        }
    }

    /// The filter is attached to the provider (upstream `filterModels` is a
    /// createProvider option).
    #[test]
    fn provider_applies_the_filter() {
        let provider = github_copilot_provider();
        let models = provider.get_models().unwrap();
        let first = models[0].clone();
        let credential = oauth_credential(serde_json::json!([first.id]));
        let filtered = provider.filter_models(&models, Some(&credential)).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, first.id);
        // A non-OAuth credential passes the full catalog through the filter.
        assert_eq!(
            provider.filter_models(&models, None).unwrap().len(),
            models.len()
        );
    }
}
