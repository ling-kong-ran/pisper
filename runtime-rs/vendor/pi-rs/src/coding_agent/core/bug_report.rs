//! Port of upstream `coding-agent/src/core/bug-report.ts` (HEAD 2bbfcca43,
//! v0.99.1): the diagnostics bundle builder behind `/bug-report`.
//!
//! The pure surface — URL/JSON redaction, metadata + diagnostics collection
//! (byte-pinned JSON, upstream key order), the archive file assembly, and
//! the summary prompt construction — is pinned against the verbatim upstream
//! sources under `tests/fixtures/core_delta_oracle/bug-report/`.
//!
//! # Seams (disclosed)
//!
//! - **VERSION**: upstream reads `pkg.version` (`"0.99.1"` at HEAD); the port
//!   embeds [`env!("CARGO_PKG_VERSION")`] like the rest of the port (see
//!   `crash_log`). The oracle captures carry an `ORACLE-VERSION` placeholder
//!   substituted with the crate version before comparison.
//! - **Host facts**: upstream `collectEnvironment` reads `process.platform`,
//!   `process.arch`, `os.release()`, `os.version()`, `process.env`, and the
//!   runtime banner (`node/vX` or `bun/X`). The port takes those through
//!   [`HostEnvironment`] ([`detect_host_environment`] best-effort live
//!   values: platform/arch from the build target, Windows release/version
//!   from the registry, matching node's `os` values; the runtime banner is
//!   `rust/<crate version>`). Oracle scenarios inject fixed values.
//! - **Zip**: upstream `utils/zip.ts` is vendored as
//!   [`create_zip_archive`]/[`write_zip_archive`] (the utils slice owns the
//!   shared module). `deflateRawSync` becomes `flate2`'s raw deflate at the
//!   zlib default level 6 and `crc32` the IEEE CRC-32. Upstream stamps
//!   entries with the **local** time (`Date.getHours()`); the port has no
//!   tz database, so the DOS timestamp is rendered in UTC — cosmetic for
//!   archive mtimes, disclosed.
//! - **Dependencies already ported**: crash records come from
//!   [`crate::coding_agent::core::crash_log::CrashRecord`], extension
//!   metadata from [`crate::coding_agent::extensions::types`] (read-only),
//!   and the summary pipeline reuses
//!   [`crate::coding_agent::core::compaction`] (`complete_summarization`,
//!   `estimate_tokens`, `get_summarization_failure`, `serialize_conversation`)
//!   plus [`crate::coding_agent::core::messages::convert_to_llm`].

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::agent_core::harness::compaction::utils::assistant_blocks_text;
use crate::agent_core::types::AgentMessage;
use crate::ai::models::Provider;
use crate::ai::retry::{RetryCallbacks, RetryPolicy};
use crate::ai::transcript::normalize_context;
use crate::ai::types::message::UserMessage;
use crate::ai::types::options::{ProviderEnv, ProviderHeaders};
use crate::ai::types::primitives::{ModelThinkingLevel, StopReason};
use crate::ai::types::{Model, StringOrBlocks, TextContent, TextOrImageBlock};
use crate::ai::uuid::uuid_v7;
use crate::coding_agent::core::crash_log::CrashRecord;
use crate::coding_agent::extensions::types::Extension;
use crate::coding_agent::session_manager::SessionManager;

pub use crate::coding_agent::core::compaction::StreamFn;

/// Upstream `VERSION` (`config.ts`): the port pins the crate version — the
/// release pipeline keeps them in lockstep, unlike the upstream npm version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Custom entry type the session records for a filed bug report.
pub const BUG_REPORT_CUSTOM_ENTRY_TYPE: &str = "pi.bug-report";

const BUG_REPORT_SCHEMA_VERSION: i64 = 1;
const REDACTED: &str = "<redacted>";

fn sensitive_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        // Upstream literal with the `/i` flag.
        Regex::new(
            r"(?i)(?:^|[-_])(api[-_]?key|secret|token|password|passwd|credential|authorization|cookie)(?:$|[-_])",
        )
        .expect("static regex")
    })
}

fn camel_boundary_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"([a-z0-9])([A-Z])").expect("static regex"))
}

fn nested_url_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)^([a-z][a-z0-9+.-]*:)([a-z][a-z0-9+.-]*://.*)$").expect("static regex")
    })
}

fn is_sensitive_key(key: &str) -> bool {
    let snake = camel_boundary_regex().replace_all(key, "${1}_${2}");
    sensitive_key_regex().is_match(&snake)
}

/// Strip credentials and secret-looking query parameters from a URL.
pub fn redact_url(value: &str) -> String {
    // Nested scheme wrapper, e.g. `proxy-https://https://api.example.com`:
    // redact the inner URL and keep the prefix verbatim.
    if let Some(captures) = nested_url_regex().captures(value) {
        let prefix = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let nested = captures.get(2).map(|m| m.as_str()).unwrap_or_default();
        return format!("{prefix}{}", redact_url(nested));
    }
    let Ok(mut url) = url::Url::parse(value) else {
        return value.to_string();
    };
    let mut changed = false;
    if !url.username().is_empty() || url.password().is_some() {
        // WHATWG setters silently ignore credentials on schemes that cannot
        // have them (node's `url.username = ""` never throws either).
        let _ = url.set_username("");
        let _ = url.set_password(None);
        changed = true;
    }
    if url.query_pairs().any(|(key, _)| is_sensitive_key(&key)) {
        // `searchParams.set(name, REDACTED)`: the first occurrence keeps its
        // position, later same-name pairs are removed, and the rewritten
        // query serializes urlencoded (node and the `url` crate both follow
        // the WHATWG serializer).
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let mut replaced_first = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for (key, value) in &pairs {
            if is_sensitive_key(key) {
                if seen.contains(&key.as_str()) {
                    continue;
                }
                seen.push(key);
                replaced_first.push((key.clone(), REDACTED.to_string()));
            } else {
                replaced_first.push((key.clone(), value.clone()));
            }
        }
        {
            let mut query = url.query_pairs_mut();
            query.clear();
            query.extend_pairs(
                replaced_first
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            );
        }
        changed = true;
    }
    if changed {
        url.to_string()
    } else {
        value.to_string()
    }
}

/// Copy a JSON value while removing values that may contain credentials.
/// A sensitive key replaces its entire subtree (upstream's replacer runs
/// before descending); strings get [`redact_url`].
pub fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    fn walk(key: &str, child: &serde_json::Value) -> serde_json::Value {
        if !child.is_null() && is_sensitive_key(key) {
            return serde_json::Value::String(REDACTED.to_string());
        }
        match child {
            serde_json::Value::String(text) => serde_json::Value::String(redact_url(text)),
            serde_json::Value::Array(items) => serde_json::Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| walk(&index.to_string(), item))
                    .collect(),
            ),
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(name, value)| (name.clone(), walk(name, value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk("", value)
}

/// Upstream `redactSettings`: drop `trackingId`/`deviceId` from the top
/// level, then redact the rest.
pub fn redact_settings(
    settings: &crate::coding_agent::core::settings_manager::SettingsValue,
) -> serde_json::Value {
    let mut plain = settings_value_to_json(settings);
    if let serde_json::Value::Object(map) = &mut plain {
        map.shift_remove("trackingId");
        map.shift_remove("deviceId");
    }
    redact_json_value(&plain)
}

/// `SettingsValue` → JSON (ordered objects; integral numbers stay integral
/// like `JSON.stringify`).
pub fn settings_value_to_json(
    value: &crate::coding_agent::core::settings_manager::SettingsValue,
) -> serde_json::Value {
    use crate::coding_agent::core::settings_manager::SettingsValue;
    match value {
        SettingsValue::Null => serde_json::Value::Null,
        SettingsValue::Bool(flag) => serde_json::Value::Bool(*flag),
        SettingsValue::Num(number) => number_to_json(*number),
        SettingsValue::Str(text) => serde_json::Value::String(text.clone()),
        SettingsValue::Arr(items) => {
            serde_json::Value::Array(items.iter().map(settings_value_to_json).collect())
        }
        SettingsValue::Obj(pairs) => serde_json::Value::Object(
            pairs
                .iter()
                .map(|(name, value)| (name.clone(), settings_value_to_json(value)))
                .collect(),
        ),
    }
}

fn number_to_json(number: f64) -> serde_json::Value {
    // `JSON.stringify` emits integral numbers without a decimal point.
    if number.is_finite() && number.fract() == 0.0 && number.abs() <= i64::MAX as f64 {
        return serde_json::Value::Number(serde_json::Number::from(number as i64));
    }
    serde_json::Number::from_f64(number)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

/// JSON → `SettingsValue` (wiring/test helper mirroring the storage parse).
#[cfg(test)]
pub(crate) fn json_to_settings_value(
    value: serde_json::Value,
) -> crate::coding_agent::core::settings_manager::SettingsValue {
    use crate::coding_agent::core::settings_manager::SettingsValue;
    match value {
        serde_json::Value::Null => SettingsValue::Null,
        serde_json::Value::Bool(flag) => SettingsValue::Bool(flag),
        serde_json::Value::Number(number) => {
            SettingsValue::Num(number.as_f64().unwrap_or_default())
        }
        serde_json::Value::String(text) => SettingsValue::Str(text),
        serde_json::Value::Array(items) => {
            SettingsValue::Arr(items.into_iter().map(json_to_settings_value).collect())
        }
        serde_json::Value::Object(map) => SettingsValue::Obj(
            map.into_iter()
                .map(|(name, value)| (name, json_to_settings_value(value)))
                .collect(),
        ),
    }
}

/// Host facts upstream reads from `process`/`node:os` inside
/// `collectEnvironment`. Oracle scenarios (and tests) inject fixed values.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostEnvironment {
    /// Upstream `process.versions.bun ? `bun/${v}` : `node/${process.version}`.
    pub runtime: String,
    /// Upstream `process.platform` (`win32`/`darwin`/`linux`/...).
    pub platform: String,
    /// Upstream `process.arch` (`x64`/`arm64`/...).
    pub arch: String,
    /// Upstream `os.release()`.
    pub os_release: String,
    /// Upstream `os.version()`.
    pub os_version: String,
    /// A `process.env` snapshot.
    pub env: std::collections::BTreeMap<String, String>,
}

impl HostEnvironment {
    fn env_value(&self, name: &str) -> Option<&str> {
        self.env
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

/// Best-effort live [`HostEnvironment`]: node-compatible platform/arch names
/// from the build target, node-compatible `os.release()`/`os.version()` on
/// Windows via the registry (`10.0.<build>` / product name), the kernel
/// release on Linux, and a `rust/<crate version>` runtime banner (upstream
/// reports `node/vX` or `bun/X` — disclosed).
pub fn detect_host_environment() -> HostEnvironment {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
    .to_string();
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
    .to_string();
    #[cfg(windows)]
    let (os_release, os_version) = {
        let key = windows_registry::LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .ok();
        let release = key.as_ref().and_then(|key| {
            let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
            let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
            let build = key.get_u32("CurrentBuildNumber").ok()?;
            Some(format!("{major}.{minor}.{build}"))
        });
        let version = key
            .as_ref()
            .and_then(|key| key.get_string("ProductName").ok())
            .unwrap_or_default();
        (release.unwrap_or_default(), version)
    };
    #[cfg(target_os = "linux")]
    let (os_release, os_version) = (
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.trim().to_string())
            .unwrap_or_default(),
        String::new(),
    );
    #[cfg(not(any(windows, target_os = "linux")))]
    let (os_release, os_version) = (String::new(), String::new());
    let env = std::env::vars().collect();
    HostEnvironment {
        runtime: format!("rust/{VERSION}"),
        platform,
        arch,
        os_release,
        os_version,
        env,
    }
}

/// Upstream `getPiUserAgent` (coding-agent `utils/pi-user-agent.ts`):
/// `pi/{version} ({platform}; {runtime}; {arch})`. Distinct from the pi-ai
/// package's own `pi_user_agent` (`pi ({platform} {release}; {arch})`).
pub fn get_pi_user_agent_from(version: &str, platform: &str, runtime: &str, arch: &str) -> String {
    format!("pi/{version} ({platform}; {runtime}; {arch})")
}

/// [`get_pi_user_agent_from`] over [`detect_host_environment`].
pub fn get_pi_user_agent(version: &str) -> String {
    let host = detect_host_environment();
    get_pi_user_agent_from(version, &host.platform, &host.runtime, &host.arch)
}

/// Upstream `collectEnvironment` (schema key order preserved).
pub fn collect_environment(host: &HostEnvironment) -> serde_json::Value {
    let env = |name: &str| -> serde_json::Value {
        match host.env_value(name) {
            Some(value) => serde_json::Value::String(value.to_string()),
            None => serde_json::Value::Null,
        }
    };
    let boolean = |name: &str| -> bool { host.env_value(name).is_some() };
    let shell = host.env_value("SHELL").and_then(|shell| {
        shell
            .split(['/', '\\'])
            .next_back()
            .filter(|part| !part.is_empty())
            .map(str::to_string)
    });
    // Names help diagnose configuration; values never leave the machine.
    let pi_environment_variables: Vec<serde_json::Value> = host
        .env
        .keys()
        .filter(|name| name.starts_with("PI_"))
        .map(|name| serde_json::Value::String(name.clone()))
        .collect();
    serde_json::json!({
        "version": VERSION,
        "userAgent": get_pi_user_agent_from(VERSION, &host.platform, &host.runtime, &host.arch),
        "runtime": host.runtime,
        "platform": host.platform,
        "arch": host.arch,
        "osRelease": host.os_release,
        "osVersion": host.os_version,
        "shell": shell
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        "terminal": {
            "term": env("TERM"),
            "program": env("TERM_PROGRAM"),
            "programVersion": env("TERM_PROGRAM_VERSION"),
            "colorterm": env("COLORTERM"),
            "tmux": boolean("TMUX"),
            "ssh": boolean("SSH_CONNECTION") || boolean("SSH_CLIENT") || boolean("SSH_TTY"),
            "ci": boolean("CI"),
        },
        "piEnvironmentVariables": pi_environment_variables,
    })
}

/// Upstream `describeModel` (schema key order preserved).
pub fn describe_model(model: &Model) -> serde_json::Value {
    let mut value = serde_json::Map::new();
    value.insert(
        "provider".into(),
        serde_json::Value::String(model.provider.clone()),
    );
    value.insert("id".into(), serde_json::Value::String(model.id.clone()));
    value.insert("name".into(), serde_json::Value::String(model.name.clone()));
    value.insert("api".into(), serde_json::Value::String(model.api.clone()));
    value.insert(
        "baseUrl".into(),
        serde_json::Value::String(redact_url(&model.base_url)),
    );
    value.insert("reasoning".into(), serde_json::Value::Bool(model.reasoning));
    value.insert(
        "input".into(),
        serde_json::to_value(&model.input).unwrap_or(serde_json::Value::Null),
    );
    value.insert(
        "contextWindow".into(),
        serde_json::json!(model.context_window),
    );
    value.insert("maxTokens".into(), serde_json::json!(model.max_tokens));
    value.insert(
        "samplingParams".into(),
        match &model.sampling_params {
            Some(params) => {
                redact_json_value(&serde_json::to_value(params).unwrap_or(serde_json::Value::Null))
            }
            None => serde_json::Value::Null,
        },
    );
    value.insert(
        "compat".into(),
        match &model.compat {
            Some(compat) => redact_json_value(compat),
            None => serde_json::Value::Null,
        },
    );
    value.insert(
        "thinkingLevelMap".into(),
        match &model.thinking_level_map {
            Some(map) => serde_json::to_value(map).unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        },
    );
    let mut header_names: Vec<String> = model
        .headers
        .as_ref()
        .map(|headers| headers.keys().cloned().collect())
        .unwrap_or_default();
    header_names.sort();
    value.insert(
        "headerNames".into(),
        serde_json::Value::Array(
            header_names
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    serde_json::Value::Object(value)
}

/// The `ModelRuntime` reads `describeProvider`/metadata collection make
/// (upstream takes the `ModelRuntime` object; the port narrows it to the
/// reads so tests can stub the runtime like the oracle does). The async
/// provider lookup stays with the caller: pass
/// `model_runtime.get_provider(&model.provider).await` as the options'
/// `provider`.
pub trait BugReportRuntimeView: Sync {
    /// Upstream `getProviderAuthStatus(providerId)`.
    fn auth_status(&self, provider_id: &str) -> super::provider_composer::AuthStatus;
    /// Upstream `isUsingOAuth(providerId)`.
    fn using_oauth(&self, provider_id: &str) -> bool;
    /// Upstream `getRegisteredProviderIds()`.
    fn registered_provider_ids(&self) -> Vec<String>;
}

impl BugReportRuntimeView for super::model_runtime::ModelRuntime {
    fn auth_status(&self, provider_id: &str) -> super::provider_composer::AuthStatus {
        self.get_provider_auth_status(provider_id)
    }

    fn using_oauth(&self, provider_id: &str) -> bool {
        self.is_using_oauth(provider_id)
    }

    fn registered_provider_ids(&self) -> Vec<String> {
        self.get_registered_provider_ids()
    }
}

/// Upstream `describeProvider` (schema key order preserved). The runtime
/// reads ride on [`BugReportRuntimeView`] so the pure composition is
/// testable against the oracle's stub runtime.
pub fn describe_provider(
    runtime_view: &dyn BugReportRuntimeView,
    provider: &dyn Provider,
) -> serde_json::Value {
    let mut auth_types: Vec<serde_json::Value> = Vec::new();
    if provider.auth().api_key.is_some() {
        auth_types.push(serde_json::Value::String("api_key".to_string()));
    }
    if provider.auth().oauth.is_some() {
        auth_types.push(serde_json::Value::String("oauth".to_string()));
    }
    let auth_status = runtime_view.auth_status(provider.id());
    let mut status = serde_json::Map::new();
    status.insert(
        "configured".into(),
        serde_json::Value::Bool(auth_status.configured),
    );
    if let Some(source) = &auth_status.source {
        status.insert(
            "source".into(),
            serde_json::Value::String(source.as_str().to_string()),
        );
    }
    if let Some(label) = &auth_status.label {
        status.insert("label".into(), serde_json::Value::String(label.clone()));
    }
    let mut header_names: Vec<String> = provider
        .headers()
        .map(|headers| headers.keys().cloned().collect())
        .unwrap_or_default();
    header_names.sort();
    let registered = runtime_view.registered_provider_ids();
    serde_json::json!({
        "id": provider.id(),
        "name": provider.name(),
        "baseUrl": match provider.base_url() {
            Some(base_url) => serde_json::Value::String(redact_url(base_url)),
            None => serde_json::Value::Null,
        },
        "headerNames": header_names,
        "authTypes": auth_types,
        "authStatus": serde_json::Value::Object(status),
        "usingOAuth": runtime_view.using_oauth(provider.id()),
        "registeredByExtension": registered.iter().any(|id| id == provider.id()),
    })
}

/// Upstream `describeExtension` (schema key order preserved).
pub fn describe_extension(extension: &Extension) -> serde_json::Value {
    serde_json::json!({
        "path": extension.path,
        "source": redact_url(&extension.source_info.source),
        "scope": extension.source_info.scope,
        "origin": extension.source_info.origin,
        "hidden": extension.hidden,
    })
}

/// Upstream `CollectBugReportMetadataOptions`. Host facts ride in explicitly
/// (upstream reads `process`/`node:os` directly) and the provider is
/// resolved by the caller (the port's `ModelRuntime::get_provider` is
/// async), keeping collection synchronous like upstream.
pub struct CollectBugReportMetadataOptions<'a> {
    pub id: Option<&'a str>,
    pub hint: Option<&'a str>,
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub include_session: bool,
    pub include_summary: bool,
    pub message_count: u64,
    pub model: Option<&'a Model>,
    /// The resolved provider for `model` (`ModelRuntime::get_provider`).
    pub provider: Option<&'a dyn Provider>,
    pub runtime_view: &'a dyn BugReportRuntimeView,
    pub thinking_level: ModelThinkingLevel,
    pub extensions: &'a [Extension],
    pub extension_errors: &'a [(String, String)],
    pub global_settings: &'a crate::coding_agent::core::settings_manager::SettingsValue,
    pub project_settings: &'a crate::coding_agent::core::settings_manager::SettingsValue,
    /// Upstream `process`/`node:os` facts (see [`HostEnvironment`]).
    pub host_environment: &'a HostEnvironment,
}

/// Upstream `collectBugReportMetadata`, with `createdAt` riding the live
/// clock. See [`collect_bug_report_metadata_at_ms`] for the deterministic
/// form.
pub fn collect_bug_report_metadata(
    options: CollectBugReportMetadataOptions<'_>,
) -> serde_json::Value {
    collect_bug_report_metadata_at_ms(options, crate::ai::now_ms())
}

/// `collectBugReportMetadata` with the instant injected (upstream
/// `new Date().toISOString()` at call time).
pub fn collect_bug_report_metadata_at_ms(
    options: CollectBugReportMetadataOptions<'_>,
    now_ms: i64,
) -> serde_json::Value {
    let hint = options
        .hint
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(str::to_string);
    let mut session = serde_json::Map::new();
    session.insert(
        "id".into(),
        serde_json::Value::String(options.session_id.to_string()),
    );
    session.insert(
        "included".into(),
        serde_json::Value::Bool(options.include_session),
    );
    session.insert(
        "summaryIncluded".into(),
        serde_json::Value::Bool(options.include_summary),
    );
    session.insert(
        "messageCount".into(),
        serde_json::json!(options.message_count),
    );
    if options.include_session {
        session.insert(
            "cwd".into(),
            serde_json::Value::String(options.cwd.to_string()),
        );
    }
    let mut extension_errors = Vec::new();
    for (path, error) in options.extension_errors {
        extension_errors.push(serde_json::json!({ "path": path, "error": error }));
    }
    serde_json::json!({
        "schemaVersion": BUG_REPORT_SCHEMA_VERSION,
        "id": options.id.map(str::to_string).unwrap_or_else(uuid_v7),
        "createdAt": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms),
        "hint": hint.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
        "environment": collect_environment(options.host_environment),
        "session": serde_json::Value::Object(session),
        "model": options.model.map(describe_model).unwrap_or(serde_json::Value::Null),
        "provider": match options.provider {
            Some(provider) => describe_provider(options.runtime_view, provider),
            None => serde_json::Value::Null,
        },
        "thinkingLevel": options.thinking_level,
        "extensions": options.extensions.iter().map(describe_extension).collect::<Vec<_>>(),
        "extensionErrors": extension_errors,
        "settings": {
            "global": redact_settings(options.global_settings),
            "project": redact_settings(options.project_settings),
        },
    })
}

/// Upstream `ReadonlySessionManager` subset the diagnostics collector reads.
pub trait ReadonlyBugReportSession {
    /// Upstream `getEntries()`.
    fn get_entries(&self) -> Vec<crate::coding_agent::session_manager::SessionEntry>;
    /// Upstream `getSessionId()`.
    fn get_session_id(&self) -> String;
}

impl ReadonlyBugReportSession for SessionManager {
    fn get_entries(&self) -> Vec<crate::coding_agent::session_manager::SessionEntry> {
        SessionManager::get_entries(self)
    }

    fn get_session_id(&self) -> String {
        SessionManager::get_session_id(self).to_string()
    }
}

/// Collect failed assistant turns without collecting conversation content.
pub fn collect_bug_report_diagnostics(
    session_manager: &dyn ReadonlyBugReportSession,
    crashes: &[CrashRecord],
) -> serde_json::Value {
    collect_bug_report_diagnostics_at(
        session_manager.get_session_id(),
        &session_manager.get_entries(),
        crashes,
    )
}

/// Diagnostics over explicit inputs (the pure half; the public wrapper reads
/// them off the session manager like upstream).
fn collect_bug_report_diagnostics_at(
    session_id: String,
    entries: &[crate::coding_agent::session_manager::SessionEntry],
    crashes: &[CrashRecord],
) -> serde_json::Value {
    let mut assistant = Vec::new();
    let mut assistant_message_count: u64 = 0;
    for entry in entries {
        let crate::coding_agent::session_manager::SessionEntry::Message(message_entry) = entry
        else {
            continue;
        };
        let AgentMessage::Assistant(message) = &message_entry.message else {
            continue;
        };
        assistant_message_count += 1;
        let diagnostics = message
            .diagnostics
            .as_ref()
            .map(|diagnostics| {
                serde_json::to_value(diagnostics).unwrap_or(serde_json::Value::Array(Vec::new()))
            })
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let has_diagnostics = message
            .diagnostics
            .as_ref()
            .map(Vec::is_empty)
            .unwrap_or(true);
        if has_diagnostics
            && message.stop_reason != StopReason::Error
            && message.stop_reason != StopReason::Aborted
            && message.error_message.is_none()
        {
            continue;
        }
        let mut record = serde_json::Map::new();
        record.insert(
            "entryId".into(),
            serde_json::Value::String(message_entry.id.clone()),
        );
        record.insert(
            "timestamp".into(),
            serde_json::Value::String(message_entry.timestamp.clone()),
        );
        record.insert(
            "provider".into(),
            serde_json::Value::String(message.provider.clone()),
        );
        record.insert(
            "model".into(),
            serde_json::Value::String(message.model.clone()),
        );
        record.insert("api".into(), serde_json::Value::String(message.api.clone()));
        record.insert(
            "stopReason".into(),
            serde_json::to_value(message.stop_reason).unwrap_or(serde_json::Value::Null),
        );
        if let Some(raw_stop_reason) = &message.raw_stop_reason {
            record.insert(
                "rawStopReason".into(),
                serde_json::Value::String(raw_stop_reason.clone()),
            );
        }
        if let Some(error_message) = &message.error_message {
            record.insert(
                "errorMessage".into(),
                serde_json::Value::String(error_message.clone()),
            );
        }
        record.insert("diagnostics".into(), diagnostics);
        assistant.push(serde_json::Value::Object(record));
    }
    // Upstream strips `notified` from each crash record.
    let crashes: Vec<serde_json::Value> = crashes
        .iter()
        .map(|record| {
            let mut record = record.clone();
            record.notified = None;
            serde_json::to_value(&record).unwrap_or(serde_json::Value::Null)
        })
        .collect();
    serde_json::json!({
        "schemaVersion": BUG_REPORT_SCHEMA_VERSION,
        "sessionId": session_id,
        "entryCount": entries.len(),
        "assistantMessageCount": assistant_message_count,
        "assistant": assistant,
        "crashes": crashes,
    })
}

/// Upstream `BugReportBundle`. Metadata/diagnostics are the pinned JSON
/// objects above.
#[derive(Debug, Clone, PartialEq)]
pub struct BugReportBundle {
    pub metadata: serde_json::Value,
    pub diagnostics: serde_json::Value,
    pub session_jsonl: Option<String>,
    pub summary: Option<String>,
}

/// Upstream `BugReportSessionEntryData`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BugReportSessionEntryData {
    pub id: String,
    pub created_at: String,
    pub hint: Option<String>,
    pub session_included: bool,
    pub summary_included: bool,
    pub delivery: BugReportDelivery,
    pub path: Option<String>,
}

/// Upstream `BugReportSessionEntryData["delivery"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BugReportDelivery {
    Zip,
    Upload,
}

/// Upstream `BugReportFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BugReportFile {
    pub name: String,
    pub content_type: String,
    pub data: String,
}

/// Files shared by upload and zip export.
pub fn bug_report_files(bundle: &BugReportBundle) -> Vec<BugReportFile> {
    let mut files = vec![
        BugReportFile {
            name: "report.json".to_string(),
            content_type: "application/json".to_string(),
            data: format!(
                "{}\n",
                serde_json::to_string_pretty(&bundle.metadata).unwrap_or_default()
            ),
        },
        BugReportFile {
            name: "diagnostics.json".to_string(),
            content_type: "application/json".to_string(),
            data: format!(
                "{}\n",
                serde_json::to_string_pretty(&bundle.diagnostics).unwrap_or_default()
            ),
        },
    ];
    if let Some(session_jsonl) = &bundle.session_jsonl {
        files.push(BugReportFile {
            name: "session.jsonl".to_string(),
            content_type: "application/x-ndjson".to_string(),
            data: session_jsonl.clone(),
        });
    }
    if let Some(summary) = &bundle.summary {
        files.push(BugReportFile {
            name: "summary.md".to_string(),
            content_type: "text/markdown".to_string(),
            data: if summary.ends_with('\n') {
                summary.clone()
            } else {
                format!("{summary}\n")
            },
        });
    }
    files
}

/// Upstream `writeBugReportArchive`.
pub fn write_bug_report_archive(bundle: &BugReportBundle, file_path: &str) -> std::io::Result<()> {
    write_zip_archive(file_path, &bug_report_files(bundle), crate::ai::now_ms())
}

/// Upstream `bugReportArchiveFileName`.
pub fn bug_report_archive_file_name(id: &str) -> String {
    format!("pi-bug-report-{id}.zip")
}

// ---------------------------------------------------------------------------
// Vendored `utils/zip.ts` — the small, classic ZIP archives used by bug
// reports (deflate level 6 + CRC-32, UTF-8 flag 0x0800). Upstream stamps the
// DOS time from local wall-clock parts; the port renders UTC (disclosed).
// ---------------------------------------------------------------------------

/// Upstream `ZipEntry`.
pub struct ZipEntry {
    pub name: String,
    pub data: String,
}

struct DosDateTime {
    time: u16,
    day: u16,
}

/// Upstream `dosDateTime(new Date())`; `now_ms` renders in UTC.
fn dos_date_time(now_ms: i64) -> DosDateTime {
    let days = now_ms.div_euclid(86_400_000);
    let seconds_of_day = now_ms.rem_euclid(86_400_000) / 1000;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let year = year.clamp(1980, (1 << 7) + 1980 - 1);
    let month = m as u16;
    let day_of_month = d as u16;
    let hour = (seconds_of_day / 3600) as u16;
    let minute = ((seconds_of_day % 3600) / 60) as u16;
    let second = (seconds_of_day % 60) as u16;
    DosDateTime {
        time: (hour << 11) | (minute << 5) | (second >> 1),
        day: (((year - 1980) as u16) << 9) | (month << 5) | day_of_month,
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    crc.sum()
}

/// Test-support re-export of the CRC-32 used by the archive writer (the
/// oracle tests recompute it over inflated payloads like the node driver).
#[cfg(test)]
pub(crate) fn test_support_crc32(data: &[u8]) -> u32 {
    crc32(data)
}

fn deflate_raw(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

/// Upstream `createZipArchive`.
pub fn create_zip_archive(entries: &[ZipEntry], now_ms: i64) -> std::io::Result<Vec<u8>> {
    let DosDateTime { time, day } = dos_date_time(now_ms);
    let mut files: Vec<u8> = Vec::new();
    let mut directory: Vec<u8> = Vec::new();
    let mut offset: u32 = 0;
    for entry in entries {
        let name = entry.name.as_bytes();
        let data = entry.data.as_bytes();
        let compressed = deflate_raw(data)?;
        let checksum = crc32(data);
        files.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        files.extend_from_slice(&20u16.to_le_bytes());
        files.extend_from_slice(&0x0800u16.to_le_bytes());
        files.extend_from_slice(&8u16.to_le_bytes());
        files.extend_from_slice(&time.to_le_bytes());
        files.extend_from_slice(&day.to_le_bytes());
        files.extend_from_slice(&checksum.to_le_bytes());
        files.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        files.extend_from_slice(&(data.len() as u32).to_le_bytes());
        files.extend_from_slice(&(name.len() as u16).to_le_bytes());
        files.extend_from_slice(&0u16.to_le_bytes());
        files.extend_from_slice(name);
        files.extend_from_slice(&compressed);

        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        directory.extend_from_slice(&20u16.to_le_bytes());
        directory.extend_from_slice(&20u16.to_le_bytes());
        directory.extend_from_slice(&0x0800u16.to_le_bytes());
        directory.extend_from_slice(&8u16.to_le_bytes());
        directory.extend_from_slice(&time.to_le_bytes());
        directory.extend_from_slice(&day.to_le_bytes());
        directory.extend_from_slice(&checksum.to_le_bytes());
        directory.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
        directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&0u32.to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name);

        offset += (30 + name.len() + compressed.len()) as u32;
    }
    let mut end: Vec<u8> = Vec::new();
    end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    end.extend_from_slice(&(directory.len() as u32).to_le_bytes());
    end.extend_from_slice(&offset.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    files.extend_from_slice(&directory);
    files.extend_from_slice(&end);
    Ok(files)
}

/// Upstream `writeZipArchive`.
pub fn write_zip_archive(
    file_path: &str,
    entries: &[BugReportFile],
    now_ms: i64,
) -> std::io::Result<()> {
    let entries: Vec<ZipEntry> = entries
        .iter()
        .map(|file| ZipEntry {
            name: file.name.clone(),
            data: file.data.clone(),
        })
        .collect();
    std::fs::write(file_path, create_zip_archive(&entries, now_ms)?)
}

// ---------------------------------------------------------------------------
// Summary generation
// ---------------------------------------------------------------------------

const BUG_SUMMARY_SYSTEM_PROMPT: &str = "You are helping a user file a bug report about pi, the coding agent they are talking to. You will be shown the conversation transcript. Write a report for the pi developers describing what the user was doing and what went wrong.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the report.";

const BUG_SUMMARY_INSTRUCTIONS: &str = "Write the bug report in Markdown with these sections:\n\n## What the user was doing\nOne short paragraph.\n\n## What went wrong\nConcrete description of the failure: wrong output, errors, hangs, tool failures, unexpected behavior. Quote error messages and tool output verbatim where they exist.\n\n## Steps to reproduce\nNumbered list, as specific as the transcript allows.\n\n## Relevant details\nTool calls involved, files touched, model behavior, anything else that helps a developer reproduce or locate the problem.\n\nDo not include file contents, secrets, or credentials from the transcript; refer to files by path only. Keep the report factual and concise.";

/// `ModelThinkingLevel` minus the `"off"` member (the upstream
/// `ThinkingLevel` union); `None` for `"off"`.
fn non_off_thinking_level(
    level: ModelThinkingLevel,
) -> Option<crate::ai::types::primitives::ThinkingLevel> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(crate::ai::types::primitives::ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(crate::ai::types::primitives::ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(crate::ai::types::primitives::ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(crate::ai::types::primitives::ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(crate::ai::types::primitives::ThinkingLevel::Xhigh),
        ModelThinkingLevel::Max => Some(crate::ai::types::primitives::ThinkingLevel::Max),
    }
}

/// Upstream `selectMessages`: newest-first fill up to the token budget; the
/// first (newest) message always fits.
fn select_messages(messages: &[AgentMessage], token_budget: u64) -> Vec<&AgentMessage> {
    let mut selected: Vec<&AgentMessage> = Vec::new();
    let mut tokens: u64 = 0;
    for message in messages.iter().rev() {
        let next = crate::coding_agent::core::compaction::estimate_tokens(message);
        if !selected.is_empty() && tokens + next > token_budget {
            break;
        }
        selected.push(message);
        tokens += next;
    }
    selected.reverse();
    selected
}

/// Upstream `GenerateBugReportSummaryOptions`.
pub struct GenerateBugReportSummaryOptions<'a> {
    pub messages: &'a [AgentMessage],
    pub hint: Option<&'a str>,
    pub model: &'a Model,
    pub api_key: Option<&'a str>,
    pub headers: Option<&'a ProviderHeaders>,
    pub env: Option<&'a ProviderEnv>,
    pub signal: CancellationToken,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub stream_fn: Option<StreamFn>,
    pub retry: Option<&'a RetryPolicy>,
    pub session_id: Option<&'a str>,
}

/// Ask the session model for a report when the user does not share the
/// transcript. Returns the report text; errors carry the upstream
/// `Error.message` strings.
pub async fn generate_bug_report_summary(
    options: GenerateBugReportSummaryOptions<'_>,
) -> Result<String, String> {
    let model = options.model;
    let context_window = if model.context_window > 0 {
        model.context_window
    } else {
        128_000
    };
    let budget = (context_window as f64 * 0.6).floor() as u64;
    let messages = select_messages(options.messages, budget);
    let hint = options.hint.map(str::trim).filter(|hint| !hint.is_empty());
    // Upstream serializes the SELECTED messages, not the full transcript.
    let selected: Vec<AgentMessage> = messages.iter().map(|message| (*message).clone()).collect();
    let llm_messages = crate::coding_agent::core::messages::convert_to_llm(&selected);
    let conversation = crate::coding_agent::core::compaction::serialize_conversation(&llm_messages);
    let mut parts: Vec<String> = Vec::new();
    if messages.len() < options.messages.len() {
        parts.push(format!(
            "Note: only the last {} of {} messages are shown.",
            messages.len(),
            options.messages.len()
        ));
    }
    parts.push(format!("<conversation>\n{conversation}\n</conversation>"));
    if let Some(hint) = hint {
        parts.push(format!("<user-report>\n{hint}\n</user-report>"));
    }
    parts.push(BUG_SUMMARY_INSTRUCTIONS.to_string());
    let prompt = parts.join("\n\n");
    let request_options = crate::ai::types::options::SimpleStreamOptions {
        stream: crate::ai::types::options::StreamOptions {
            max_tokens: Some(if model.max_tokens > 0 {
                model.max_tokens.min(4096)
            } else {
                4096
            }),
            signal: Some(options.signal),
            api_key: options.api_key.map(str::to_string),
            headers: options.headers.cloned(),
            env: options.env.cloned(),
            session_id: options.session_id.map(str::to_string),
            ..crate::ai::types::options::StreamOptions::default()
        },
        reasoning: options
            .thinking_level
            .and_then(non_off_thinking_level)
            .filter(|_| model.reasoning),
        ..crate::ai::types::options::SimpleStreamOptions::default()
    };
    let context = normalize_context(&crate::ai::Context {
        system_prompt: Some(BUG_SUMMARY_SYSTEM_PROMPT.to_string()),
        messages: vec![crate::ai::types::Message::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                text: prompt,
                text_signature: None,
            })]),
            timestamp: crate::ai::now_ms(),
        })],
        tools: None,
    });
    let mut callbacks = RetryCallbacks::default();
    let response = crate::coding_agent::core::compaction::complete_summarization(
        model,
        context,
        request_options,
        options.stream_fn,
        options.retry,
        &mut callbacks,
    )
    .await;
    if response.stop_reason == StopReason::Aborted {
        return Err("Bug report summary was cancelled".to_string());
    }
    if let Some(failure) = crate::coding_agent::core::compaction::get_summarization_failure(
        &response,
        "Bug report summary",
    ) {
        return Err(failure);
    }
    if response.content.iter().any(|block| {
        matches!(
            block,
            crate::ai::types::message::AssistantBlock::ToolCall(_)
        )
    }) {
        return Err("Bug report summary attempted to call a tool".to_string());
    }
    let text = assistant_blocks_text(&response.content, "\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("Bug report summary was empty".to_string());
    }
    Ok(text)
}

#[cfg(test)]
#[path = "bug_report_tests.rs"]
mod tests;
