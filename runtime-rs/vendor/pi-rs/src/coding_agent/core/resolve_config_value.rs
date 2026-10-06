//! Port of upstream `coding-agent/src/core/resolve-config-value.ts`
//! (vendored into the W3.6 slice: it is the direct dependency of
//! [`super::provider_composer`] and feeds `model-registry.ts`'s
//! `clearApiKeyCache` re-export).
//!
//! Parses configuration values that may be shell commands (`!cmd …`),
//! environment-variable templates (`$VAR`, `${VAR}`, `$$`/`$!` escapes), or
//! literals. The template parser (`parseConfigValueTemplate`), the env-name
//! patterns, resolution precedence (`env overlay → process.env`, JS `||`
//! semantics), and the `resolveConfigValueOrThrow` / `resolveHeadersOrThrow`
//! error texts are byte-pinned against the real upstream module in
//! `tests/fixtures/core_oracle_model/resolve_config_value.oracle.json` (generator
//! `oracle_config_value.mjs`).
//!
//! Disclosures:
//! - Upstream throws `Error` from the `*OrThrow` entry points; the port
//!   carries the same message text through `Result::Err` (the composer's
//!   callers turn it into provider composition errors / `ModelsError`s).
//! - Live shell-command execution is platform/shell dependent and is not
//!   byte-pinned. The vendored [`shell_config`] reproduces upstream
//!   `getShellConfig`'s resolution order (Git Bash known locations →
//!   `bash.exe` on PATH on Windows; `/bin/bash` → `bash` → `sh` on Unix,
//!   legacy-WSL stdin transport included); `getBinDir`-based custom shells
//!   (upstream takes them through `utils/shell.ts`'s optional argument) and
//!   the 10s `spawnSync` timeout join the disclosure notes: the port uses
//!   blocking [`std::process::Command`] with no timeout, like the rest of
//!   the port's sync process surface.
//! - The command-result cache lives for the process lifetime, keyed by the
//!   raw `!…` config, exactly like upstream ([`clear_config_value_cache`]
//!   empties it).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

/// Ordered `Record<string, string>` overlay (upstream passes plain objects;
/// lookups are by name and construction order is caller-owned).
pub type ConfigEnv = BTreeMap<String, String>;

// ---------------------------------------------------------------------------
// Reference parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplatePart {
    Literal(String),
    Env(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigValueReference {
    Command(String),
    Template(Vec<TemplatePart>),
}

/// Upstream `ENV_VAR_NAME_RE` (`/^[A-Za-z_][A-Za-z0-9_]*$/`).
fn is_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Upstream `ENV_VAR_NAME_PREFIX_RE` (`/^[A-Za-z_][A-Za-z0-9_]*/`).
fn env_var_name_prefix(value: &str) -> Option<&str> {
    let mut end = 0;
    for (index, c) in value.char_indices() {
        if index == 0 {
            if !(c.is_ascii_alphabetic() || c == '_') {
                return None;
            }
        } else if !(c.is_ascii_alphanumeric() || c == '_') {
            break;
        }
        end = index + c.len_utf8();
    }
    if end == 0 {
        None
    } else {
        Some(&value[..end])
    }
}

fn append_literal(parts: &mut Vec<TemplatePart>, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(TemplatePart::Literal(previous)) = parts.last_mut() {
        previous.push_str(value);
        return;
    }
    parts.push(TemplatePart::Literal(value.to_string()));
}

/// Upstream `parseConfigValueTemplate`.
fn parse_config_value_template(config: &str) -> Vec<TemplatePart> {
    let mut parts: Vec<TemplatePart> = Vec::new();
    let mut index = 0;

    while index < config.len() {
        let Some(dollar_index) = config[index..].find('$').map(|offset| offset + index) else {
            append_literal(&mut parts, &config[index..]);
            break;
        };

        append_literal(&mut parts, &config[index..dollar_index]);
        let next_char = config.as_bytes().get(dollar_index + 1).copied();

        if next_char == Some(b'$') || next_char == Some(b'!') {
            let literal = next_char.unwrap() as char;
            append_literal(&mut parts, &literal.to_string());
            index = dollar_index + 2;
            continue;
        }

        if next_char == Some(b'{') {
            let Some(end_index) = config[dollar_index + 2..]
                .find('}')
                .map(|offset| offset + dollar_index + 2)
            else {
                append_literal(&mut parts, "$");
                index = dollar_index + 1;
                continue;
            };

            let name = &config[dollar_index + 2..end_index];
            if is_env_var_name(name) {
                parts.push(TemplatePart::Env(name.to_string()));
            } else {
                append_literal(&mut parts, &config[dollar_index..=end_index]);
            }
            index = end_index + 1;
            continue;
        }

        if let Some(name) = env_var_name_prefix(&config[dollar_index + 1..]) {
            parts.push(TemplatePart::Env(name.to_string()));
            index = dollar_index + 1 + name.len();
            continue;
        }

        append_literal(&mut parts, "$");
        index = dollar_index + 1;
    }

    parts
}

/// Upstream `parseConfigValueReference`.
fn parse_config_value_reference(config: &str) -> ConfigValueReference {
    if config.starts_with('!') {
        return ConfigValueReference::Command(config.to_string());
    }

    ConfigValueReference::Template(parse_config_value_template(config))
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Upstream `resolveEnvConfigValue`: the explicit overlay wins, but an empty
/// value falls through to the process environment, and an empty process
/// value is `undefined` (JS `env?.[name] || process.env[name] || undefined`).
fn resolve_env_config_value(name: &str, env: Option<&ConfigEnv>) -> Option<String> {
    match env.and_then(|env| env.get(name)) {
        Some(value) if !value.is_empty() => Some(value.clone()),
        _ => std::env::var(name).ok().filter(|value| !value.is_empty()),
    }
}

/// Upstream `getTemplateEnvVarNames` (first-seen order).
fn get_template_env_var_names(parts: &[TemplatePart]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        let TemplatePart::Env(name) = part else {
            continue;
        };
        if names.contains(name) {
            continue;
        }
        names.push(name.clone());
    }
    names
}

/// Upstream `resolveTemplate`.
fn resolve_template(parts: &[TemplatePart], env: Option<&ConfigEnv>) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(value) => resolved.push_str(value),
            TemplatePart::Env(name) => {
                let env_value = resolve_env_config_value(name, env)?;
                resolved.push_str(&env_value);
            }
        }
    }
    Some(resolved)
}

/// Upstream `getConfigValueEnvVarName`: the single env name of a pure `$VAR`
/// reference, `None` otherwise.
pub fn get_config_value_env_var_name(config: &str) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Template(parts) => match parts.as_slice() {
            [TemplatePart::Env(name)] => Some(name.clone()),
            _ => None,
        },
        ConfigValueReference::Command(_) => None,
    }
}

/// Upstream `getConfigValueEnvVarNames`.
pub fn get_config_value_env_var_names(config: &str) -> Vec<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Template(parts) => get_template_env_var_names(&parts),
        ConfigValueReference::Command(_) => Vec::new(),
    }
}

/// Upstream `getMissingConfigValueEnvVarNames`.
pub fn get_missing_config_value_env_var_names(
    config: &str,
    env: Option<&ConfigEnv>,
) -> Vec<String> {
    get_config_value_env_var_names(config)
        .into_iter()
        .filter(|name| resolve_env_config_value(name, env).is_none())
        .collect()
}

/// Upstream `isCommandConfigValue`.
pub fn is_command_config_value(config: &str) -> bool {
    matches!(
        parse_config_value_reference(config),
        ConfigValueReference::Command(_)
    )
}

/// Upstream `isConfigValueConfigured`.
pub fn is_config_value_configured(config: &str, env: Option<&ConfigEnv>) -> bool {
    get_missing_config_value_env_var_names(config, env).is_empty()
}

// ---------------------------------------------------------------------------
// Shell execution (vendored utils/shell.ts subset)
// ---------------------------------------------------------------------------

/// Upstream `ShellConfig` (utils/shell.ts): the shell plus argument prefix;
/// `stdin` transport pipes the command instead of appending it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    pub shell: String,
    pub args: Vec<String>,
    pub command_transport: Option<Transport>,
}

/// Upstream `commandTransport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Argv,
    Stdin,
}

/// Upstream `isLegacyWslBashPath`: legacy WSL bash runs with `-s` stdin.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let bytes = normalized.as_bytes();
    // `^[a-z]:\windows\(?:system32|sysnative)\bash.exe$`
    let prefix = "\\windows\\";
    let drive_ok = bytes.len() > 2 && bytes[0].is_ascii_lowercase() && bytes[1] == b':';
    if !drive_ok || !normalized[2..].starts_with(prefix) {
        return false;
    }
    let rest = &normalized[2 + prefix.len()..];
    rest == "system32\\bash.exe" || rest == "sysnative\\bash.exe"
}

fn get_bash_shell_config(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-s".to_string()],
            command_transport: Some(Transport::Stdin),
        }
    } else {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-c".to_string()],
            command_transport: None,
        }
    }
}

/// Upstream `getShellConfig()`'s default resolution (no custom shell path —
/// the custom path flows through `utils/shell.ts`'s settings plumbing, not
/// yet ported; disclosed). `None` = upstream's "No bash shell found" throw,
/// mapped to the `executed: false` channel by the caller.
fn get_shell_config() -> Option<ShellConfig> {
    if cfg!(windows) {
        let mut paths: Vec<String> = Vec::new();
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            paths.push(format!("{program_files}\\Git\\bin\\bash.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            paths.push(format!("{program_files_x86}\\Git\\bin\\bash.exe"));
        }
        for path in &paths {
            if std::path::Path::new(path).exists() {
                return Some(get_bash_shell_config(path));
            }
        }
        // Fallback: `where bash.exe` + existence check (upstream
        // findExecutableOnPath).
        if let Ok(output) = std::process::Command::new("where").arg("bash.exe").output() {
            if output.status.success() {
                let first = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .split(['\r', '\n'])
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !first.is_empty() && std::path::Path::new(&first).exists() {
                    return Some(get_bash_shell_config(&first));
                }
            }
        }
        None
    } else {
        if std::path::Path::new("/bin/bash").exists() {
            return Some(get_bash_shell_config("/bin/bash"));
        }
        if let Ok(output) = std::process::Command::new("which").arg("bash").output() {
            if output.status.success() {
                let first = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !first.is_empty() {
                    return Some(get_bash_shell_config(&first));
                }
            }
        }
        Some(ShellConfig {
            shell: "sh".to_string(),
            args: vec!["-c".to_string()],
            command_transport: None,
        })
    }
}

/// Upstream `executeWithConfiguredShell`: `None` outcome = the shell could
/// not run at all (upstream `executed: false`), `Some(None)` = it ran and
/// produced nothing usable, `Some(text)` = trimmed stdout.
fn execute_with_configured_shell(command: &str) -> Option<Option<String>> {
    use std::process::Stdio;
    let config = get_shell_config()?;
    let command_from_stdin = config.command_transport == Some(Transport::Stdin);
    let mut spawn = std::process::Command::new(&config.shell);
    if command_from_stdin {
        spawn.args(&config.args).stdin(Stdio::piped());
    } else {
        spawn.args(&config.args).arg(command).stdin(Stdio::null());
    }
    let output = match spawn.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(None),
    };
    if !output.status.success() {
        return Some(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(if value.is_empty() { None } else { Some(value) })
}

/// Upstream `executeWithDefaultShell` (`execSync`, resolve-config-value.ts
/// 185-196): node runs the command STRING through the platform shell —
/// `/bin/sh -c` on posix, `cmd.exe /d /s /c` on win32 — with a 10s timeout,
/// stdin ignored and stderr ignored; trimmed stdout or `None` on any failure.
fn execute_with_default_shell(command: &str) -> Option<String> {
    #[cfg(unix)]
    let mut spawn = {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    #[cfg(windows)]
    let mut spawn = {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut cmd = std::process::Command::new(comspec);
        cmd.arg("/d").arg("/s").arg("/c").arg(command);
        cmd
    };
    let mut child = spawn
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // read stdout concurrently with the deadline wait so a child producing
    // more than the pipe buffer cannot deadlock the 10s timeout
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut stdout) = stdout {
            use std::io::Read;
            let _ = stdout.read_to_end(&mut buffer);
        }
        buffer
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10_000);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => return None,
        }
    };
    let text = String::from_utf8_lossy(&reader.join().unwrap_or_default())
        .trim()
        .to_string();
    if !status.success() || text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Upstream `executeCommandUncached`: strips the leading `!`; on win32 the
/// configured shell runs first and the direct spawn is only the fallback
/// when no shell could be found at all.
fn execute_command_uncached(command_config: &str) -> Option<String> {
    let command = &command_config[1..];
    if cfg!(windows) {
        match execute_with_configured_shell(command) {
            Some(executed) => executed,
            None => execute_with_default_shell(command),
        }
    } else {
        execute_with_default_shell(command)
    }
}

/// The process-lifetime command-result cache (upstream `commandResultCache`).
fn command_result_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Upstream `executeCommand` (cached).
fn execute_command(command_config: &str) -> Option<String> {
    let mut cache = command_result_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cached) = cache.get(command_config) {
        return cached.clone();
    }
    let result = execute_command_uncached(command_config);
    cache.insert(command_config.to_string(), result.clone());
    result
}

/// Upstream `resolveConfigValue`.
pub fn resolve_config_value(config: &str, env: Option<&ConfigEnv>) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Command(config) => execute_command(&config),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

/// Upstream `resolveConfigValueUncached`.
pub fn resolve_config_value_uncached(config: &str, env: Option<&ConfigEnv>) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Command(config) => execute_command_uncached(&config),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

/// Upstream `resolveConfigValueOrThrow` failure text (byte-pinned).
pub type ConfigValueError = String;

/// Upstream `resolveConfigValueOrThrow`.
pub fn resolve_config_value_or_throw(
    config: &str,
    description: &str,
    env: Option<&ConfigEnv>,
) -> Result<String, ConfigValueError> {
    if let Some(resolved_value) = resolve_config_value_uncached(config, env) {
        return Ok(resolved_value);
    }

    match parse_config_value_reference(config) {
        ConfigValueReference::Command(config) => Err(format!(
            "Failed to resolve {description} from shell command: {}",
            &config[1..]
        )),
        ConfigValueReference::Template(_) => {
            let missing_env_vars = get_missing_config_value_env_var_names(config, env);
            if missing_env_vars.len() == 1 {
                Err(format!(
                    "Failed to resolve {description} from environment variable: {}",
                    missing_env_vars[0]
                ))
            } else if missing_env_vars.len() > 1 {
                Err(format!(
                    "Failed to resolve {description} from environment variables: {}",
                    missing_env_vars.join(", ")
                ))
            } else {
                Err(format!("Failed to resolve {description}"))
            }
        }
    }
}

/// Upstream `resolveHeaders` (unresolved and empty-resolving entries are
/// dropped; all-empty yields `None`). Header records stay in document order
/// (JS object semantics) because the `*OrThrow` error text pins the *first*
/// failing entry in document order.
pub fn resolve_headers(
    headers: Option<&[(String, String)]>,
    env: Option<&ConfigEnv>,
) -> Option<Vec<(String, String)>> {
    let headers = headers?;
    let mut resolved: Vec<(String, String)> = Vec::new();
    for (key, value) in headers {
        // Upstream: `if (resolvedValue) {...}` — empty resolutions drop.
        if let Some(resolved_value) = resolve_config_value(value, env) {
            if resolved_value.is_empty() {
                continue;
            }
            match resolved.iter_mut().find(|(existing, _)| existing == key) {
                Some(entry) => entry.1 = resolved_value,
                None => resolved.push((key.clone(), resolved_value)),
            }
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Upstream `resolveHeadersOrThrow` (error texts byte-pinned).
pub fn resolve_headers_or_throw(
    headers: Option<&[(String, String)]>,
    description: &str,
    env: Option<&ConfigEnv>,
) -> Result<Option<Vec<(String, String)>>, ConfigValueError> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut resolved: Vec<(String, String)> = Vec::new();
    for (key, value) in headers {
        let resolved_value =
            resolve_config_value_or_throw(value, &format!("{description} header \"{key}\""), env)?;
        match resolved.iter_mut().find(|(existing, _)| existing == key) {
            Some(entry) => entry.1 = resolved_value,
            None => resolved.push((key.clone(), resolved_value)),
        }
    }
    Ok(if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    })
}

/// Upstream `clearConfigValueCache` (the coding-agent `clearApiKeyCache`).
pub fn clear_config_value_cache() {
    command_result_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

#[cfg(test)]
#[path = "resolve_config_value_tests.rs"]
mod tests;
