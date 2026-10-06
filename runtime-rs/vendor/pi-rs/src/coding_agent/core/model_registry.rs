//! Port of upstream `coding-agent/src/core/model-registry.ts`: the
//! synchronous compatibility facade exposed to extensions over
//! [`ModelRuntime`](super::model_runtime::ModelRuntime). Coding-agent
//! internals use the runtime directly, exactly like upstream.
//!
//! Disclosure (sync→async seam): upstream's facade methods are synchronous
//! over the JS runtime; the port's runtime reads are `async fn` (see the
//! model-runtime module docs), so the facade methods that hit the collection
//! are async too. The pure re-exports (`ResolvedRequestAuth`, the
//! `clearApiKeyCache` alias) keep their upstream names/shapes.

use crate::ai::auth::types::{AuthError, AuthResult};
use crate::ai::models::{ModelsApiStreamOptions, ModelsSimpleStreamOptions, Provider};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::Model;
use tokio::sync::mpsc;

use crate::ai::types::events::AssistantMessageEvent;

use super::model_runtime::ModelRuntime;
use super::provider_composer::{AuthStatus, ProviderConfigInput};

pub use super::provider_composer::AuthStatusSource;
pub use super::resolve_config_value::clear_config_value_cache as clear_api_key_cache;

/// Upstream `ResolvedRequestAuth`: request auth for one model as the
/// registry's compatibility surface reports it.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedRequestAuth {
    Ok {
        api_key: Option<String>,
        headers: Option<crate::ai::types::options::ProviderHeaders>,
        base_url: Option<String>,
        env: Option<crate::ai::types::options::ProviderEnv>,
    },
    Err {
        error: String,
    },
}

/// Upstream `ModelRegistry`.
#[derive(Clone)]
pub struct ModelRegistry {
    runtime: ModelRuntime,
}

impl ModelRegistry {
    /// Upstream `new ModelRegistry(runtime)`.
    pub fn new(runtime: ModelRuntime) -> Self {
        Self { runtime }
    }

    /// The wrapped runtime (the port's stand-in for the test helper's
    /// `WeakMap` channel).
    pub fn runtime(&self) -> &ModelRuntime {
        &self.runtime
    }

    /// Reload models.json asynchronously. Await before making registry reads.
    pub async fn refresh(
        &self,
        options: Option<crate::ai::models::ModelsRefreshOptions>,
    ) -> Result<crate::ai::models::ModelsRefreshResult, String> {
        self.runtime.refresh(options.unwrap_or_default()).await
    }

    pub fn get_error(&self) -> Option<String> {
        self.runtime.get_error()
    }

    pub async fn get_all(&self) -> Vec<Model> {
        self.runtime.get_models(None).await
    }

    pub fn get_available(&self) -> Vec<Model> {
        self.runtime.get_available_snapshot()
    }

    pub async fn find(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.runtime.get_model(provider, model_id).await
    }

    pub fn has_configured_auth(&self, model: &Model) -> bool {
        self.runtime.has_configured_auth(&model.provider)
    }

    /// Upstream `getApiKeyAndHeaders`: request auth through the runtime with
    /// the compatibility fallbacks (static model headers when provider auth
    /// is unconfigured; `No API key found for "<provider>"` when
    /// `authHeader` is set without a resolved key).
    pub async fn get_api_key_and_headers(&self, model: &Model) -> ResolvedRequestAuth {
        let resolution = match self
            .runtime
            .get_auth(super::model_runtime::ProviderOrModel::Model(model), None)
            .await
        {
            Ok(resolution) => resolution,
            Err(error) => {
                return ResolvedRequestAuth::Err {
                    error: map_auth_error(error, &model.provider),
                }
            }
        };
        let Some(resolution) = resolution else {
            // Compatibility fallback: report static model headers when the
            // provider has no configured auth header handling.
            let compatibility = match self.runtime.get_compatibility_request_config(model) {
                Ok(compatibility) => compatibility,
                Err(error) => return ResolvedRequestAuth::Err { error },
            };
            if compatibility.auth_header {
                return ResolvedRequestAuth::Err {
                    error: format!("No API key found for \"{}\"", model.provider),
                };
            }
            return ResolvedRequestAuth::Ok {
                api_key: None,
                headers: compatibility.headers,
                base_url: None,
                env: None,
            };
        };
        ResolvedRequestAuth::Ok {
            api_key: resolution.auth.api_key,
            headers: resolution.auth.headers,
            base_url: resolution.auth.base_url,
            env: resolution.env,
        }
    }

    pub fn get_provider_auth_status(&self, provider: &str) -> AuthStatus {
        self.runtime.get_provider_auth_status(provider)
    }

    pub async fn get_provider(&self, provider: &str) -> Option<ArcProvider> {
        self.runtime.get_provider(provider).await
    }

    /// Stream through the configured provider with request-time
    /// authentication.
    pub fn stream(
        &self,
        model: &Model,
        context: &crate::ai::Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        self.runtime.stream(model, context, options)
    }

    /// Stream with provider-neutral options and request-time authentication.
    pub fn stream_simple(
        &self,
        model: &Model,
        context: &crate::ai::Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        self.runtime.stream_simple(model, context, options)
    }

    pub async fn complete(
        &self,
        model: &Model,
        context: &crate::ai::Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessage {
        self.runtime.complete(model, context, options).await
    }

    pub async fn get_provider_display_name(&self, provider: &str) -> String {
        match self.runtime.get_provider(provider).await {
            Some(provider) => provider.name().to_string(),
            None => provider.to_string(),
        }
    }

    pub async fn get_provider_auth(&self, provider: &str) -> Option<AuthResult> {
        self.runtime
            .get_auth(
                super::model_runtime::ProviderOrModel::Provider(provider),
                None,
            )
            .await
            .ok()
            .flatten()
    }

    pub async fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        match self
            .runtime
            .get_auth(
                super::model_runtime::ProviderOrModel::Provider(provider),
                None,
            )
            .await
        {
            Ok(resolution) => resolution.and_then(|resolution| resolution.auth.api_key),
            Err(_) => None,
        }
    }

    pub fn is_using_oauth(&self, model: &Model) -> bool {
        self.runtime.is_using_oauth(&model.provider)
    }

    /// Upstream `registerProvider(provider)` (the native form).
    pub async fn register_native_provider(
        &self,
        provider: std::sync::Arc<dyn Provider>,
    ) -> Result<(), super::model_runtime::ProviderRegistrationError> {
        self.runtime.register_native_provider(provider).await
    }

    /// Upstream `registerProvider(providerName, config)` (the by-name form).
    pub async fn register_provider(
        &self,
        provider_name: &str,
        config: ProviderConfigInput,
    ) -> Result<(), super::model_runtime::ProviderRegistrationError> {
        self.register_provider_sync(provider_name, config)
    }

    pub fn register_provider_sync(
        &self,
        provider_name: &str,
        config: ProviderConfigInput,
    ) -> Result<(), super::model_runtime::ProviderRegistrationError> {
        if config.models.is_none()
            && config.stream_simple.is_none()
            && config.name.is_none()
            && config.base_url.is_none()
            && config.api_key.is_none()
            && config.api.is_none()
            && config.headers.is_none()
            && config.auth_header.is_none()
            && config.oauth.is_none()
            && config.refresh_models.is_none()
        {
            // Upstream throws when the second argument is missing entirely;
            // the port's Option-less signature approximates the guard with
            // the "everything undefined" shape.
            return Err(super::model_runtime::ProviderRegistrationError(
                "Provider config is required when registering by name".to_string(),
            ));
        }
        self.runtime.register_provider_sync(provider_name, config)
    }

    pub async fn unregister_provider(&self, provider_name: &str) {
        self.runtime.unregister_provider(provider_name).await
    }

    pub fn get_registered_provider_config(
        &self,
        provider_name: &str,
    ) -> Option<ProviderConfigInput> {
        self.runtime.get_registered_provider_config(provider_name)
    }

    pub fn get_registered_native_provider(&self, provider_name: &str) -> Option<ArcProvider> {
        self.runtime.get_registered_native_provider(provider_name)
    }

    pub fn get_registered_provider_ids(&self) -> Vec<String> {
        self.runtime.get_registered_provider_ids()
    }
}

/// `Arc<dyn Provider>` alias for the registry surface.
pub type ArcProvider = std::sync::Arc<dyn Provider>;

/// The registry's error-message mapping: upstream maps the
/// `authHeader requires a resolved API key` cause to the provider-scoped
/// `No API key found for …` text; everything else passes through.
fn map_auth_error(error: AuthError, provider: &str) -> String {
    let message = match &error {
        AuthError::Models(models_error) => models_error.message.clone(),
        other => other.to_string(),
    };
    if message == "authHeader requires a resolved API key" {
        format!("No API key found for \"{provider}\"")
    } else {
        message
    }
}

#[cfg(test)]
#[path = "model_registry_tests.rs"]
mod tests;
