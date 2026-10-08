//! Port of upstream `coding-agent/src/cli/auth-check.ts` (sha256
//! bed735dae4b5…): provider auth readiness checks and credential reads over
//! the ported [`ModelRuntime`].
//!
//! The port's resolver surface is synchronous-prefetch (see
//! `core::model_resolver::PrefetchedRuntime`), so `resolveCliModel` calls are
//! bridged through a prefetch of the runtime's current lists — semantically
//! identical for the check/print flows, which read resolved state.

use std::sync::Arc;

use crate::ai::auth::credential_store::CredentialStore;
use crate::ai::auth::types::AuthType;
use crate::coding_agent::core::model_resolver::{
    resolve_cli_model, PrefetchedRuntime, ResolveCliModelOptions,
};
use crate::coding_agent::core::model_runtime::{
    CreateModelRuntimeOptions, ModelRuntime, ProviderOrModel,
};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;

use super::args::Args;
use super::auth_command::{
    get_auth_credential, validate_auth_command_args, AuthCommandError, AuthCommandKind,
    AuthCommandResult,
};

/// Upstream `AuthCheckStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCheckStatus {
    Ready,
    NotReady,
    Invalid,
}

/// Upstream `AuthCheckReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCheckReason {
    ProviderNotFound,
    CredentialsNotConfigured,
    CredentialNotAvailable,
    InvalidState,
}

/// Upstream `AuthCheckResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCheckResult {
    pub status: AuthCheckStatus,
    pub provider: String,
    pub reason: Option<AuthCheckReason>,
    pub auth_type: Option<AuthType>,
}

/// Bridge from the runtime's current state to the resolver's sync reads.
async fn prefetch(model_runtime: &ModelRuntime) -> PrefetchedRuntime {
    let models = model_runtime.get_models(None).await;
    PrefetchedRuntime {
        available: model_runtime.get_available_snapshot(),
        configured_auth: models
            .iter()
            .filter(|model| model_runtime.has_configured_auth(&model.provider))
            .map(|model| model.provider.clone())
            .collect(),
        model_lookup: models
            .iter()
            .map(|model| ((model.provider.clone(), model.id.clone()), model.clone()))
            .collect(),
        models: models.clone(),
    }
}

/// Upstream `checkProviderAuth`.
pub async fn check_provider_auth(
    args: &Args,
    model_runtime: &ModelRuntime,
    options: Option<CheckAuthOptions>,
) -> AuthCommandResult<AuthCheckResult> {
    let options = options.unwrap_or(CheckAuthOptions { refresh: false });
    let (cli_provider, cli_model) = validate_auth_command_args(args, AuthCommandKind::Check)?;
    let mut provider = cli_provider;
    if let Some(cli_model) = cli_model {
        let reads = prefetch(model_runtime).await;
        let resolved = resolve_cli_model(ResolveCliModelOptions {
            cli_provider: provider.as_deref(),
            cli_model: Some(&cli_model),
            cli_thinking: None,
            model_runtime: &reads,
        });
        if resolved.error.is_some() || resolved.model.is_none() {
            return Err(AuthCommandError(resolved.error.unwrap_or_else(|| {
                format!("Unable to resolve model \"{cli_model}\"")
            })));
        }
        provider = Some(
            resolved
                .model
                .map(|model| model.provider)
                .unwrap_or_default(),
        );
    }
    let Some(provider) = provider else {
        return Err(AuthCommandError(
            "Unable to resolve an auth provider".to_string(),
        ));
    };
    if model_runtime.get_error().is_some() {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::Invalid,
            provider,
            reason: Some(AuthCheckReason::InvalidState),
            auth_type: None,
        });
    }
    if model_runtime.get_provider(&provider).await.is_none() {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider,
            reason: Some(AuthCheckReason::ProviderNotFound),
            auth_type: None,
        });
    }
    let auth = match model_runtime.check_auth(&provider, None).await {
        Ok(auth) => auth,
        Err(_) => {
            return Ok(AuthCheckResult {
                status: AuthCheckStatus::Invalid,
                provider,
                reason: Some(AuthCheckReason::InvalidState),
                auth_type: None,
            });
        }
    };
    let Some(auth) = auth else {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider,
            reason: Some(AuthCheckReason::CredentialsNotConfigured),
            auth_type: None,
        });
    };
    if options.refresh
        && model_runtime
            .get_auth(ProviderOrModel::Provider(&provider), None)
            .await
            .is_err()
    {
        return Ok(AuthCheckResult {
            status: AuthCheckStatus::NotReady,
            provider,
            reason: Some(AuthCheckReason::CredentialsNotConfigured),
            auth_type: None,
        });
    }
    Ok(AuthCheckResult {
        status: AuthCheckStatus::Ready,
        provider,
        reason: None,
        auth_type: Some(auth.r#type),
    })
}

/// Upstream `checkProviderAuth`'s `{ refresh }` option.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckAuthOptions {
    pub refresh: bool,
}

/// Upstream `getProviderCredential`.
pub async fn get_provider_credential(
    provider_id: &str,
    model_runtime: &ModelRuntime,
    credentials: &dyn CredentialStore,
    options: GetProviderCredentialOptions,
) -> Option<String> {
    let credential = credentials
        .read(
            provider_id,
            &crate::ai::auth::types::AuthOperationOptions::NONE,
        )
        .await
        .ok()
        .flatten();
    if !options.refresh {
        if let Some(credential) = &credential {
            if credential.auth_type() == AuthType::OAuth {
                return Some(oauth_access(credential));
            }
        }
    }
    let auth = model_runtime
        .get_auth(ProviderOrModel::Provider(provider_id), None)
        .await
        .ok()
        .flatten();
    get_auth_credential(auth.as_ref())
}

/// Upstream option object `{ refresh: boolean }`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GetProviderCredentialOptions {
    pub refresh: bool,
}

fn oauth_access(credential: &crate::ai::auth::types::Credential) -> String {
    match credential {
        crate::ai::auth::types::Credential::OAuth(oauth) => oauth.access.clone(),
        crate::ai::auth::types::Credential::ApiKey(_) => String::new(),
    }
}

/// Upstream `createAuthCheckModelRuntime`.
pub async fn create_auth_check_model_runtime(
    credentials: Arc<dyn CredentialStore>,
) -> Result<ModelRuntime, String> {
    ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(credentials),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..CreateModelRuntimeOptions::default()
    })
    .await
}

#[cfg(test)]
#[path = "auth_check_tests.rs"]
mod tests;
