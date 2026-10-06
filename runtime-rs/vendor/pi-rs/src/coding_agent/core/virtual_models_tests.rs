//! Oracle + unit tests for [`super::virtual_models`]. The byte-exact
//! expectations come from `tests/fixtures/core_delta_oracle/virtual-models/`
//! (verbatim upstream sources under `node --experimental-strip-types`; see
//! the fixture manifest for the SHA pins and canonicalization notes).

use serde_json::{json, Value};

use crate::agent_core::types::AgentMessage;
use crate::ai::auth::context::default_provider_auth_context;
use crate::ai::auth::resolve::ModelsError;
use crate::ai::auth::types::{ApiKeyAuthInput, AuthOperationOptions, AuthResult, ProviderAuth};
use crate::ai::models::Provider;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::{ModelThinkingLevel, StopReason, Usage, UsageCost};
use crate::ai::types::{Model, ModelInput};
use crate::coding_agent::core::virtual_models::{
    create_virtual_model, find_latest_response, get_branch_selection, get_virtual_model_state,
    is_virtual_model, unrouted_stream, unrouted_stream_error_message, with_virtual_models,
    BranchSelection, CreateVirtualModelOptions, VIRTUAL_MODEL_API,
};
use crate::coding_agent::session_manager::SessionEntry;
use std::sync::Arc;

const ORACLE: &str = include_str!(
    "../../../tests/fixtures/core_delta_oracle/virtual-models/virtual_models.oracle.json"
);

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

/// The canonicalization the capture applies: `thinkingLevelMap` keys are
/// sorted (the Rust `ThinkingLevelMap` is a BTreeMap; entry sets are equal).
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if key == "thinkingLevelMap" {
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

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    canonicalize(serde_json::to_value(value).expect("serializable"))
}

fn physical_model(provider: &str, id: &str) -> Model {
    serde_json::from_value(json!({
        "id": id,
        "name": format!("Name {id}"),
        "api": "openai-completions",
        "provider": provider,
        "baseUrl": "https://api.example.com/v1",
        "reasoning": false,
        "thinkingLevelMap": { "off": null },
        "input": ["text"],
        "cost": { "input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0.2 },
        "contextWindow": 8192,
        "maxTokens": 1024
    }))
    .expect("wire model")
}

fn wire_usage() -> Usage {
    Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: UsageCost::default(),
    }
}

fn wire_assistant_json(
    api: &str,
    provider: &str,
    model: &str,
    stop_reason: StopReason,
    extra: Value,
) -> Value {
    let mut value = json!({
        "role": "assistant",
        "content": [],
        "api": api,
        "provider": provider,
        "model": model,
        "usage": serde_json::to_value(wire_usage()).unwrap(),
        "stopReason": stop_reason,
        "timestamp": 1,
    });
    if let Value::Object(map) = extra {
        for (key, entry) in map {
            value.as_object_mut().unwrap().insert(key, entry);
        }
    }
    value
}

fn wire_assistant(
    api: &str,
    provider: &str,
    model: &str,
    stop_reason: StopReason,
    extra: Value,
) -> AssistantMessage {
    serde_json::from_value(wire_assistant_json(
        api,
        provider,
        model,
        stop_reason,
        extra,
    ))
    .expect("wire assistant")
}

/// Serialized assistant message with the `role` tag the port carries only on
/// the enveloping `Message` (upstream's structural objects keep `role`).
fn assistant_json(message: &AssistantMessage) -> Value {
    let mut value = serde_json::to_value(message).expect("serializable");
    value
        .as_object_mut()
        .unwrap()
        .insert("role".to_string(), json!("assistant"));
    canonicalize(value)
}

#[test]
fn create_virtual_model_matches_the_captured_grid() {
    let cases = vec![
        (
            "defaults",
            CreateVirtualModelOptions {
                provider: "llama.cpp".to_string(),
                id: "auto".to_string(),
                name: "Auto".to_string(),
                thinking_levels: None,
                context_window: None,
                max_tokens: None,
                input: None,
            },
        ),
        (
            "thinkingLevels_subset",
            CreateVirtualModelOptions {
                provider: "p".to_string(),
                id: "m".to_string(),
                name: "M".to_string(),
                thinking_levels: Some(vec![
                    ModelThinkingLevel::Off,
                    ModelThinkingLevel::Medium,
                    ModelThinkingLevel::Max,
                ]),
                context_window: None,
                max_tokens: None,
                input: None,
            },
        ),
        (
            "thinkingLevels_all",
            CreateVirtualModelOptions {
                provider: "p".to_string(),
                id: "m".to_string(),
                name: "M".to_string(),
                thinking_levels: Some(vec![
                    ModelThinkingLevel::Off,
                    ModelThinkingLevel::Minimal,
                    ModelThinkingLevel::Low,
                    ModelThinkingLevel::Medium,
                    ModelThinkingLevel::High,
                    ModelThinkingLevel::Xhigh,
                    ModelThinkingLevel::Max,
                ]),
                context_window: None,
                max_tokens: None,
                input: None,
            },
        ),
        (
            "thinkingLevels_unknown",
            CreateVirtualModelOptions {
                provider: "p".to_string(),
                id: "m".to_string(),
                name: "M".to_string(),
                // The capture drives `["medium"]` (a non-"off" level without
                // "off"): reasoning flips on and every other level maps to
                // null. The untyped JS-only `["off", "bogus"]` shape is not
                // representable in the typed port (disclosed).
                thinking_levels: Some(vec![ModelThinkingLevel::Medium]),
                context_window: None,
                max_tokens: None,
                input: None,
            },
        ),
        (
            "limits",
            CreateVirtualModelOptions {
                provider: "p".to_string(),
                id: "m".to_string(),
                name: "M".to_string(),
                thinking_levels: None,
                context_window: Some(1000),
                max_tokens: Some(100),
                input: None,
            },
        ),
        (
            "input_text_only",
            CreateVirtualModelOptions {
                provider: "p".to_string(),
                id: "m".to_string(),
                name: "M".to_string(),
                thinking_levels: None,
                context_window: None,
                max_tokens: None,
                input: Some(vec![ModelInput::Text]),
            },
        ),
    ];
    let grid = scenario("create_virtual_model_grid");
    for (name, definition) in cases {
        let model = create_virtual_model(&definition);
        assert_eq!(
            to_json(&model),
            grid[name],
            "create_virtual_model {name} diverges"
        );
        assert_eq!(model.api, VIRTUAL_MODEL_API);
        // Wire round trip keeps the entry byte-stable.
        let serialized = serde_json::to_string(&model).unwrap();
        let reparsed: Model = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed, model);
    }
}

#[test]
fn is_virtual_model_matches_the_captured_grid() {
    let grid = scenario("is_virtual_model");
    let virtual_model = create_virtual_model(&CreateVirtualModelOptions {
        provider: "p".to_string(),
        id: "v".to_string(),
        name: "Virtual v".to_string(),
        thinking_levels: None,
        context_window: None,
        max_tokens: None,
        input: None,
    });
    assert_eq!(
        is_virtual_model(&virtual_model),
        grid["virtualModel"].as_bool().unwrap()
    );
    let physical = physical_model("p", "gpt");
    assert_eq!(
        is_virtual_model(&physical),
        grid["physicalModel"].as_bool().unwrap()
    );
    let assistant_virtual = wire_assistant("pi-virtual", "p", "v", StopReason::Stop, json!({}));
    assert_eq!(
        is_virtual_model(&assistant_virtual),
        grid["assistantVirtual"].as_bool().unwrap()
    );
    let assistant_physical = wire_assistant(
        "anthropic-messages",
        "anthropic",
        "claude",
        StopReason::Stop,
        json!({}),
    );
    assert_eq!(
        is_virtual_model(&assistant_physical),
        grid["assistantPhysical"].as_bool().unwrap()
    );
}

#[test]
fn find_latest_response_matches_the_captured_grid() {
    let grid = scenario("find_latest_response");
    let assistant = move |stop_reason: StopReason, extra: Value| {
        AgentMessage::Assistant(
            serde_json::from_value::<AssistantMessage>(wire_assistant_json(
                "api",
                "p",
                "m",
                stop_reason,
                extra,
            ))
            .expect("wire assistant"),
        )
    };
    let user: AgentMessage =
        serde_json::from_value(json!({"role": "user", "content": "hi", "timestamp": 1})).unwrap();
    let tool_result: AgentMessage = serde_json::from_value(json!({
        "role": "toolResult",
        "toolCallId": "c1",
        "toolName": "bash",
        "content": [{"type": "text", "text": "x"}],
        "details": {},
        "isError": false,
        "timestamp": 2
    }))
    .unwrap();
    let branch = vec![
        user.clone(),
        assistant(StopReason::Error, json!({"errorMessage": "boom"})),
        assistant(StopReason::Aborted, json!({})),
        tool_result,
        assistant(StopReason::ToolUse, json!({})),
        assistant(StopReason::Stop, json!({"model": "final"})),
    ];
    assert_eq!(
        assistant_json(find_latest_response(&branch).unwrap()),
        grid["mixed"],
        "mixed branch"
    );
    assert!(find_latest_response(&[]).is_none());
    assert!(grid["empty"].is_null());
    let failures = vec![
        assistant(StopReason::Error, json!({})),
        assistant(StopReason::Aborted, json!({})),
    ];
    assert!(find_latest_response(&failures).is_none());
    assert!(grid["onlyFailures"].is_null());
    let first_wins = vec![assistant(StopReason::Stop, json!({"model": "first"})), user];
    assert_eq!(
        assistant_json(find_latest_response(&first_wins).unwrap()),
        grid["firstWins"]
    );
}

fn branch_entry(value: Value) -> SessionEntry {
    serde_json::from_value(value).expect("wire session entry")
}

fn message_entry(id: &str, message: AgentMessage) -> SessionEntry {
    branch_entry(json!({
        "type": "message",
        "id": id,
        "parentId": null,
        "timestamp": format!("t-{id}"),
        "message": serde_json::to_value(message).unwrap(),
    }))
}

fn model_change_entry(id: &str, provider: &str, model_id: &str) -> SessionEntry {
    branch_entry(json!({
        "type": "model_change",
        "id": id,
        "parentId": null,
        "timestamp": format!("t-{id}"),
        "provider": provider,
        "modelId": model_id,
    }))
}

fn state_entry(id: &str, provider: &str, model_id: &str, state: Value) -> SessionEntry {
    branch_entry(json!({
        "type": "custom",
        "customType": "pi.virtual-model-state",
        "data": { "provider": provider, "modelId": model_id, "state": state },
        "id": id,
        "parentId": null,
        "timestamp": format!("t-{id}"),
    }))
}

fn get_model(provider: &str, model_id: &str) -> Option<Model> {
    match (provider, model_id) {
        ("p", "virtual") => Some(create_virtual_model(&CreateVirtualModelOptions {
            provider: provider.to_string(),
            id: model_id.to_string(),
            name: format!("Virtual {model_id}"),
            thinking_levels: None,
            context_window: None,
            max_tokens: None,
            input: None,
        })),
        ("p", "physical") => Some(physical_model(provider, model_id)),
        _ => None,
    }
}

#[test]
fn get_branch_selection_matches_the_captured_grid() {
    let grid = scenario("get_branch_selection_grid");
    let usage = wire_usage();
    let assistant = |provider: &str, model: &str| {
        AgentMessage::Assistant(
            serde_json::from_value::<AssistantMessage>(json!({
                "role": "assistant",
                "content": [],
                "api": "openai-completions",
                "provider": provider,
                "model": model,
                "usage": serde_json::to_value(usage).unwrap(),
                "stopReason": "stop",
                "timestamp": 1
            }))
            .expect("wire assistant"),
        )
    };
    let cases: Vec<(&str, Vec<SessionEntry>)> = vec![
        ("empty_branch", vec![]),
        (
            "only_user",
            vec![message_entry(
                "u1",
                AgentMessage::User(
                    serde_json::from_value(
                        json!({"role": "user", "content": "hi", "timestamp": 1}),
                    )
                    .unwrap(),
                ),
            )],
        ),
        (
            "physical_response",
            vec![
                message_entry("m1", assistant("p", "physical")),
                message_entry("m2", assistant("other", "model")),
            ],
        ),
        (
            "model_change_last",
            vec![
                message_entry("m1", assistant("p", "physical")),
                model_change_entry("c1", "p", "physical"),
            ],
        ),
        (
            "virtual_model_change_holds",
            vec![
                message_entry("m1", assistant("p", "physical")),
                model_change_entry("c1", "p", "virtual"),
                message_entry("m2", assistant("p", "physical")),
            ],
        ),
        (
            "virtual_model_change_unregistered",
            vec![
                message_entry("m1", assistant("p", "physical")),
                model_change_entry("c1", "p", "gone"),
                message_entry("m2", assistant("p", "physical")),
            ],
        ),
        (
            "virtual_assistant_skipped",
            vec![
                model_change_entry("c1", "p", "virtual"),
                message_entry("m1", assistant("p", "virtual")),
                message_entry("m2", assistant("p", "physical")),
            ],
        ),
        (
            "earliest_model_change_wins_over_later_virtual_change",
            vec![
                model_change_entry("c1", "p", "virtual"),
                message_entry("m1", assistant("p", "physical")),
                model_change_entry("c2", "other", "physical"),
                message_entry("m2", assistant("p", "physical")),
            ],
        ),
    ];
    for (name, branch) in cases {
        let observed = get_branch_selection(&branch, &get_model);
        let expected = &grid[name];
        let expected = if expected.is_null() {
            None
        } else {
            Some(expected.clone())
        };
        assert_eq!(
            observed.map(|selection: BranchSelection| serde_json::to_value(selection).unwrap()),
            expected,
            "get_branch_selection {name} diverges"
        );
    }
}

#[test]
fn get_virtual_model_state_matches_the_captured_grid() {
    let grid = scenario("get_virtual_model_state_grid");
    let branch = vec![
        state_entry("s1", "p", "virtual", json!({"tier": "fast"})),
        state_entry("s2", "p", "other", json!({"tier": "nope"})),
        branch_entry(json!({
            "type": "custom",
            "customType": "pi.other",
            "data": { "provider": "p", "modelId": "virtual", "state": "x" },
            "id": "s3",
            "parentId": null,
            "timestamp": "t"
        })),
        branch_entry(json!({
            "type": "custom",
            "customType": "pi.virtual-model-state",
            "id": "s4",
            "parentId": null,
            "timestamp": "t"
        })),
        branch_entry(json!({
            "type": "custom",
            "customType": "pi.virtual-model-state",
            "data": { "provider": 42, "modelId": "virtual", "state": "bad" },
            "id": "s5",
            "parentId": null,
            "timestamp": "t"
        })),
        state_entry("s6", "p", "virtual", json!({"tier": "newest"})),
    ];
    assert_eq!(
        get_virtual_model_state(&branch, "p", "virtual"),
        Some(grid["match"].clone())
    );
    assert_eq!(
        get_virtual_model_state(&branch, "p", "other"),
        Some(grid["other_model"].clone())
    );
    assert!(grid["unknown"].is_null());
    assert_eq!(get_virtual_model_state(&branch, "p", "gone"), None);
    assert_eq!(get_virtual_model_state(&[], "p", "virtual"), None);
}

#[test]
fn state_entry_json_schema_is_pinned() {
    let grid = scenario("state_entry_json_schema");
    let entry = state_entry("s1", "p", "virtual", json!({"tier": "fast", "note": null}));
    assert_eq!(
        serde_json::to_string(&entry).unwrap(),
        grid["serialized"].as_str().unwrap()
    );
    // The data object key order (provider, modelId, state) rides on the
    // VirtualModelStateData declaration order.
    let data = crate::coding_agent::core::virtual_models::VirtualModelStateData {
        provider: "p".to_string(),
        model_id: "virtual".to_string(),
        state: json!({"tier": "fast", "note": null}),
    };
    let keys: Vec<String> = serde_json::to_value(&data)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let expected: Vec<String> = grid["dataKeyOrder"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, expected);
}

// ---------------------------------------------------------------------------
// withVirtualModels
// ---------------------------------------------------------------------------

/// The wrapped provider fixture mirroring the capture's object literal.
struct StubProvider {
    filter: Option<bool>,
    filter_all: Option<bool>,
}

impl StubProvider {
    fn catalog_model(&self, id: &str) -> Model {
        physical_model("p", id)
    }
}

impl Provider for StubProvider {
    fn id(&self) -> &str {
        "p"
    }

    fn name(&self) -> &str {
        "Provider P"
    }

    fn base_url(&self) -> Option<&str> {
        Some("https://p.example.com")
    }

    fn auth(&self) -> &ProviderAuth {
        static AUTH: std::sync::OnceLock<ProviderAuth> = std::sync::OnceLock::new();
        AUTH.get_or_init(ProviderAuth::default)
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        Ok(vec![
            self.catalog_model("shared"),
            self.catalog_model("other"),
        ])
    }

    fn get_all_models(&self) -> Result<Vec<crate::ai::types::AnyModel>, ModelsError> {
        let image: crate::ai::types::AnyModel = serde_json::from_value(json!({
            "id": "img",
            "name": "Name img",
            "api": "openai-completions",
            "provider": "p",
            "baseUrl": "https://api.example.com/v1",
            "input": ["text"],
            "cost": { "input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0.2 },
            "type": "image",
            "output": ["image"]
        }))
        .expect("wire image model");
        Ok(vec![
            crate::ai::types::AnyModel::Chat(self.catalog_model("shared")),
            crate::ai::types::AnyModel::Chat(self.catalog_model("other")),
            image,
        ])
    }

    fn filter_models(
        &self,
        models: &[Model],
        _credential: Option<&crate::ai::auth::types::Credential>,
    ) -> Option<Vec<Model>> {
        self.filter.map(|_| {
            models
                .iter()
                .filter(|model| model.id != "other")
                .cloned()
                .collect()
        })
    }

    fn has_filter_models(&self) -> bool {
        self.filter.is_some()
    }

    fn filter_all_models(
        &self,
        models: &[crate::ai::types::AnyModel],
        _credential: Option<&crate::ai::auth::types::Credential>,
    ) -> Option<Vec<crate::ai::types::AnyModel>> {
        self.filter_all.map(|_| {
            models
                .iter()
                .filter(|model| model.id() != "other")
                .cloned()
                .collect()
        })
    }

    fn has_filter_all_models(&self) -> bool {
        self.filter_all.is_some()
    }

    fn api_for(&self, _model: &Model) -> Option<Arc<dyn crate::ai::ApiImpl>> {
        Some(Arc::new(StubApi))
    }
}

/// Eventless API implementation for the api_for presence assertions.
struct StubApi;

impl crate::ai::ApiImpl for StubApi {
    fn stream(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        _model: &Model,
        _ctx: &crate::ai::transcript::TranscriptContext,
        _options: &crate::ai::types::options::StreamOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        rx
    }

    fn stream_simple(
        &self,
        _cfg: &crate::ai::ProviderConfig,
        _model: &Model,
        _ctx: &crate::ai::transcript::TranscriptContext,
        _options: &crate::ai::types::options::SimpleStreamOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        rx
    }
}

fn virtual_models_for(provider_id: &str, ids: &[&str]) -> Vec<Model> {
    ids.iter()
        .map(|id| {
            create_virtual_model(&CreateVirtualModelOptions {
                provider: provider_id.to_string(),
                id: (*id).to_string(),
                name: format!("Virtual {id}"),
                thinking_levels: None,
                context_window: None,
                max_tokens: None,
                input: None,
            })
        })
        .collect()
}

fn auto_model() -> Model {
    create_virtual_model(&CreateVirtualModelOptions {
        provider: "llama.cpp".to_string(),
        id: "auto".to_string(),
        name: "Virtual auto".to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Off, ModelThinkingLevel::Medium]),
        context_window: Some(8192),
        max_tokens: None,
        input: None,
    })
}

#[tokio::test]
async fn with_virtual_models_keyless_matches_the_capture() {
    let grid = scenario("with_virtual_models_keyless");
    let provider = with_virtual_models(
        "llama.cpp",
        None,
        vec![
            auto_model(),
            virtual_models_for("llama.cpp", &["eco"]).pop().unwrap(),
        ],
    );
    assert_eq!(provider.id(), grid["id"].as_str().unwrap());
    assert_eq!(provider.name(), grid["name"].as_str().unwrap());
    assert_eq!(provider.base_url(), None);
    assert_eq!(to_json(&provider.get_models().unwrap()), grid["models"]);
    assert!(grid["getAllModelsUndefined"].as_bool().unwrap());

    // The api-key auth literal: name "Virtual model", resolve always reports
    // configured with source "virtual" and an empty auth object.
    let api_key = provider
        .auth()
        .api_key
        .as_ref()
        .expect("virtual api key auth");
    assert_eq!(api_key.name(), grid["auth"]["apiKeyName"].as_str().unwrap());
    let auth_context = default_provider_auth_context();
    let input = ApiKeyAuthInput {
        ctx: &auth_context as &dyn crate::ai::auth::types::AuthContext,
        credential: None,
        options: &AuthOperationOptions::NONE,
    };
    let resolved: AuthResult = api_key
        .resolve(input)
        .await
        .expect("resolve ok")
        .expect("configured");
    assert_eq!(
        resolved.source.as_deref(),
        Some(grid["auth"]["resolve"]["source"].as_str().unwrap())
    );
    // Upstream resolves `{ auth: {}, source: "virtual" }` — no credential
    // material at all.
    assert!(resolved.auth.api_key.is_none());
    assert!(resolved.auth.headers.is_none());
    assert!(resolved.auth.base_url.is_none());
    assert!(resolved.env.is_none());

    // filterModels/filterAllModels/getAllModels are absent on the keyless
    // wrapper; the trait's getAllModels falls back to getModels like the
    // upstream `getAllModels?.() ?? getModels()` call sites.
    assert!(provider.filter_models(&[], None).is_none());
    assert!(!provider.has_filter_models());
    assert!(!provider.has_filter_all_models());
    assert!(provider.filter_all_models(&[], None).is_none());

    // The unrouted stream error text.
    let message = unrouted_stream(&auto_model(), &crate::ai::Context::default())
        .recv()
        .await;
    let Some(AssistantMessageEvent::Error { error, .. }) = message else {
        panic!("expected the unrouted error event");
    };
    let expected = &grid["unroutedMessage"];
    assert_eq!(error.api, expected["api"].as_str().unwrap());
    assert_eq!(error.provider, expected["provider"].as_str().unwrap());
    assert_eq!(error.model, expected["model"].as_str().unwrap());
    assert_eq!(
        error.error_message.as_deref(),
        Some(expected["errorMessage"].as_str().unwrap())
    );
    assert_eq!(to_json(&error.usage), expected["usage"]);
    assert_eq!(
        error.content.is_empty(),
        expected["content"].as_array().unwrap().is_empty()
    );
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(
        unrouted_stream_error_message(&auto_model()),
        expected["errorMessage"].as_str().unwrap()
    );
}

#[test]
fn with_virtual_models_wrapped_plain_matches_the_capture() {
    let grid = scenario("with_virtual_models_wrapped_plain");
    let provider = with_virtual_models(
        "p",
        Some(std::sync::Arc::new(StubProvider {
            filter: None,
            filter_all: None,
        })),
        virtual_models_for("p", &["shared", "extra"]),
    );
    assert_eq!(provider.name(), grid["name"].as_str().unwrap());
    assert_eq!(provider.base_url(), Some(grid["baseUrl"].as_str().unwrap()));
    assert_eq!(to_json(&provider.get_models().unwrap()), grid["models"]);
    assert_eq!(
        to_json(&provider.get_all_models().unwrap()),
        grid["allModels"]
    );
    // filterModels over the wrapper's own catalog plus an injected virtual.
    let mut full = provider.get_models().unwrap();
    full.push(virtual_models_for("p", &["injected"]).pop().unwrap());
    assert_eq!(
        to_json(&provider.filter_models(&full, None).unwrap()),
        grid["filterModels"]
    );
    assert_eq!(
        to_json(
            &provider
                .filter_models(&provider.get_models().unwrap(), None)
                .unwrap()
        ),
        grid["filterModelsOfFullList"]
    );
    assert!(grid["hasFilterAllModels"].as_bool().unwrap());
    assert!(provider
        .filter_all_models(&provider.get_all_models().unwrap(), None)
        .is_none());

    // Virtual models report no API implementation (the unrouted error text
    // rides on the wiring layer's dispatch; see unrouted_stream_error_message).
    let virtual_model = virtual_models_for("p", &["extra"]).pop().unwrap();
    assert!(provider.api_for(&virtual_model).is_none());
    // Physical models keep the wrapped provider's implementation.
    assert!(provider.api_for(&physical_model("p", "other")).is_some());
}

#[test]
fn with_virtual_models_wrapped_filtered_matches_the_capture() {
    let grid = scenario("with_virtual_models_wrapped_filtered");
    let provider = with_virtual_models(
        "p",
        Some(std::sync::Arc::new(StubProvider {
            filter: Some(true),
            filter_all: Some(true),
        })),
        virtual_models_for("p", &["shared", "extra"]),
    );
    assert_eq!(
        to_json(
            &provider
                .filter_models(&provider.get_models().unwrap(), None)
                .unwrap()
        ),
        grid["filterModels"]
    );
    assert_eq!(
        to_json(
            &provider
                .filter_all_models(&provider.get_all_models().unwrap(), None)
                .unwrap()
        ),
        grid["filterAllModels"]
    );
}

/// The refreshed catalog adds a physical "extra" model after registration;
/// the virtual model still hides it.
struct RefreshedProvider(StubProvider);

impl Provider for RefreshedProvider {
    fn id(&self) -> &str {
        "p"
    }

    fn name(&self) -> &str {
        "Provider P"
    }

    fn auth(&self) -> &ProviderAuth {
        static AUTH: std::sync::OnceLock<ProviderAuth> = std::sync::OnceLock::new();
        AUTH.get_or_init(ProviderAuth::default)
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        Ok(vec![
            self.0.catalog_model("shared"),
            self.0.catalog_model("extra"),
            self.0.catalog_model("other"),
        ])
    }

    fn get_all_models(&self) -> Result<Vec<crate::ai::types::AnyModel>, ModelsError> {
        Ok(self
            .get_models()?
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect())
    }
}

#[test]
fn with_virtual_models_catalog_refresh_keeps_virtual_hiding() {
    let grid = scenario("with_virtual_models_catalog_refresh");
    let provider = with_virtual_models(
        "p",
        Some(std::sync::Arc::new(RefreshedProvider(StubProvider {
            filter: None,
            filter_all: None,
        }))),
        virtual_models_for("p", &["extra"]),
    );
    assert_eq!(to_json(&provider.get_models().unwrap()), grid["models"]);
}
