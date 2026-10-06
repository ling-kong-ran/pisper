//! Port of upstream `coding-agent/src/cli/credential-print.ts` (sha256
//! 67400cdf9459…): resolve one configured provider credential for the
//! `auth print-api-key` / `auth print-bearer-token` commands.
//!
//! The port calls `ModelRuntime.get_auth`, which refreshes and persists OAuth
//! credentials with less than five minutes remaining through the normal
//! request-auth path (same as upstream).

use std::collections::HashMap;

use crate::ai::auth::resolve::AuthResolutionOverrides;
use crate::ai::auth::types::AuthType;
use crate::coding_agent::core::model_resolver::{
    resolve_cli_model, PrefetchedRuntime, ResolveCliModelOptions,
};
use crate::coding_agent::core::model_runtime::{ModelRuntime, ProviderOrModel};

use super::args::Args;
use super::auth_command::{
    get_auth_credential, validate_auth_command_args, AuthCommandError, AuthCommandKind,
    AuthCommandResult,
};

const DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS: i64 = 30 * 60_000;

/// Upstream `CredentialPrintKind` = `Exclude<AuthCommandKind, "check">`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialPrintKind {
    ApiKey,
    BearerToken,
}

impl From<CredentialPrintKind> for AuthCommandKind {
    fn from(kind: CredentialPrintKind) -> AuthCommandKind {
        match kind {
            CredentialPrintKind::ApiKey => AuthCommandKind::ApiKey,
            CredentialPrintKind::BearerToken => AuthCommandKind::BearerToken,
        }
    }
}

/// Upstream `resolveCredentialForPrint` options (`minExpiryMs`, `signal`).
#[derive(Debug, Clone, Default)]
pub struct ResolveCredentialOptions {
    pub min_expiry_ms: Option<i64>,
    pub signal: Option<tokio_util::sync::CancellationToken>,
}

/// Upstream `resolveCredentialForPrint`.
pub async fn resolve_credential_for_print(
    args: &Args,
    model_runtime: &ModelRuntime,
    kind: CredentialPrintKind,
    min_expiry_ms: Option<i64>,
    signal: Option<tokio_util::sync::CancellationToken>,
) -> AuthCommandResult<String> {
    let _ = signal; // cancellation rides the auth overrides' signal below
    let (cli_provider, cli_model) = validate_auth_command_args(args, kind.into())?;
    let credentials_list = model_runtime
        .list_credentials(None)
        .await
        .map_err(|error| AuthCommandError(error.to_string()))?;
    let credential_types: HashMap<String, AuthType> = credentials_list
        .into_iter()
        .map(|credential| (credential.provider_id, credential.r#type))
        .collect();

    // (provider_id, optional resolved model)
    let mut providers: Vec<(String, Option<crate::ai::types::Model>)> = Vec::new();
    if let Some(cli_provider) = &cli_provider {
        let provider = model_runtime.get_provider(cli_provider).await;
        let Some(provider) = provider else {
            return Err(AuthCommandError(format!(
                "Unknown provider \"{cli_provider}\". Use --list-models to see available providers."
            )));
        };
        let provider_id = provider.id().to_string();
        if let Some(cli_model) = &cli_model {
            let resolved =
                resolve_cli_model_for(model_runtime, Some(&provider_id), cli_model).await;
            if resolved.error.is_some() || resolved.model.is_none() {
                return Err(AuthCommandError(resolved.error.unwrap_or_else(|| {
                    "Unable to resolve the requested provider/model".to_string()
                })));
            }
            providers.push((provider_id, resolved.model));
        } else {
            providers.push((provider_id, None));
        }
    } else {
        let Some(cli_model) = &cli_model else {
            // Upstream `cliModel!` would throw; validate_auth_command_args
            // already rejected the missing-provider-and-model case.
            return Err(AuthCommandError(
                "Credential printing requires --provider <provider> or --model <model>".to_string(),
            ));
        };
        let providers_list = model_runtime.get_providers().await;
        let provider_ids: Vec<String> = providers_list
            .iter()
            .map(|provider| provider.id().to_string())
            .collect();
        for provider_id in provider_ids {
            if !credential_types.contains_key(&provider_id) {
                continue;
            }
            let resolved =
                resolve_cli_model_for(model_runtime, Some(&provider_id), cli_model).await;
            if resolved.model.is_some()
                && resolved.error.is_none()
                && !resolved
                    .warning
                    .as_deref()
                    .is_some_and(|warning| warning.contains("Using custom model id"))
            {
                providers.push((provider_id.clone(), resolved.model));
            }
        }
        if providers.is_empty() {
            return Err(AuthCommandError(format!(
                "Model \"{cli_model}\" not found. Use --list-models to see available models."
            )));
        }
    }

    let mut credentials: Vec<(String, String)> = Vec::new();
    for (provider_id, model) in &providers {
        let credential_type = credential_types.get(provider_id).copied();
        if kind == CredentialPrintKind::ApiKey && credential_type == Some(AuthType::OAuth) {
            continue;
        }
        if kind == CredentialPrintKind::BearerToken && credential_type != Some(AuthType::OAuth) {
            continue;
        }
        let overrides = if kind == CredentialPrintKind::BearerToken {
            AuthResolutionOverrides {
                api_key: None,
                env: None,
                min_oauth_validity_ms: Some(
                    min_expiry_ms.unwrap_or(DEFAULT_BEARER_TOKEN_MIN_EXPIRY_MS),
                ),
                signal: signal.clone(),
            }
        } else {
            AuthResolutionOverrides {
                api_key: None,
                env: None,
                min_oauth_validity_ms: None,
                signal: signal.clone(),
            }
        };
        let auth = match model {
            Some(model) => {
                model_runtime
                    .get_auth(ProviderOrModel::Model(model), Some(&overrides))
                    .await
            }
            None => {
                model_runtime
                    .get_auth(ProviderOrModel::Provider(provider_id), Some(&overrides))
                    .await
            }
        };
        let auth = auth.map_err(|error| AuthCommandError(error.to_string()))?;
        if let Some(auth) = auth {
            if let Some(value) = get_auth_credential(Some(&auth)) {
                credentials.push((provider_id.clone(), value));
            }
        }
    }

    if credentials.len() == 1 {
        return Ok(credentials.remove(0).1);
    }
    if credentials.is_empty() {
        let provider_id = providers.first().map(|(id, _)| id.clone());
        let credential_type = provider_id
            .as_ref()
            .and_then(|id| credential_types.get(id).copied());
        if cli_provider.is_some()
            && kind == CredentialPrintKind::ApiKey
            && credential_type == Some(AuthType::OAuth)
        {
            return Err(AuthCommandError(format!(
                "Provider \"{}\" is configured with OAuth, not an API key",
                provider_id.unwrap_or_default()
            )));
        }
        if cli_provider.is_some()
            && kind == CredentialPrintKind::BearerToken
            && credential_type != Some(AuthType::OAuth)
        {
            return Err(AuthCommandError(format!(
                "Provider \"{}\" is not configured with an OAuth bearer token",
                provider_id.unwrap_or_default()
            )));
        }
        return Err(AuthCommandError(format!(
            "No usable {} is configured",
            if kind == CredentialPrintKind::ApiKey {
                "API key"
            } else {
                "OAuth bearer token"
            }
        )));
    }
    let ids: Vec<String> = credentials.into_iter().map(|(id, _)| id).collect();
    Err(AuthCommandError(format!(
        "Multiple configured providers matched ({}). Specify --provider.",
        ids.join(", ")
    )))
}

async fn resolve_cli_model_for(
    model_runtime: &ModelRuntime,
    provider_id: Option<&str>,
    cli_model: &str,
) -> crate::coding_agent::core::model_resolver::ResolveCliModelResult {
    let models = model_runtime.get_models(None).await;
    let reads = PrefetchedRuntime {
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
        models,
    };
    resolve_cli_model(ResolveCliModelOptions {
        cli_provider: provider_id,
        cli_model: Some(cli_model),
        cli_thinking: None,
        model_runtime: &reads,
    })
}

#[cfg(test)]
#[path = "credential_print_tests.rs"]
mod tests;
