//! `main.ts` auth-command dispatch. Only an explicit credential-printing
//! command writes a credential to stdout; diagnostics never include it.
use super::runtime::OperationDeadline;
use crate::{
    ai::auth::{credential_store::CredentialStore, types::AuthType},
    coding_agent::{
        cli::{
            args::parse_args,
            auth_check::{
                check_provider_auth, create_auth_check_model_runtime, get_provider_credential,
                AuthCheckReason, AuthCheckResult, AuthCheckStatus, CheckAuthOptions,
                GetProviderCredentialOptions,
            },
            auth_command::{
                get_auth_command_name, get_auth_command_usage, is_auth_command_help,
                parse_auth_command, print_auth_command_help, validate_auth_command_args,
                AuthCommand, AuthCommandError, AuthCommandKind,
            },
            credential_print::{resolve_credential_for_print, CredentialPrintKind},
        },
        core::{
            auth_storage::{AuthStorage, ReadOnlyAuthStorage},
            model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
            path_join,
        },
    },
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct AuthCommandOutput {
    pub stdout: String,
    pub stderr: Vec<String>,
    pub exit_code: i32,
}
fn error(message: String, exit_code: i32) -> AuthCommandOutput {
    AuthCommandOutput {
        stdout: String::new(),
        stderr: vec![format!("Error: {message}")],
        exit_code,
    }
}

pub async fn run_auth_command(args: &[String], agent_dir: &str) -> Option<AuthCommandOutput> {
    if is_auth_command_help(args) {
        return Some(AuthCommandOutput {
            stdout: format!("{}\n", print_auth_command_help()),
            ..Default::default()
        });
    }
    let command = match parse_auth_command(args) {
        Ok(None) => return None,
        Ok(Some(command)) => command,
        Err(failure) => return Some(error(failure.0, 1)),
    };
    let parsed = parse_args(&command.args);
    if let Some(option) = parsed.unknown_flags.first_key() {
        return Some(AuthCommandOutput {
            stdout: String::new(),
            stderr: vec![
                format!(
                    "Unknown option --{option} for \"{}\".",
                    get_auth_command_name(command.kind)
                ),
                format!(
                    "Use \"pi --help\" or \"{}\".",
                    get_auth_command_usage(command.kind)
                ),
            ],
            exit_code: 1,
        });
    }
    let code = if command.kind == AuthCommandKind::Check {
        2
    } else {
        1
    };
    if !parsed.diagnostics.is_empty() {
        return Some(error(
            parsed
                .diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            code,
        ));
    }
    Some(match execute(command, parsed, agent_dir).await {
        Ok(output) => output,
        Err(failure) => error(failure.0, code),
    })
}
async fn execute(
    command: AuthCommand,
    parsed: crate::coding_agent::cli::args::Args,
    agent_dir: &str,
) -> Result<AuthCommandOutput, AuthCommandError> {
    let auth_path = path_join(agent_dir, "auth.json");
    if command.kind != AuthCommandKind::Check {
        let mut deadline = OperationDeadline::new();
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: Some(auth_path),
            models_path: Some(Some(path_join(agent_dir, "models.json"))),
            allow_model_network: false,
            signal: Some(deadline.token.clone()),
            ..Default::default()
        })
        .await
        .map_err(|_| AuthCommandError("Failed to resolve credential".into()))?;
        let kind = if command.kind == AuthCommandKind::ApiKey {
            CredentialPrintKind::ApiKey
        } else {
            CredentialPrintKind::BearerToken
        };
        let credential = resolve_credential_for_print(
            &parsed,
            &runtime,
            kind,
            command.min_expiry_ms,
            Some(deadline.token.clone()),
        )
        .await?;
        deadline.finish();
        return Ok(AuthCommandOutput {
            stdout: format!("{credential}\n"),
            ..Default::default()
        });
    }
    let (provider, model) = validate_auth_command_args(&parsed, command.kind)?;
    let result: Result<(AuthCheckResult, Option<String>), AuthCommandError> = async {
        let credentials: Arc<dyn CredentialStore> = if command.no_refresh {
            Arc::new(ReadOnlyAuthStorage::new(&auth_path))
        } else {
            Arc::new(AuthStorage::create(&auth_path))
        };
        let runtime = create_auth_check_model_runtime(credentials.clone())
            .await
            .map_err(AuthCommandError)?;
        let mut result = check_provider_auth(
            &parsed,
            &runtime,
            Some(CheckAuthOptions {
                refresh: !command.no_refresh,
            }),
        )
        .await?;
        let mut credential = None;
        if command.credentials && result.status == AuthCheckStatus::Ready {
            credential = get_provider_credential(
                &result.provider,
                &runtime,
                credentials.as_ref(),
                GetProviderCredentialOptions {
                    refresh: !command.no_refresh,
                },
            )
            .await
            .filter(|s| !s.is_empty());
            if credential.is_none() {
                result = AuthCheckResult {
                    status: AuthCheckStatus::NotReady,
                    provider: result.provider,
                    reason: Some(AuthCheckReason::CredentialNotAvailable),
                    auth_type: None,
                };
            }
        }
        Ok((result, credential))
    }
    .await;
    let (result, credential) = result.unwrap_or_else(|_| {
        (
            AuthCheckResult {
                status: AuthCheckStatus::Invalid,
                provider: provider.or(model).unwrap_or_default(),
                reason: Some(AuthCheckReason::InvalidState),
                auth_type: None,
            },
            None,
        )
    });
    Ok(render_check(&result, credential.as_deref(), command.json))
}
fn render_check(
    result: &AuthCheckResult,
    credential: Option<&str>,
    as_json: bool,
) -> AuthCommandOutput {
    let (status, exit_code) = match result.status {
        AuthCheckStatus::Ready => ("ready", 0),
        AuthCheckStatus::NotReady => ("not_ready", 1),
        AuthCheckStatus::Invalid => ("invalid", 2),
    };
    let output = if as_json {
        let mut value = json!({"status":status,"provider":result.provider});
        if let Some(reason) = result.reason {
            value["reason"] = Value::String(
                match reason {
                    AuthCheckReason::ProviderNotFound => "provider_not_found",
                    AuthCheckReason::CredentialsNotConfigured => "credentials_not_configured",
                    AuthCheckReason::CredentialNotAvailable => "credential_not_available",
                    AuthCheckReason::InvalidState => "invalid_state",
                }
                .into(),
            );
        }
        if let Some(kind) = result.auth_type {
            value["authType"] = Value::String(
                match kind {
                    AuthType::ApiKey => "api_key",
                    AuthType::OAuth => "oauth",
                }
                .into(),
            );
        }
        if let Some(credential) = credential {
            value["credentials"] = credential.into();
        }
        value.to_string()
    } else {
        credential.unwrap_or(status).into()
    };
    AuthCommandOutput {
        stdout: format!("{output}\n"),
        stderr: vec![],
        exit_code,
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
