//! Port of upstream `coding-agent/src/cli/auth-command.ts` (sha256
//! c313c83c6a89…): `auth` subcommand parsing, validation and credential
//! extraction.
//!
//! Deterministic outputs (parse results, error strings, usage/help text,
//! credential extraction rules) are byte-compared against
//! `tests/fixtures/cli_oracle/oracle.json` in tests.

use crate::ai::auth::types::AuthResult;

use super::args::Args;
use super::APP_NAME;

/// Upstream `AuthCommandKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCommandKind {
    Check,
    ApiKey,
    BearerToken,
}

impl AuthCommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthCommandKind::Check => "check",
            AuthCommandKind::ApiKey => "api_key",
            AuthCommandKind::BearerToken => "bearer_token",
        }
    }
}

/// Upstream `AuthCommand`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    pub kind: AuthCommandKind,
    pub args: Vec<String>,
    pub json: bool,
    pub credentials: bool,
    pub no_refresh: bool,
    pub min_expiry_ms: Option<i64>,
}

/// Upstream `AuthCommandError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommandError(pub String);

impl std::fmt::Display for AuthCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthCommandError {}

pub type AuthCommandResult<T> = Result<T, AuthCommandError>;

fn auth_command_error(message: impl Into<String>) -> AuthCommandError {
    AuthCommandError(message.into())
}

/// Upstream `AUTH_COMMAND_USAGE`.
pub fn get_auth_command_usage(kind: AuthCommandKind) -> String {
    match kind {
        AuthCommandKind::Check => format!(
            "{APP_NAME} auth check --provider <provider> [--json] [--credentials] [--no-refresh]"
        ),
        AuthCommandKind::ApiKey => {
            format!("{APP_NAME} auth print-api-key --provider <provider> [--model <model>]")
        }
        AuthCommandKind::BearerToken => format!(
            "{APP_NAME} auth print-bearer-token --provider <provider> [--model <model>] [--min-expiry <duration>]"
        ),
    }
}

/// Upstream `getAuthCommandName`.
pub fn get_auth_command_name(kind: AuthCommandKind) -> &'static str {
    match kind {
        AuthCommandKind::Check => "auth check",
        AuthCommandKind::ApiKey => "auth print-api-key",
        AuthCommandKind::BearerToken => "auth print-bearer-token",
    }
}

/// Upstream `isAuthCommandHelp`.
pub fn is_auth_command_help(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some("auth")
        && (args.get(1).is_none()
            || args.get(1).map(String::as_str) == Some("help")
            || args.iter().any(|arg| arg == "--help")
            || args.iter().any(|arg| arg == "-h"))
}

/// Upstream `printAuthCommandHelp` (divergence 1: returns the text).
pub fn print_auth_command_help() -> String {
    format!(
        "Usage:
  {APP_NAME} auth print-api-key [--provider <provider>] [--model <model>]
  {APP_NAME} auth print-bearer-token [--provider <provider>] [--model <model>] [--min-expiry <duration>]
  {APP_NAME} auth check [--provider <provider>] [--model <model>] [--json] [--credentials] [--no-refresh]

Auth commands require at least one of --provider or --model. Checks refresh expired OAuth credentials by default; --no-refresh prevents this. --credentials emits the credential, or includes it in JSON output."
    )
}

/// Upstream `/^(\d+)(ms|s|m|h)$/iu`.
fn parse_min_expiry(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    let digit_end = bytes
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(bytes.len());
    if digit_end == 0 || digit_end == bytes.len() {
        return None;
    }
    let (amount, unit) = value.split_at(digit_end);
    let multiplier: i64 = match unit {
        "ms" | "MS" | "mS" | "Ms" => 1,
        "s" | "S" => 1_000,
        "m" | "M" => 60_000,
        "h" | "H" => 3_600_000,
        _ => return None,
    };
    amount.parse::<i64>().ok().map(|amount| amount * multiplier)
}

/// Upstream `parseAuthCommand`.
pub fn parse_auth_command(args: &[String]) -> AuthCommandResult<Option<AuthCommand>> {
    if args.first().map(String::as_str) != Some("auth") {
        return Ok(None);
    }

    let kind = match args.get(1).map(String::as_str) {
        Some("check") => AuthCommandKind::Check,
        Some("print-api-key") => AuthCommandKind::ApiKey,
        Some("print-bearer-token") => AuthCommandKind::BearerToken,
        _ => {
            return Err(auth_command_error(format!(
                "Unknown auth command \"{}\". Use \"{APP_NAME} auth print-api-key\", \"{APP_NAME} auth print-bearer-token\", or \"{APP_NAME} auth check\".",
                args.get(1).map(String::as_str).unwrap_or("")
            )));
        }
    };

    let mut command_args: Vec<String> = Vec::new();
    let mut json = false;
    let mut credentials = false;
    let mut no_refresh = false;
    let mut min_expiry_ms: Option<i64> = None;

    let mut index = 2usize;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--min-expiry" {
            if kind != AuthCommandKind::BearerToken {
                return Err(auth_command_error(
                    "--min-expiry is only supported by print-bearer-token",
                ));
            }
            index += 1;
            let value = args.get(index).map(String::as_str);
            let parsed = value.and_then(parse_min_expiry);
            let Some(parsed) = parsed else {
                return Err(auth_command_error(
                    "--min-expiry must use a duration such as 30m or 1h",
                ));
            };
            min_expiry_ms = Some(parsed);
            index += 1;
            continue;
        }
        if arg == "--json" || arg == "--credentials" || arg == "--no-refresh" {
            if kind != AuthCommandKind::Check {
                return Err(auth_command_error(format!(
                    "{arg} is only supported by auth check"
                )));
            }
            if arg == "--json" {
                json = true;
            } else if arg == "--credentials" {
                credentials = true;
            } else {
                no_refresh = true;
            }
            index += 1;
            continue;
        }
        command_args.push(arg.to_string());
        index += 1;
    }

    Ok(Some(AuthCommand {
        kind,
        args: command_args,
        json,
        credentials,
        no_refresh,
        min_expiry_ms,
    }))
}

/// Upstream `validateAuthCommandArgs`. Returns `(provider, model)`.
pub fn validate_auth_command_args(
    args: &Args,
    kind: AuthCommandKind,
) -> AuthCommandResult<(Option<String>, Option<String>)> {
    let provider = args
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    if !args.unknown_flags.is_empty() {
        let option = args.unknown_flags.first_key().unwrap_or("");
        return Err(auth_command_error(format!(
            "Unknown option --{option} for \"{}\".",
            get_auth_command_name(kind)
        )));
    }
    if args.api_key.is_some() || !args.messages.is_empty() || !args.file_args.is_empty() {
        return Err(auth_command_error(
            "Auth commands only accept --provider and --model",
        ));
    }
    if kind == AuthCommandKind::Check {
        if provider.is_none() && model.is_none() {
            return Err(auth_command_error(
                "Auth checks require --provider <provider> or --model <model>",
            ));
        }
        return Ok((provider, model));
    }
    if provider.is_none() && model.is_none() {
        return Err(auth_command_error(
            "Credential printing requires --provider <provider> or --model <model>",
        ));
    }
    Ok((provider, model))
}

/// Upstream `getAuthCredential`.
pub fn get_auth_credential(auth: Option<&AuthResult>) -> Option<String> {
    let auth = auth?;
    if let Some(api_key) = &auth.auth.api_key {
        return Some(api_key.clone());
    }
    if let Some(headers) = &auth.auth.headers {
        for (name, value) in headers.iter() {
            if name.eq_ignore_ascii_case("authorization") {
                let Some(value) = value else { continue };
                if let Some(token) = bearer_token(value) {
                    return Some(token);
                }
            }
        }
    }
    None
}

/// Upstream `/^Bearer\s+(.+)$/iu` extraction (`\s+` greedy; the capture group
/// starts at the first non-whitespace character).
fn bearer_token(authorization: &str) -> Option<String> {
    const PREFIX: &[u8; 6] = b"bearer";
    let bytes = authorization.as_bytes();
    if bytes.len() < 6
        || !bytes[..6]
            .iter()
            .zip(PREFIX)
            .all(|(actual, expected)| actual.to_ascii_lowercase() == *expected)
    {
        return None;
    }
    let rest = authorization.get(6..)?;
    let trimmed = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
    if trimmed.len() == rest.len() || trimmed.is_empty() {
        // No `\s+` separator (or empty capture group): no match.
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
#[path = "auth_command_tests.rs"]
mod tests;
