//! Oracle + unit tests for [`super::bug_report`]. The byte-exact
//! expectations come from `tests/fixtures/core_delta_oracle/bug-report/`
//! (verbatim upstream sources under `node --experimental-strip-types`; see
//! the fixture manifest for the SHA pins, stubs, and canonicalization).

use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::agent_core::types::AgentMessage;
use crate::ai::models::Provider;
use crate::ai::retry::RetryPolicy;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::{ProviderEnv, ProviderHeaders};
use crate::ai::types::primitives::{ModelThinkingLevel, StopReason};
use crate::ai::types::Model;
use crate::coding_agent::core::bug_report::{
    bug_report_archive_file_name, bug_report_files, collect_bug_report_diagnostics,
    collect_bug_report_metadata_at_ms, generate_bug_report_summary, get_pi_user_agent_from,
    is_sensitive_key, redact_json_value, redact_settings, redact_url, BugReportBundle,
    BugReportRuntimeView, CollectBugReportMetadataOptions, GenerateBugReportSummaryOptions,
    HostEnvironment,
};
use crate::coding_agent::core::compaction::StreamFn;
use crate::coding_agent::core::crash_log::CrashRecord;
use crate::coding_agent::core::provider_composer::AuthStatus;
use crate::coding_agent::core::settings_manager::SettingsValue;
use crate::coding_agent::extensions::types::{SourceInfo, SourceOrigin, SourceScope};

const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_delta_oracle/bug-report/bug_report.oracle.json");

fn scenario(name: &str) -> Value {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle json");
    oracle["scenarios"]
        .as_array()
        .expect("scenarios array")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("scenario {name} missing"))["observed"]
        .clone()
}

/// The capture canonicalizes the BTreeMap-backed model objects
/// (`thinkingLevelMap`, `samplingParams`): identical entry sets, sorted key
/// order. Applied to both sides before comparison.
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if matches!(key.as_str(), "thinkingLevelMap" | "samplingParams") {
                        if let Value::Object(entries) = value {
                            let mut sorted: Vec<(String, Value)> = entries.into_iter().collect();
                            sorted.sort_by(|a, b| a.0.cmp(&b.0));
                            return (key, Value::Object(sorted.into_iter().collect()));
                        }
                    }
                    (key, canonicalize(value))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

#[test]
fn redact_url_grid_matches() {
    let grid = scenario("redact_url_grid");
    let cases = vec![
        (
            "credentials_and_params",
            "https://user:pass@api.example.com/v1/x?api_key=abc&token=XYZ&q=hello&Password=pw&token=dup#frag",
        ),
        ("unchanged", "https://example.com/path"),
        ("not_a_url", "not a url"),
        ("mailto", "mailto:user@host"),
        ("userinfo_only", "https://token@host.com/"),
        ("hyphen_keys", "http://h.com/?authorization=Bearer%20x&a=1&api-key=k"),
        ("camel_key", "https://h.com/?apiKey=v"),
        ("upper_key", "https://h.com/?API_KEY=v"),
        ("encoded_value", "https://h.com/?x=a%20b&token=s"),
        ("nested_scheme", "socks5:https://u:p@h.com/?token=t"),
        ("uppercase_url", "HTTPS://U:P@H.COM/?TOKEN=x"),
        ("boundary_miss", "https://h.com/?refresh_token=a&sessionid=b&secret-key=c"),
        ("empty_value", "https://h.com/?token="),
        ("no_query_with_creds", "https://u:p@h.com/x"),
        ("plus_value", "https://h.com/?q=a+b&access_token=z"),
        ("trailing_query", "https://h.com/x?"),
        ("keys_only", "https://h.com/?token"),
        ("credential_subpath", "https://h.com/?credential=abc&other=keep"),
        ("oauth_cookie", "https://h.com/?OAUTH=1&cookie=2&session=3"),
        ("multiple_sensitive", "https://h.com/?b=2&api_key=x&c=3&api_key=y"),
    ];
    for (name, value) in cases {
        assert_eq!(redact_url(value), grid[name].as_str().unwrap(), "{name}");
    }
}

#[test]
fn sensitive_key_boundaries_match() {
    for key in [
        "apiKey",
        "API_KEY",
        "api-key",
        "secret-key",
        "refresh_token",
        "Password",
        "SECRET_KEY",
        "authToken",
    ] {
        assert!(is_sensitive_key(key), "{key} must be sensitive");
    }
    for key in ["sessionid", "myKey", "OAUTH", "session", "keys", "tokenize"] {
        assert!(!is_sensitive_key(key), "{key} must not be sensitive");
    }
}

#[test]
fn redact_json_value_grid_matches() {
    let grid = scenario("redact_json_value_grid");
    assert_eq!(
        redact_json_value(&json!({
            "apiKey": "sk-123",
            "nested": { "token": "t", "keep": "yes", "deeper": { "SECRET_KEY": "s", "arr": ["plain", "https://u:p@h.com/?x=1&password=y"] } }
        })),
        grid["nested"]
    );
    assert_eq!(
        redact_json_value(&json!({ "token": null, "keep": null })),
        grid["null_child_kept"]
    );
    assert_eq!(
        redact_json_value(&json!(["https://u:p@h.com/", "not a url", 42, true, null])),
        grid["url_strings"]
    );
    assert_eq!(
        redact_json_value(
            &json!({ "secretToken": "s", "authToken": "a", "refreshToken": "r", "sessionid": "ok", "myKey": "ok" })
        ),
        grid["camel_boundaries"]
    );
    assert_eq!(
        redact_json_value(&json!({ "a": 1, "b": [1, 2, { "c": 3 }] })),
        grid["passthrough"]
    );
    assert_eq!(redact_json_value(&json!({})), grid["empty"]);
}

fn settings_value(pairs: Vec<(&str, Value)>) -> SettingsValue {
    SettingsValue::Obj(
        pairs
            .into_iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    crate::coding_agent::core::bug_report::json_to_settings_value(value),
                )
            })
            .collect(),
    )
}

#[test]
fn redact_settings_matches() {
    let grid = scenario("redact_settings");
    let settings = settings_value(vec![
        ("trackingId", json!("track-me")),
        ("deviceId", json!("device-me")),
        ("theme", json!("dark")),
        ("defaultProvider", json!("anthropic")),
        (
            "providerAuth",
            json!({ "anthropic": { "apiKey": "sk", "token": "t", "expiresIn": 3600 } }),
        ),
        (
            "nested",
            json!({ "apiKeyEnv": "ANTHROPIC_API_KEY", "callbackUrl": "https://u:p@h.com/cb?secret=s" }),
        ),
    ]);
    assert_eq!(redact_settings(&settings), grid["redacted"]);
    let keeps = redact_settings(&settings_value(vec![
        ("trackingId", json!("t")),
        ("other", json!(1.0)),
    ]));
    let keys: Vec<String> = keeps.as_object().unwrap().keys().cloned().collect();
    let expected: Vec<String> = grid["keepsUnknownTopLevel"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, expected);
}

// ---------------------------------------------------------------------------
// Metadata collection
// ---------------------------------------------------------------------------

fn host_environment(runtime: &str) -> HostEnvironment {
    let env: std::collections::BTreeMap<String, String> = [
        ("SHELL", "C:\\Program Files\\Git\\usr\\bin\\bash.exe"),
        ("TERM", "xterm-256color"),
        ("TERM_PROGRAM", "vscode"),
        ("TERM_PROGRAM_VERSION", "1.2.3"),
        ("COLORTERM", "truecolor"),
        ("TMUX", ""),
        ("SSH_TTY", "/dev/pts/3"),
        ("CI", "1"),
        ("PI_ZED", "last"),
        ("PI_ALPHA", "first"),
        ("PI_MIDDY", "middle"),
        ("PI_EMPTY", ""),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    HostEnvironment {
        runtime: runtime.to_string(),
        platform: "win32".to_string(),
        arch: "x64".to_string(),
        os_release: "10.0.26200".to_string(),
        os_version: "Windows 11 Pro".to_string(),
        env,
    }
}

fn fixture_model() -> Model {
    serde_json::from_value(json!({
        "id": "claude-4",
        "name": "Claude 4",
        "api": "anthropic-messages",
        "provider": "stub",
        "baseUrl": "https://stub.example.com?api_key=secret",
        "reasoning": true,
        "thinkingLevelMap": { "off": null, "high": "high" },
        "input": ["text", "image"],
        "cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75 },
        "contextWindow": 200000,
        "maxTokens": 64000,
        "samplingParams": { "top_p": 0.9, "api_key_hint": "nope" },
        "compat": { "forceAdaptiveThinking": true, "password": "p" },
        "headers": { "x-a": "1", "X-B": "2" }
    }))
    .expect("wire model")
}

/// The stub provider mirroring the capture's `getProvider` object.
struct StubProviderFixture;

impl Provider for StubProviderFixture {
    fn id(&self) -> &str {
        "stub"
    }

    fn name(&self) -> &str {
        "Stub Provider"
    }

    fn base_url(&self) -> Option<&str> {
        Some("https://stub.example.com/v1?api_key=x")
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        static HEADERS: std::sync::OnceLock<ProviderHeaders> = std::sync::OnceLock::new();
        Some(HEADERS.get_or_init(|| {
            ProviderHeaders::from_iter([
                ("x-stub".to_string(), Some("1".to_string())),
                ("z-last".to_string(), Some("2".to_string())),
            ])
        }))
    }

    fn get_models(&self) -> Result<Vec<Model>, crate::ai::auth::resolve::ModelsError> {
        Ok(Vec::new())
    }

    fn auth(&self) -> &crate::ai::auth::types::ProviderAuth {
        static AUTH: std::sync::OnceLock<crate::ai::auth::types::ProviderAuth> =
            std::sync::OnceLock::new();
        AUTH.get_or_init(|| crate::ai::auth::types::ProviderAuth {
            api_key: Some(Arc::new(StubKeyAuth)),
            oauth: Some(Arc::new(StubOAuthAuth)),
        })
    }
}

struct StubKeyAuth;

impl crate::ai::auth::types::ApiKeyAuth for StubKeyAuth {
    fn name(&self) -> &str {
        "Stub key"
    }

    fn resolve<'a>(
        &'a self,
        _input: crate::ai::auth::types::ApiKeyAuthInput<'a>,
    ) -> futures::future::BoxFuture<
        'a,
        Result<Option<crate::ai::auth::types::AuthResult>, crate::ai::auth::types::AuthError>,
    > {
        Box::pin(async { Ok(None) })
    }
}

struct StubOAuthAuth;

impl crate::ai::auth::types::OAuthAuth for StubOAuthAuth {
    fn name(&self) -> &str {
        "Stub oauth"
    }

    fn login<'a>(
        &'a self,
        _interaction: crate::ai::auth::types::ProviderAuthInteraction,
    ) -> futures::future::BoxFuture<
        'a,
        Result<crate::ai::auth::types::OAuthCredential, crate::ai::auth::types::AuthError>,
    > {
        Box::pin(async { Err(crate::ai::auth::types::AuthError::Cancelled) })
    }

    fn refresh<'a>(
        &'a self,
        _credential: crate::ai::auth::types::OAuthCredential,
        _options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> futures::future::BoxFuture<
        'a,
        Result<crate::ai::auth::types::OAuthCredential, crate::ai::auth::types::AuthError>,
    > {
        Box::pin(async { Err(crate::ai::auth::types::AuthError::Cancelled) })
    }

    fn to_auth<'a>(
        &'a self,
        _credential: crate::ai::auth::types::OAuthCredential,
    ) -> futures::future::BoxFuture<
        'a,
        Result<crate::ai::auth::types::ModelAuth, crate::ai::auth::types::AuthError>,
    > {
        Box::pin(async { Ok(crate::ai::auth::types::ModelAuth::default()) })
    }
}

/// The stub runtime view mirroring the capture's `modelRuntime` object.
struct StubRuntimeView;

impl BugReportRuntimeView for StubRuntimeView {
    fn auth_status(&self, provider_id: &str) -> AuthStatus {
        if provider_id == "stub" {
            AuthStatus {
                configured: true,
                source: Some(
                    crate::coding_agent::core::provider_composer::AuthStatusSource::Runtime,
                ),
                label: Some("STUB_KEY".to_string()),
            }
        } else {
            AuthStatus {
                configured: false,
                source: None,
                label: None,
            }
        }
    }

    fn using_oauth(&self, provider_id: &str) -> bool {
        provider_id == "stub"
    }

    fn registered_provider_ids(&self) -> Vec<String> {
        vec!["stub".to_string(), "other".to_string()]
    }
}

fn fixture_extensions() -> Vec<crate::coding_agent::extensions::types::Extension> {
    use crate::coding_agent::extensions::types::Extension;
    let source_info = |source: &str, scope: SourceScope, origin: SourceOrigin| SourceInfo {
        path: String::new(),
        source: source.to_string(),
        scope,
        origin,
        base_dir: None,
    };
    let extension = |path: &str, info: SourceInfo, hidden: bool| Extension {
        path: path.to_string(),
        resolved_path: path.to_string(),
        hidden,
        replaceable: false,
        source_info: info,
        handlers: Default::default(),
        tools: Default::default(),
        live_tools: Default::default(),
        message_renderers: Default::default(),
        markdown_transformer: None,
        entry_renderers: None,
        commands: Default::default(),
        flags: Default::default(),
        shortcuts: Default::default(),
    };
    vec![
        extension(
            "/ext/one.ts",
            source_info("one.ts", SourceScope::Project, SourceOrigin::TopLevel),
            false,
        ),
        extension(
            "/ext/pkg/index.ts",
            source_info(
                "https://registry.example/pkg?token=leak",
                SourceScope::User,
                SourceOrigin::Package,
            ),
            true,
        ),
    ]
}

/// Substitute the placeholders the capture carries for port-side values.
fn substitute(value: &mut Value) {
    let text = serde_json::to_string(&value).expect("stringifiable");
    let text = text.replace("ORACLE-VERSION", env!("CARGO_PKG_VERSION"));
    *value = serde_json::from_str(&text).expect("valid json after substitution");
}

/// The capture-side canonicalization for metadata comparisons: substitute
/// the version placeholder and sort the BTreeMap-backed model objects.
fn expected_metadata(mut grid: Value) -> Value {
    substitute(&mut grid);
    canonicalize(grid)
}

fn metadata(options: CollectBugReportMetadataOptions<'_>) -> Value {
    let mut observed = collect_bug_report_metadata_at_ms(options, 0);
    observed["createdAt"] = json!("<createdAt>");
    substitute(&mut observed);
    canonicalize(observed)
}

#[test]
fn user_agent_format_matches() {
    assert_eq!(
        get_pi_user_agent_from("0.99.1", "win32", "node/v22.1.0", "x64"),
        "pi/0.99.1 (win32; node/v22.1.0; x64)"
    );
}

#[test]
fn metadata_with_model_matches_the_capture() {
    let grid = scenario("metadata_with_model");
    let host = host_environment("node/v25.8.2");
    let model = fixture_model();
    let provider = StubProviderFixture;
    let view = StubRuntimeView;
    let extensions = fixture_extensions();
    let global = settings_value(vec![
        ("trackingId", json!("track-me")),
        ("deviceId", json!("device-me")),
        ("theme", json!("dark")),
        ("apiKeyEnv", json!("sk-live")),
        ("nested", json!({ "token": "t" })),
    ]);
    let project = settings_value(vec![
        ("theme", json!("solarized")),
        ("authorization", json!("Bearer")),
    ]);
    let observed = metadata(CollectBugReportMetadataOptions {
        id: Some("fixed-id-1"),
        hint: Some("  it crashed  "),
        session_id: "session-1",
        cwd: "/work/project",
        include_session: true,
        include_summary: false,
        message_count: 3,
        model: Some(&model),
        provider: Some(&provider),
        runtime_view: &view,
        thinking_level: ModelThinkingLevel::High,
        extensions: &extensions,
        extension_errors: &[("/ext/broken.ts".to_string(), "boom".to_string())],
        global_settings: &global,
        project_settings: &project,
        host_environment: &host,
    });
    assert_eq!(observed, expected_metadata(grid));
}

#[test]
fn metadata_minimal_matches_the_capture() {
    let grid = scenario("metadata_minimal");
    let host = host_environment("node/v25.8.2");
    let view = StubRuntimeView;
    // The driver's metadataScenario defaults carry the module-level settings
    // fixtures for every scenario (only the fields in the overrides differ).
    let global = settings_value(vec![
        ("trackingId", json!("track-me")),
        ("deviceId", json!("device-me")),
        ("theme", json!("dark")),
        ("apiKeyEnv", json!("sk-live")),
        ("nested", json!({ "token": "t" })),
    ]);
    let project = settings_value(vec![
        ("theme", json!("solarized")),
        ("authorization", json!("Bearer")),
    ]);
    let observed = metadata(CollectBugReportMetadataOptions {
        id: Some("fixed-id-2"),
        hint: Some("   "),
        session_id: "session-1",
        cwd: "/work/project",
        include_session: false,
        include_summary: false,
        message_count: 3,
        model: None,
        provider: None,
        runtime_view: &view,
        thinking_level: ModelThinkingLevel::Off,
        extensions: &[],
        extension_errors: &[],
        global_settings: &global,
        project_settings: &project,
        host_environment: &host,
    });
    assert_eq!(observed, expected_metadata(grid));
}

#[test]
fn metadata_environment_matches_the_capture() {
    let grid = scenario("metadata_environment");
    let host = host_environment("node/<version>");
    let view = StubRuntimeView;
    let global = settings_value(vec![]);
    let project = settings_value(vec![]);
    let mut observed = collect_bug_report_metadata_at_ms(
        CollectBugReportMetadataOptions {
            id: Some("fixed-id-env"),
            hint: None,
            session_id: "s",
            cwd: "/c",
            include_session: false,
            include_summary: false,
            message_count: 0,
            model: None,
            provider: None,
            runtime_view: &view,
            thinking_level: ModelThinkingLevel::Off,
            extensions: &[],
            extension_errors: &[],
            global_settings: &global,
            project_settings: &project,
            host_environment: &host,
        },
        0,
    );
    observed["createdAt"] = json!("<createdAt>");
    observed["environment"]["osRelease"] = json!("<osRelease>");
    observed["environment"]["osVersion"] = json!("<osVersion>");
    substitute(&mut observed);
    // The scenario payload IS the environment object.
    assert_eq!(
        canonicalize(observed["environment"].clone()),
        expected_metadata(grid)
    );
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

fn crash_record(value: Value, notified: Option<bool>) -> CrashRecord {
    let mut record: CrashRecord = serde_json::from_value(value).expect("wire crash record");
    record.notified = notified;
    record
}

#[test]
fn diagnostics_match_the_capture() {
    let grid = scenario("diagnostics");
    let entries: Value = json!([
        {
            "type": "message", "id": "m1", "parentId": null, "timestamp": "t1",
            "message": {
                "role": "assistant", "content": [{ "type": "text", "text": "ok" }],
                "api": "anthropic-messages", "provider": "anthropic", "model": "claude",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "stop", "timestamp": 1
            }
        },
        { "type": "model_change", "id": "c1", "parentId": "m1", "timestamp": "t2", "provider": "anthropic", "modelId": "claude" },
        {
            "type": "message", "id": "m2", "parentId": "c1", "timestamp": "t3",
            "message": {
                "role": "assistant", "content": [],
                "api": "anthropic-messages", "provider": "anthropic", "model": "claude",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "error", "errorMessage": "http 500", "timestamp": 2
            }
        },
        {
            "type": "message", "id": "m3", "parentId": "m2", "timestamp": "t4",
            "message": {
                "role": "assistant", "content": [],
                "api": "openai-completions", "provider": "openai", "model": "gpt",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "aborted", "rawStopReason": "cancelled", "timestamp": 3
            }
        },
        {
            "type": "message", "id": "m4", "parentId": "m3", "timestamp": "t5",
            "message": {
                "role": "assistant", "content": [],
                "api": "api", "provider": "p", "model": "m",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "stop",
                "diagnostics": [{ "type": "retry", "timestamp": 7, "error": { "name": "HttpError", "message": "429" } }],
                "timestamp": 4
            }
        },
        { "type": "custom", "customType": "pi.note", "data": {}, "id": "cu1", "parentId": "m4", "timestamp": "t6" },
        { "type": "message", "id": "m5", "parentId": "cu1", "timestamp": "t7", "message": { "role": "user", "content": "hi", "timestamp": 5 } },
        {
            "type": "message", "id": "m6", "parentId": "m5", "timestamp": "t8",
            "message": {
                "role": "assistant", "content": [],
                "api": "api", "provider": "p", "model": "m",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "stop", "errorMessage": "leftover message", "timestamp": 6
            }
        }
    ]);
    let entries: Vec<crate::coding_agent::session_manager::SessionEntry> =
        serde_json::from_value(entries).expect("wire entries");
    let session = FixedSession {
        entries,
        session_id: "session-1".to_string(),
    };
    let crashes = vec![
        crash_record(
            json!({
                "timestamp": "2026-01-01T00:00:00.000Z",
                "version": "0.99.1",
                "kind": "fatal_error",
                "message": "first",
                "stack": "Error: first\n    at f (/ext/one.ts:1:1)",
                "sessionFile": "/s/session.jsonl",
                "cwd": "/work"
            }),
            Some(true),
        ),
        crash_record(
            json!({
                "timestamp": "2026-01-02T00:00:00.000Z",
                "version": "0.99.1",
                "kind": "uncaught_exception",
                "message": "second",
                "stack": null,
                "sessionFile": null,
                "cwd": "/work"
            }),
            None,
        ),
    ];
    let observed = collect_bug_report_diagnostics(&session, &crashes);
    assert_eq!(observed, expected_metadata(grid));

    let empty = FixedSession {
        entries: Vec::new(),
        session_id: "empty".to_string(),
    };
    assert_eq!(
        collect_bug_report_diagnostics(&empty, &[]),
        scenario("diagnostics_empty")
    );
    assert_eq!(
        scenario("bug_report_custom_entry_type").as_str().unwrap(),
        "pi.bug-report"
    );
}

struct FixedSession {
    entries: Vec<crate::coding_agent::session_manager::SessionEntry>,
    session_id: String,
}

impl crate::coding_agent::core::bug_report::ReadonlyBugReportSession for FixedSession {
    fn get_entries(&self) -> Vec<crate::coding_agent::session_manager::SessionEntry> {
        self.entries.clone()
    }

    fn get_session_id(&self) -> String {
        self.session_id.clone()
    }
}

// ---------------------------------------------------------------------------
// Files / archive
// ---------------------------------------------------------------------------

#[test]
fn bug_report_files_match_the_capture() {
    let grid = scenario("bug_report_files");
    let bundle = BugReportBundle {
        metadata: json!({ "id": "rep-1", "z": 1, "a": [1, 2] }),
        diagnostics: json!({ "sessionId": "s", "crashes": [] }),
        session_jsonl: Some("{\"type\":\"session\"}\n".to_string()),
        summary: Some("Report body".to_string()),
    };
    let files = bug_report_files(&bundle);
    let files_json: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "name": file.name,
                "contentType": file.content_type,
                "data": file.data,
            })
        })
        .collect();
    assert_eq!(Value::Array(files_json), grid["full"]);
    let no_trailing = BugReportBundle {
        summary: Some("no trailing".to_string()),
        ..bundle.clone()
    };
    let summary = bug_report_files(&no_trailing)
        .into_iter()
        .find(|file| file.name == "summary.md")
        .unwrap();
    assert_eq!(
        summary.content_type,
        grid["summary_newline_added"]["contentType"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        summary.data,
        grid["summary_newline_added"]["data"].as_str().unwrap()
    );
    let without_summary = BugReportBundle {
        summary: None,
        ..bundle.clone()
    };
    let names: Vec<String> = bug_report_files(&without_summary)
        .into_iter()
        .map(|file| file.name)
        .collect();
    let expected: Vec<String> = grid["without_summary"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, expected);
    let without_session = BugReportBundle {
        session_jsonl: None,
        ..bundle.clone()
    };
    let names: Vec<String> = bug_report_files(&without_session)
        .into_iter()
        .map(|file| file.name)
        .collect();
    let expected: Vec<String> = grid["without_session"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, expected);
    assert_eq!(
        bug_report_archive_file_name("rep-1"),
        grid["archive_name"].as_str().unwrap()
    );
}

#[test]
fn archive_structure_matches_the_capture() {
    use std::io::Read;

    let grid = scenario("bug_report_archive");
    let bundle = BugReportBundle {
        metadata: json!({ "id": "rep-1", "z": 1, "a": [1, 2] }),
        diagnostics: json!({ "sessionId": "s", "crashes": [] }),
        session_jsonl: Some("{\"type\":\"session\"}\n".to_string()),
        summary: Some("Report body".to_string()),
    };
    let path =
        std::env::temp_dir().join(format!("pi-bug-report-archive-{}.zip", std::process::id()));
    crate::coding_agent::core::bug_report::write_bug_report_archive(
        &bundle,
        path.to_str().unwrap(),
    )
    .expect("archive written");
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    let u16 = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u32 = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let mut entries = Vec::new();
    let mut offset = 0;
    while u32(offset) == 0x0403_4b50 {
        let flags = u16(offset + 6);
        let method = u16(offset + 8);
        let checksum = u32(offset + 14);
        let compressed_size = u32(offset + 18) as usize;
        let uncompressed_size = u32(offset + 22) as usize;
        let name_length = u16(offset + 26) as usize;
        let name =
            String::from_utf8(bytes[offset + 30..offset + 30 + name_length].to_vec()).unwrap();
        let compressed =
            &bytes[offset + 30 + name_length..offset + 30 + name_length + compressed_size];
        let mut decoder = flate2::read::DeflateDecoder::new(compressed);
        let mut inflated = String::new();
        decoder.read_to_string(&mut inflated).expect("inflatable");
        let expected_payload = match name.as_str() {
            "report.json" => format!(
                "{}\n",
                serde_json::to_string_pretty(&bundle.metadata).unwrap()
            ),
            "diagnostics.json" => format!(
                "{}\n",
                serde_json::to_string_pretty(&bundle.diagnostics).unwrap()
            ),
            "session.jsonl" => bundle.session_jsonl.clone().unwrap(),
            _ => format!("{}\n", bundle.summary.clone().unwrap()),
        };
        entries.push(json!({
            "name": name,
            "flagsHex": format!("0x{:x}", flags),
            "method": method,
            "crcMatchesPayload": checksum == crate::coding_agent::core::bug_report::test_support_crc32(inflated.as_bytes()),
            "uncompressedSize": uncompressed_size,
            "inflated": inflated,
            "decompressesRoundTrip": inflated == expected_payload,
        }));
        offset += 30 + name_length + compressed_size;
    }
    let observed = json!({
        "entryCount": entries.len(),
        "entries": entries,
        "endsWithCentralDirectory": u32(offset) == 0x0201_4b50,
        "trailingNewlineEnforced": entries
            .iter()
            .find(|entry| entry["name"] == "summary.md")
            .map(|entry| entry["inflated"].clone()),
    });
    assert_eq!(observed, expected_metadata(grid));
}

// ---------------------------------------------------------------------------
// Summary generation
// ---------------------------------------------------------------------------

fn summary_model(extra: Value) -> Model {
    let mut value = json!({
        "id": "claude-4",
        "name": "Claude 4",
        "api": "anthropic-messages",
        "provider": "stub",
        "baseUrl": "https://stub.example.com",
        "reasoning": false,
        "input": ["text"],
        "cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75 },
        "contextWindow": 200000,
        "maxTokens": 64000,
    });
    if let Value::Object(map) = extra {
        for (key, entry) in map {
            value.as_object_mut().unwrap().insert(key, entry);
        }
    }
    serde_json::from_value(value).expect("wire model")
}

fn fixture_messages() -> Vec<AgentMessage> {
    vec![
        AgentMessage::User(
            serde_json::from_value(json!({ "role": "user", "content": "hello", "timestamp": 1 }))
                .unwrap(),
        ),
        AgentMessage::User(
            serde_json::from_value(
                json!({ "role": "user", "content": "help me debug", "timestamp": 2 }),
            )
            .unwrap(),
        ),
    ]
}

fn big_messages() -> Vec<AgentMessage> {
    let mut messages = Vec::new();
    for index in 0..6 {
        messages.push(AgentMessage::User(
            serde_json::from_value(json!({
                "role": "user",
                "content": format!("u{index} {}", "x".repeat(400)),
                "timestamp": index
            }))
            .unwrap(),
        ));
        messages.push(AgentMessage::ToolResult(
            serde_json::from_value(json!({
                "role": "toolResult",
                "toolCallId": format!("c{index}"),
                "toolName": "bash",
                "content": [{ "type": "text", "text": format!("r{index} {}", "y".repeat(400)) }],
                "details": {},
                "isError": false,
                "timestamp": index
            }))
            .unwrap(),
        ));
    }
    messages
}

fn response_message(extra: Value) -> AssistantMessage {
    let mut value = json!({
        "role": "assistant",
        "content": [{ "type": "text", "text": "  Report body.\n\nSecond line.  " }],
        "api": "anthropic-messages",
        "provider": "stub",
        "model": "claude-4",
        "usage": {
            "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
        },
        "stopReason": "stop",
        "timestamp": 1
    });
    if let Value::Object(map) = extra {
        for (key, entry) in map {
            value.as_object_mut().unwrap().insert(key, entry);
        }
    }
    serde_json::from_value(value).expect("wire response")
}

struct RecordingStream {
    calls: std::sync::Mutex<
        Vec<(
            Model,
            crate::ai::transcript::TranscriptContext,
            crate::ai::types::options::SimpleStreamOptions,
        )>,
    >,
    response: AssistantMessage,
    throws: bool,
}

impl RecordingStream {
    fn stream_fn(self: &Arc<Self>) -> StreamFn {
        let stream = Arc::clone(self);
        Arc::new(move |model, context, options| {
            let stream = Arc::clone(&stream);
            Box::pin(async move {
                if stream.throws {
                    panic!("stream setup failed");
                }
                stream.calls.lock().unwrap().push((model, context, options));
                stream.response.clone()
            })
        })
    }
}

/// Rebuilds the upstream request-options object (upstream key order).
fn options_json(options: &crate::ai::types::options::SimpleStreamOptions) -> Value {
    let mut value = serde_json::Map::new();
    value.insert("maxTokens".into(), json!(options.stream.max_tokens));
    if options.stream.signal.is_some() {
        value.insert("signal".into(), json!("<signal>"));
    }
    if let Some(api_key) = &options.stream.api_key {
        value.insert("apiKey".into(), json!(api_key));
    }
    if let Some(headers) = &options.stream.headers {
        value.insert("headers".into(), serde_json::to_value(headers).unwrap());
    }
    if let Some(env) = &options.stream.env {
        value.insert("env".into(), serde_json::to_value(env).unwrap());
    }
    if let Some(session_id) = &options.stream.session_id {
        value.insert("sessionId".into(), json!(session_id));
    }
    if let Some(reasoning) = &options.reasoning {
        value.insert("reasoning".into(), serde_json::to_value(reasoning).unwrap());
    }
    if let Some(cache_retention) = &options.stream.cache_retention {
        value.insert(
            "cacheRetention".into(),
            serde_json::to_value(cache_retention).unwrap(),
        );
    }
    Value::Object(value)
}

fn system_prompt(context: &crate::ai::transcript::TranscriptContext) -> Value {
    match context.messages().first() {
        Some(crate::ai::types::Message::System(system)) => {
            crate::ai::transcript::content_text(&system.content).into()
        }
        _ => Value::Null,
    }
}

fn prompt_text(context: &crate::ai::transcript::TranscriptContext) -> Option<String> {
    match context.messages().last() {
        Some(crate::ai::types::Message::User(user)) => match &user.content {
            crate::ai::types::StringOrBlocks::Blocks(blocks) => match blocks.first() {
                Some(crate::ai::types::TextOrImageBlock::Text(text)) => Some(text.text.clone()),
                _ => None,
            },
            crate::ai::types::StringOrBlocks::Text(text) => Some(text.clone()),
        },
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn summary_scenario(
    name: &str,
    messages: &[AgentMessage],
    model: &Model,
    hint: Option<&str>,
    thinking_level: Option<ModelThinkingLevel>,
    session_id: Option<&str>,
    stream: Arc<RecordingStream>,
    retry: Option<&RetryPolicy>,
) {
    let outcome = generate_bug_report_summary(GenerateBugReportSummaryOptions {
        messages,
        hint,
        model,
        api_key: None,
        headers: None,
        env: None,
        signal: CancellationToken::new(),
        thinking_level,
        stream_fn: Some(stream.stream_fn()),
        retry,
        session_id,
    })
    .await;
    let observed = &scenario(name);
    let summary = match &outcome {
        Ok(text) => json!(text),
        Err(error) => json!(error),
    };
    assert_eq!(summary, observed["summary"], "{name}: summary");
    let calls = stream.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        observed["callCount"].as_u64().unwrap() as usize,
        "{name}: callCount"
    );
    let Some((streamed_model, context, options)) = calls.first() else {
        return;
    };
    assert_eq!(
        system_prompt(context),
        observed["systemPrompt"],
        "{name}: systemPrompt"
    );
    let expected_prompt = if observed["prompt"].is_null() {
        None
    } else {
        Some(observed["prompt"].clone())
    };
    assert_eq!(
        prompt_text(context).map(Value::String),
        expected_prompt,
        "{name}: prompt"
    );
    let mut options = options_json(options);
    canonicalize_generated_session_id(&mut options);
    assert_eq!(options, observed["options"], "{name}: options");
    assert_eq!(
        serde_json::to_value(streamed_model).unwrap(),
        serde_json::to_value(model).unwrap(),
        "{name}: model passthrough"
    );
}

/// `completeSummarization` routes through a fresh uuidv7 when the caller
/// passes no session id; canonicalize like the capture.
fn canonicalize_generated_session_id(options: &mut Value) {
    let generated =
        regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$")
            .expect("static regex");
    if let Some(session_id) = options.get_mut("sessionId") {
        if session_id
            .as_str()
            .map(|value| generated.is_match(value))
            .unwrap_or(false)
        {
            *session_id = json!("<sessionId>");
        }
    }
}

#[tokio::test]
async fn summary_scenarios_match_the_captures() {
    let small = fixture_messages();
    let big = big_messages();

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_basic",
        &small,
        &summary_model(json!({})),
        Some("  it hangs  "),
        Some(ModelThinkingLevel::High),
        Some("session-9"),
        Arc::clone(&stream),
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_no_hint",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_empty_hint",
        &small,
        &summary_model(json!({})),
        Some("   "),
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_truncation",
        &big,
        &summary_model(json!({ "contextWindow": 400 })),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    for (name, max_tokens) in [
        ("summary_max_tokens_zero", 0u64),
        ("summary_max_tokens_small", 100),
        ("summary_max_tokens_large", 999999),
    ] {
        let stream = Arc::new(RecordingStream {
            calls: std::sync::Mutex::new(Vec::new()),
            response: response_message(json!({})),
            throws: false,
        });
        summary_scenario(
            name,
            &small,
            &summary_model(json!({ "maxTokens": max_tokens })),
            None,
            None,
            None,
            stream,
            None,
        )
        .await;
    }

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_reasoning_model_thinking",
        &small,
        &summary_model(json!({ "reasoning": true })),
        None,
        Some(ModelThinkingLevel::Medium),
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_reasoning_model_off",
        &small,
        &summary_model(json!({ "reasoning": true })),
        None,
        Some(ModelThinkingLevel::Off),
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({})),
        throws: false,
    });
    summary_scenario(
        "summary_plain_model_thinking",
        &small,
        &summary_model(json!({})),
        None,
        Some(ModelThinkingLevel::High),
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({ "stopReason": "aborted" })),
        throws: false,
    });
    summary_scenario(
        "summary_aborted",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({ "stopReason": "error", "errorMessage": "http 500" })),
        throws: false,
    });
    summary_scenario(
        "summary_error_response",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(json!({ "stopReason": "length" })),
        throws: false,
    });
    summary_scenario(
        "summary_length_response",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(
            json!({ "content": [{ "type": "toolCall", "id": "c1", "name": "bash", "arguments": {} }] }),
        ),
        throws: false,
    });
    summary_scenario(
        "summary_tool_call",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;

    let stream = Arc::new(RecordingStream {
        calls: std::sync::Mutex::new(Vec::new()),
        response: response_message(
            json!({ "content": [{ "type": "thinking", "thinking": "hmm" }] }),
        ),
        throws: false,
    });
    summary_scenario(
        "summary_empty_text",
        &small,
        &summary_model(json!({})),
        None,
        None,
        None,
        stream,
        None,
    )
    .await;
}

/// Silence dead-code hints on the port-side retry seam (summary retries ride
/// the shared compaction retry policy; the oracle passes retry: undefined).
#[test]
fn retry_seam_accepts_a_policy() {
    let policy: Option<&RetryPolicy> = None;
    assert!(policy.is_none());
    let _ = StopReason::Stop;
    let _ = ProviderEnv::new();
    let _ = ProviderHeaders::default();
}
