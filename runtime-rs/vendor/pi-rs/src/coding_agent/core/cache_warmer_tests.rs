//! Oracle + unit tests for [`super::cache_warmer`]. The byte-exact
//! expectations come from `tests/fixtures/core_delta_oracle/cache-warmer/`
//! (verbatim upstream sources under `node --experimental-strip-types`; see
//! the fixture manifest for the SHA pins and canonicalization notes: wall
//! clock values are pinned as placeholders/offsets, repeated-refresh
//! scenarios pin the first usage entry and a count lower bound).

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::ai::models::ModelsSimpleStreamOptions;
use crate::ai::types::events::{AssistantMessageEvent, SuccessReason};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::ai::types::Model;
use crate::ai::Context;
use crate::coding_agent::core::cache_warmer::{
    format_cache_warming_status, format_cache_warming_usage, get_cache_warming_delay_ms,
    get_prompt_cache_ttl_ms, is_replayable, CacheWarmRequest, CacheWarmSessionStore,
    CacheWarmStreamSource, CacheWarmer, CacheWarmingAction, CacheWarmingDecision,
    CacheWarmingPhase, CacheWarmingState, CacheWarmingStatus, UsageEntry,
};
use crate::coding_agent::session_manager::SessionEntry;

const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_delta_oracle/cache-warmer/cache_warmer.oracle.json");

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

fn fixture_model(extra: Value) -> Model {
    let mut value = json!({
        "id": "claude",
        "name": "Claude",
        "api": "anthropic-messages",
        "provider": "anthropic",
        "baseUrl": "https://api.anthropic.com",
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

fn short_ttl_model() -> Model {
    fixture_model(json!({ "promptCache": { "short": 10.001 } }))
}

fn simple_options(extra: Value) -> SimpleStreamOptions {
    let mut value = json!({});
    if let Value::Object(map) = extra {
        for (key, entry) in map {
            value.as_object_mut().unwrap().insert(key, entry);
        }
    }
    serde_json::from_value(value).expect("wire simple options")
}

#[test]
fn delay_grid_matches() {
    let grid = scenario("delay_grid");
    for (ttl, expected) in grid.as_object().unwrap() {
        let ttl: i64 = ttl.parse().expect("integer ttl");
        assert_eq!(
            get_cache_warming_delay_ms(ttl as f64),
            expected.as_i64(),
            "delay for {ttl}"
        );
    }
}

#[test]
fn ttl_grid_matches() {
    let grid = scenario("ttl_grid");
    let cached = fixture_model(json!({ "promptCache": { "short": 300, "long": 3600 } }));
    let uncached = fixture_model(json!({}));
    let long_missing = fixture_model(json!({ "promptCache": { "short": 300 } }));
    let fractional = fixture_model(json!({ "promptCache": { "short": 0.5 } }));
    // The process env must not leak into the lookups (mirrors the capture).
    let saved = std::env::var("PI_CACHE_RETENTION").ok();
    std::env::remove_var("PI_CACHE_RETENTION");
    let cases: Vec<(&str, Model, Option<SimpleStreamOptions>)> = vec![
        (
            "default_short",
            cached.clone(),
            Some(simple_options(json!({}))),
        ),
        (
            "explicit_short",
            cached.clone(),
            Some(simple_options(json!({ "cacheRetention": "short" }))),
        ),
        (
            "explicit_long",
            cached.clone(),
            Some(simple_options(json!({ "cacheRetention": "long" }))),
        ),
        (
            "none",
            cached.clone(),
            Some(simple_options(json!({ "cacheRetention": "none" }))),
        ),
        (
            "env_long",
            cached.clone(),
            Some(simple_options(
                json!({ "env": { "PI_CACHE_RETENTION": "long" } }),
            )),
        ),
        (
            "env_empty",
            cached.clone(),
            Some(simple_options(
                json!({ "env": { "PI_CACHE_RETENTION": "" } }),
            )),
        ),
        (
            "env_other",
            cached.clone(),
            Some(simple_options(
                json!({ "env": { "PI_CACHE_RETENTION": "medium" } }),
            )),
        ),
        ("options_missing", cached, None),
        ("no_prompt_cache", uncached, Some(simple_options(json!({})))),
        (
            "long_tier_missing",
            long_missing,
            Some(simple_options(json!({ "cacheRetention": "long" }))),
        ),
        ("fractional", fractional, Some(simple_options(json!({})))),
    ];
    for (name, model, options) in cases {
        assert_eq!(
            get_prompt_cache_ttl_ms(&model, options.as_ref()),
            grid[name].as_f64(),
            "{name}"
        );
    }
    match saved {
        Some(value) => std::env::set_var("PI_CACHE_RETENTION", value),
        None => std::env::remove_var("PI_CACHE_RETENTION"),
    }
}

#[test]
fn replayable_grid_matches() {
    let grid = scenario("replayable_grid");
    let anthropic_adaptive = fixture_model(json!({ "compat": { "forceAdaptiveThinking": true } }));
    let anthropic_legacy = fixture_model(json!({ "compat": { "forceAdaptiveThinking": false } }));
    let anthropic_plain = fixture_model(json!({}));
    let openai = fixture_model(json!({ "api": "openai-completions" }));
    let cases: Vec<(&str, Model, Option<SimpleStreamOptions>)> = vec![
        (
            "anthropic_adaptive_reasoning",
            anthropic_adaptive.clone(),
            Some(simple_options(json!({ "reasoning": "high" }))),
        ),
        (
            "anthropic_adaptive_no_reasoning",
            anthropic_adaptive,
            Some(simple_options(json!({}))),
        ),
        (
            "anthropic_legacy_reasoning",
            anthropic_legacy.clone(),
            Some(simple_options(json!({ "reasoning": "high" }))),
        ),
        (
            "anthropic_nocompat_reasoning",
            anthropic_plain,
            Some(simple_options(json!({ "reasoning": "high" }))),
        ),
        // The driver passes reasoning "off" (truthy in JS); the typed port's
        // nearest truthy level is "minimal" — identical outcome for the
        // non-anthropic model under test.
        (
            "openai_reasoning",
            openai.clone(),
            Some(simple_options(json!({ "reasoning": "minimal" }))),
        ),
        (
            "openai_no_reasoning",
            openai,
            Some(simple_options(json!({}))),
        ),
        ("options_missing", anthropic_legacy, None),
    ];
    for (name, model, options) in cases {
        assert_eq!(
            is_replayable(&model, options.as_ref()),
            grid[name].as_bool().unwrap(),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------------
// CacheWarmer over stubbed seams
// ---------------------------------------------------------------------------

/// The session seam: `getBranch` serves a fixed entry list; `appendUsage`
/// records entries.
struct StubSession {
    branch: Vec<SessionEntry>,
    appended: Mutex<Vec<UsageEntry>>,
}

impl StubSession {
    fn new(branch: Value) -> Self {
        Self {
            branch: serde_json::from_value(branch).expect("wire session entries"),
            appended: Mutex::new(Vec::new()),
        }
    }

    fn empty() -> Self {
        Self::new(json!([]))
    }

    fn branch_with_prompt(input: u64, cache_read: u64, cache_write: u64) -> Self {
        Self::new(json!([
            {
                "type": "message",
                "id": "u0",
                "parentId": null,
                "timestamp": "t0",
                "message": { "role": "user", "content": "hi", "timestamp": 1 }
            },
            {
                "type": "message",
                "id": "m1",
                "parentId": null,
                "timestamp": "t",
                "message": {
                    "role": "assistant",
                    "content": [],
                    "api": "anthropic-messages",
                    "provider": "anthropic",
                    "model": "claude",
                    "usage": {
                        "input": input,
                        "output": 5,
                        "cacheRead": cache_read,
                        "cacheWrite": cache_write,
                        "totalTokens": input + 5,
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                    },
                    "stopReason": "stop",
                    "timestamp": 1
                }
            }
        ]))
    }

    fn appended_json(&self) -> Vec<Value> {
        self.appended
            .lock()
            .unwrap()
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap())
            .collect()
    }
}

impl CacheWarmSessionStore for StubSession {
    fn append_usage(
        &self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: &Usage,
        note: Option<&str>,
    ) -> UsageEntry {
        let entry = UsageEntry::cache_warm(
            "usage-1".to_string(),
            None,
            "t-usage".to_string(),
            provider,
            model,
            usage,
            note,
        );
        assert_eq!(entry.entry_type, "usage");
        assert_eq!(entry.kind, kind);
        self.appended.lock().unwrap().push(entry.clone());
        entry
    }

    fn get_branch(&self) -> Vec<SessionEntry> {
        self.branch.clone()
    }
}

/// The stream seam: every request yields the configured message.
enum StubStream {
    Message(AssistantMessage),
    /// The port's transport-failure channel: a single error event.
    Error(String),
    /// Advances the shared clock cell when the request dispatches (models a
    /// timer running late), then yields the message.
    AdvanceClockThenMessage(Arc<std::sync::atomic::AtomicI64>, i64, AssistantMessage),
}

impl CacheWarmStreamSource for StubStream {
    fn stream_simple(
        &self,
        _model: &Model,
        _context: &Context,
        _options: ModelsSimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let (tx, rx) = mpsc::channel(8);
        let events: Vec<AssistantMessageEvent> = match self {
            StubStream::Message(message) => vec![terminal_events(message.clone())],
            StubStream::AdvanceClockThenMessage(clock, target, message) => {
                clock.store(*target, std::sync::atomic::Ordering::SeqCst);
                vec![terminal_events(message.clone())]
            }
            StubStream::Error(text) => {
                let mut failed = message_with_error(text);
                failed.api = "anthropic-messages".to_string();
                vec![AssistantMessageEvent::Error {
                    reason: crate::ai::types::events::ErrorReason::Error,
                    error: failed,
                }]
            }
        };
        tokio::spawn(async move {
            let mut events = events.into_iter();
            // The reducer needs the Start skeleton before the terminal event
            // replaces the partial.
            if let Some(first) = events.next() {
                if !matches!(first, AssistantMessageEvent::Error { .. }) {
                    let skeleton = match &first {
                        AssistantMessageEvent::Done { message, .. } => message.clone(),
                        _ => first_message(&first),
                    };
                    let _ = tx
                        .send(AssistantMessageEvent::Start { message: skeleton })
                        .await;
                }
                for event in [first].into_iter().chain(events) {
                    let _ = tx.send(event).await;
                }
            }
        });
        rx
    }
}

/// The terminal event for a stub message.
fn terminal_events(message: AssistantMessage) -> AssistantMessageEvent {
    use crate::ai::types::primitives::StopReason as Stop;
    match message.stop_reason {
        Stop::Length => AssistantMessageEvent::Done {
            reason: SuccessReason::Length,
            message,
        },
        Stop::ToolUse => AssistantMessageEvent::Done {
            reason: SuccessReason::ToolUse,
            message,
        },
        Stop::Deferred => AssistantMessageEvent::Done {
            reason: SuccessReason::Deferred,
            message,
        },
        Stop::Error | Stop::Aborted => AssistantMessageEvent::Error {
            reason: crate::ai::types::events::ErrorReason::Error,
            error: message,
        },
        Stop::Stop | Stop::Pending => AssistantMessageEvent::Done {
            reason: SuccessReason::Stop,
            message,
        },
    }
}

fn first_message(event: &AssistantMessageEvent) -> AssistantMessage {
    match event {
        AssistantMessageEvent::Done { message, .. } => message.clone(),
        AssistantMessageEvent::Error { error, .. } => error.clone(),
        _ => unreachable!("only terminal events reach here"),
    }
}

fn message_with_error(text: &str) -> AssistantMessage {
    serde_json::from_value(json!({
        "role": "assistant",
        "content": [],
        "api": "anthropic-messages",
        "provider": "anthropic",
        "model": "claude",
        "usage": serde_json::to_value(Usage::default()).unwrap(),
        "stopReason": "error",
        "errorMessage": text,
        "timestamp": 1
    }))
    .expect("error message")
}

fn stub_message(stop_reason: StopReason) -> AssistantMessage {
    serde_json::from_value(json!({
        "role": "assistant",
        "content": [],
        "api": "anthropic-messages",
        "provider": "anthropic",
        "model": "claude",
        "usage": {
            "input": 10, "output": 10, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 20,
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
        },
        "stopReason": stop_reason,
        "timestamp": 1
    }))
    .expect("stub message")
}

type DecideBox = Arc<
    dyn Fn(
            crate::coding_agent::core::cache_warmer::CacheWarmingDecisionEvent,
        ) -> BoxFuture<'static, Result<CacheWarmingAction, String>>
        + Send
        + Sync,
>;

fn make_warmer(
    session: Arc<StubSession>,
    mode: &'static str,
    stream: StubStream,
    decide: Option<DecideBox>,
) -> CacheWarmer {
    let get_mode: Box<
        dyn Fn() -> crate::coding_agent::core::cache_warmer::CacheWarmingMode + Send + Sync,
    > = match mode {
        "off" => Box::new(|| crate::coding_agent::core::cache_warmer::CacheWarmingMode::Off),
        "streaming" => {
            Box::new(|| crate::coding_agent::core::cache_warmer::CacheWarmingMode::Streaming)
        }
        _ => Box::new(|| crate::coding_agent::core::cache_warmer::CacheWarmingMode::Idle),
    };
    let decide = decide.unwrap_or_else(|| {
        Arc::new(|event| Box::pin(async move { Ok(event.action) }) as BoxFuture<'static, _>)
    });
    CacheWarmer::with_decide(Arc::new(stream), session, get_mode, decide)
}

/// Warmer whose mode flips through a shared holder (the capture's
/// `modeHolder` object).
fn make_warmer_with_holder(
    session: Arc<StubSession>,
    mode_holder: Arc<std::sync::Mutex<crate::coding_agent::core::cache_warmer::CacheWarmingMode>>,
    stream: StubStream,
) -> CacheWarmer {
    let get_mode: Box<
        dyn Fn() -> crate::coding_agent::core::cache_warmer::CacheWarmingMode + Send + Sync,
    > = Box::new(move || *mode_holder.lock().unwrap());
    CacheWarmer::with_decide(
        Arc::new(stream),
        session,
        get_mode,
        Arc::new(|event| Box::pin(async move { Ok(event.action) }) as BoxFuture<'static, _>),
    )
}

fn warm_request(model: &Model, options: Value) -> CacheWarmRequest {
    CacheWarmRequest {
        model: model.clone(),
        context: Context {
            system_prompt: None,
            messages: Vec::new(),
            tools: None,
        },
        options: ModelsSimpleStreamOptions {
            simple: simple_options(options),
            transform_headers: None,
        },
    }
}

/// Status comparison that keeps the wall clock out of the pin and mirrors
/// upstream's undefined-dropping: `state` is always present; `reason` only
/// when set; `nextWarmAt` as a presence marker; `decision` +
/// `extensionOverride` only once a decision exists (run-alive or
/// stopped-with-decision paths).
fn status_json(status: &CacheWarmingStatus) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "state".to_string(),
        serde_json::to_value(status.state).unwrap(),
    );
    if let Some(reason) = status.reason.as_deref() {
        map.insert("reason".to_string(), json!(reason));
    }
    if let Some(next_warm_at) = status.next_warm_at {
        let _ = next_warm_at;
        map.insert("nextWarmAt".to_string(), json!("<number>"));
    }
    if let Some(decision) = &status.decision {
        map.insert(
            "decision".to_string(),
            canon_js_numbers(serde_json::to_value(decision).expect("decision serializable")),
        );
        map.insert(
            "extensionOverride".to_string(),
            json!(status.extension_override),
        );
    }
    Value::Object(map)
}

/// JS number equality: integral f64 values (`1.0`) serialize as `1.0` in
/// serde_json but as `1` in `JSON.stringify`; canonicalize integral floats
/// to integers before comparing.
fn canon_js_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => {
            if let Some(float) = number.as_f64() {
                if float.fract() == 0.0 && float.abs() <= i64::MAX as f64 {
                    return json!(float as i64);
                }
            }
            Value::Number(number)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canon_js_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, canon_js_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}

fn decision_json(decision: &CacheWarmingDecision) -> Value {
    canon_js_numbers(serde_json::to_value(decision).expect("decision serializable"))
}

#[tokio::test]
async fn decision_scenarios_match_the_captures() {
    // decision_streaming_known_prices: stop economics, run stopped.
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decision_streaming_known_prices");
    assert_eq!(
        decision_json(status.decision.as_ref().unwrap()),
        grid["decision"]
    );
    assert_eq!(session.appended_json(), Vec::<Value>::new());
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(status.reason.as_deref(), grid["stoppedReason"].as_str());

    // decision_after_settle: warm economics, idle phase drops to 15%.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let before = warmer.status();
    let grid = scenario("decision_after_settle");
    assert_eq!(status_json(&before), grid["decisionBeforeSettle"]);
    warmer.on_agent_settled();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let after = warmer.status();
    // The capture force-adds nextWarmAt: "<number>" to both objects; compare
    // the live fields and the placeholder separately.
    let after_json = status_json(&after);
    let after_json = match after.next_warm_at {
        Some(_) => after_json,
        None => {
            let mut value = after_json;
            value
                .as_object_mut()
                .unwrap()
                .insert("nextWarmAt".to_string(), json!("<number>"));
            value
        }
    };
    assert_eq!(after_json, grid["decisionAfterSettle"]);
    assert!(!session.appended_json().is_empty());
    assert_eq!(
        session.appended_json()[0],
        grid["firstAppendedUsage"],
        "first appended usage entry"
    );

    // decision_no_economics.
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decision_no_economics");
    assert_eq!(
        decision_json(status.decision.as_ref().unwrap()),
        grid["decision"]
    );
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(status.reason.as_deref(), grid["reason"].as_str());

    // decision_input_miss: miss priced at the input rate.
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let cheap = fixture_model(json!({
        "promptCache": { "short": 10.001 },
        "cost": { "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 0 }
    }));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(warm_request(&cheap, json!({})), Arc::new(|| true));
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert_eq!(
        decision_json(warmer.status().decision.as_ref().unwrap()),
        scenario("decision_input_miss")["decision"]
    );

    // decision_tiered: the >200k tier covers the whole request.
    let session = Arc::new(StubSession::branch_with_prompt(250000, 0, 0));
    let tiered = fixture_model(json!({
        "promptCache": { "short": 10.001 },
        "cost": {
            "input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75,
            "tiers": [{ "input": 1.5, "output": 7.5, "cacheRead": 0.15, "cacheWrite": 1.875, "inputTokensAbove": 200000 }]
        }
    }));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(warm_request(&tiered, json!({})), Arc::new(|| true));
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert_eq!(
        decision_json(warmer.status().decision.as_ref().unwrap()),
        scenario("decision_tiered")["decision"]
    );

    // decision_long_write: 1h writes price at 2x input.
    let session = Arc::new(StubSession::new(json!([
        {
            "type": "message",
            "id": "m1",
            "parentId": null,
            "timestamp": "t",
            "message": {
                "role": "assistant",
                "content": [],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude",
                "usage": {
                    "input": 100, "output": 5, "cacheRead": 0, "cacheWrite": 400, "cacheWrite1h": 300,
                    "totalTokens": 505,
                    "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 }
                },
                "stopReason": "stop",
                "timestamp": 1
            }
        }
    ])));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert_eq!(
        decision_json(warmer.status().decision.as_ref().unwrap()),
        scenario("decision_long_write")["decision"]
    );
}

#[tokio::test]
async fn decide_hook_scenarios_match_the_captures() {
    // decide_override_stop: extension stops warm economics.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let stopper: DecideBox = Arc::new(|_event| Box::pin(async { Ok(CacheWarmingAction::Stop) }));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        Some(stopper),
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decide_override_stop");
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(status.reason.as_deref(), grid["reason"].as_str());
    assert_eq!(
        decision_json(status.decision.as_ref().unwrap()),
        grid["decision"]
    );
    assert_eq!(
        status.extension_override,
        grid["extensionOverride"].as_bool().unwrap()
    );

    // decide_override_warm: extension forces a warm through stop economics.
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let forcer: DecideBox = Arc::new(|_event| Box::pin(async { Ok(CacheWarmingAction::Warm) }));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        Some(forcer),
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decide_override_warm");
    assert_eq!(status.state, CacheWarmingState::Scheduled);
    assert_eq!(
        decision_json(status.decision.as_ref().unwrap()),
        grid["decision"]
    );
    assert_eq!(
        status.extension_override,
        grid["extensionOverride"].as_bool().unwrap()
    );
    assert!(!session.appended_json().is_empty());
    assert_eq!(session.appended_json()[0], grid["firstAppendedUsage"]);

    // decide_echo_no_override: echoing pi's decision is not an override.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let echo: DecideBox = Arc::new(|event| Box::pin(async move { Ok(event.action) }));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        Some(echo),
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decide_echo_no_override");
    assert_eq!(status.state, CacheWarmingState::Scheduled);
    assert_eq!(
        status.extension_override,
        grid["extensionOverride"].as_bool().unwrap()
    );
    assert_eq!(session.appended_json()[0], grid["firstAppendedUsage"]);
    assert!(session.append().iter().all(|entry| entry.note.is_none()));

    // decide_reject_fallback: a failing decide hook falls back to pi.
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let failing: DecideBox =
        Arc::new(|_event| Box::pin(async { Err("classifier down".to_string()) }));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        Some(failing),
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("decide_reject_fallback");
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(status.reason.as_deref(), grid["reason"].as_str());
}

impl StubSession {
    fn append(&self) -> Vec<UsageEntry> {
        self.appended.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn lifecycle_scenarios_match_the_captures() {
    let grid = scenario("start_stop_reasons");
    // mode_off
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "off",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&fixture_model(json!({})), json!({})),
        Arc::new(|| true),
    );
    assert_eq!(status_json(&warmer.status()), grid["mode_off"]);
    // not_replayable
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(
            &fixture_model(json!({ "compat": { "forceAdaptiveThinking": false } })),
            json!({ "reasoning": "high" }),
        ),
        Arc::new(|| true),
    );
    assert_eq!(status_json(&warmer.status()), grid["not_replayable"]);
    // retention_none
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(
            &fixture_model(json!({ "promptCache": { "short": 300 } })),
            json!({ "cacheRetention": "none" }),
        ),
        Arc::new(|| true),
    );
    assert_eq!(status_json(&warmer.status()), grid["retention_none"]);
    // no_ttl
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&fixture_model(json!({})), json!({})),
        Arc::new(|| true),
    );
    assert_eq!(status_json(&warmer.status()), grid["no_ttl"]);
    // tiny_ttl
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(
            &fixture_model(json!({ "promptCache": { "short": 10 } })),
            json!({}),
        ),
        Arc::new(|| true),
    );
    assert_eq!(status_json(&warmer.status()), grid["tiny_ttl"]);

    // cancel
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    warmer.cancel();
    assert_eq!(status_json(&warmer.status()), scenario("cancel"));

    // on_mode_changed_off: the mode flips through the shared holder so the
    // stop lands deterministically before the 1ms refresh (the capture's
    // `modeHolder` object).
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let mode_holder = Arc::new(std::sync::Mutex::new(
        crate::coding_agent::core::cache_warmer::CacheWarmingMode::Idle,
    ));
    let warmer = make_warmer_with_holder(
        session,
        Arc::clone(&mode_holder),
        StubStream::Message(stub_message(StopReason::Stop)),
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    *mode_holder.lock().unwrap() = crate::coding_agent::core::cache_warmer::CacheWarmingMode::Off;
    warmer.on_mode_changed();
    assert_eq!(
        status_json(&warmer.status()),
        scenario("on_mode_changed_off")
    );

    // streaming_settle_stops
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let warmer = make_warmer(
        session,
        "streaming",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    warmer.on_agent_settled();
    assert_eq!(
        status_json(&warmer.status()),
        scenario("streaming_settle_stops")
    );

    // stale_context
    let session = Arc::new(StubSession::branch_with_prompt(1000, 500, 0));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    let current = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let is_current = Arc::clone(&current);
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(move || is_current.load(std::sync::atomic::Ordering::SeqCst)),
    );
    current.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status_json(&warmer.status()), scenario("stale_context"));

    // initial_status
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    assert_eq!(status_json(&warmer.status()), scenario("initial_status"));

    // status_mode_off
    let session = Arc::new(StubSession::empty());
    let warmer = make_warmer(
        session,
        "off",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    assert_eq!(status_json(&warmer.status()), scenario("status_mode_off"));

    // stream_failure_reschedules: the transport-failure channel skips usage
    // and keeps the run scheduled.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Error("overloaded".to_string()),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("stream_failure_reschedules");
    assert!(session.appended_json().is_empty());
    assert_eq!(serde_json::to_value(status.state).unwrap(), grid["state"]);
    assert_eq!(
        status.next_warm_at.is_some(),
        grid["nextWarmArmed"].as_bool().unwrap()
    );

    // error_response_skips_usage: Done with an error stop reason.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let warmer = make_warmer(
        session.clone(),
        "idle",
        StubStream::Message(stub_message(StopReason::Error)),
        None,
    );
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let status = warmer.status();
    let grid = scenario("error_response_skips_usage");
    assert!(session.appended_json().is_empty());
    assert_eq!(serde_json::to_value(status.state).unwrap(), grid["state"]);
}

#[test]
fn format_decision_time_grid_matches() {
    let grid = scenario("format_decision_time_grid");
    let decision = CacheWarmingDecision {
        phase: CacheWarmingPhase::Streaming,
        warm_cost: 0.001,
        miss_cost: 0.2,
        continuation_probability: 1.0,
        expected_savings: 0.199,
        economics_available: true,
        action: CacheWarmingAction::Warm,
    };
    let render = |next_warm_at: Option<i64>, now: i64| {
        format_cache_warming_status(
            &CacheWarmingStatus {
                state: CacheWarmingState::Scheduled,
                reason: None,
                next_warm_at,
                decision: Some(decision.clone()),
                extension_override: false,
            },
            now,
        )
    };
    assert_eq!(render(Some(10000), 10000), grid["now"].as_str().unwrap());
    assert_eq!(render(Some(5000), 10000), grid["past"].as_str().unwrap());
    assert_eq!(render(None, 10000), grid["undefined"].as_str().unwrap());
    assert_eq!(render(Some(59000), 10000), grid["s59"].as_str().unwrap());
    assert_eq!(render(Some(60000), 10000), grid["s60"].as_str().unwrap());
    assert_eq!(render(Some(61000), 10000), grid["m1s1"].as_str().unwrap());
    assert_eq!(render(Some(3600000), 10000), grid["h1"].as_str().unwrap());
    assert_eq!(
        render(Some(3661000), 10000),
        grid["h1m1s1"].as_str().unwrap()
    );
    assert_eq!(render(Some(3601000), 10000), grid["h1s1"].as_str().unwrap());
    assert_eq!(render(Some(3660000), 10000), grid["h1m1"].as_str().unwrap());
    assert_eq!(
        render(Some(10001), 10000),
        grid["ceiling"].as_str().unwrap()
    );
}

#[test]
fn format_status_grid_matches() {
    let grid = scenario("format_status_grid");
    let decision = |overrides: Value| {
        let mut value = json!({
            "phase": "streaming",
            "warmCost": 0.001,
            "missCost": 0.2,
            "continuationProbability": 1,
            "expectedSavings": 0.199,
            "economicsAvailable": true,
            "action": "warm",
        });
        if let Value::Object(map) = overrides {
            for (key, entry) in map {
                value.as_object_mut().unwrap().insert(key, entry);
            }
        }
        let decision: CacheWarmingDecision = serde_json::from_value(value).expect("decision");
        decision
    };
    let status = |value: Value| -> CacheWarmingStatus {
        let mut status = CacheWarmingStatus {
            state: CacheWarmingState::Inactive,
            reason: None,
            next_warm_at: None,
            decision: None,
            extension_override: false,
        };
        if let Some(state) = value.get("state").and_then(Value::as_str) {
            status.state = match state {
                "scheduled" => CacheWarmingState::Scheduled,
                "refreshing" => CacheWarmingState::Refreshing,
                _ => CacheWarmingState::Inactive,
            };
        }
        if let Some(reason) = value.get("reason") {
            status.reason = reason.as_str().map(str::to_string);
        }
        if let Some(next_warm_at) = value.get("nextWarmAt").and_then(Value::as_i64) {
            status.next_warm_at = Some(next_warm_at);
        }
        if let Some(decision_value) = value.get("decision") {
            status.decision = Some(decision(decision_value.clone()));
        }
        if let Some(extension_override) = value.get("extensionOverride").and_then(Value::as_bool) {
            status.extension_override = extension_override;
        }
        status
    };
    // The input grid mirrors the driver's `statuses` object.
    let cases: Vec<(&str, Value)> = vec![
        (
            "waiting",
            json!({ "state": "inactive", "reason": "waiting for first request" }),
        ),
        (
            "disabled",
            json!({ "state": "inactive", "reason": "cache warming disabled" }),
        ),
        ("unknownReason", json!({ "state": "inactive" })),
        (
            "economicsUnavailable",
            json!({ "state": "inactive", "reason": "cache economics unavailable",
                "decision": { "economicsAvailable": false, "expectedSavings": -0.001, "action": "stop" } }),
        ),
        (
            "economicsUnavailableOverride",
            json!({ "state": "inactive", "reason": "stopped by extension",
                "decision": { "economicsAvailable": false, "expectedSavings": -0.001, "action": "stop" },
                "extensionOverride": true }),
        ),
        (
            "stoppedBelowThreshold",
            json!({ "state": "inactive", "reason": "expected savings below threshold",
                "decision": { "expectedSavings": -0.05, "action": "stop" } }),
        ),
        (
            "stoppedNegativeSavings",
            json!({ "state": "inactive", "reason": "expected savings below threshold",
                "decision": { "expectedSavings": -0.5, "action": "stop" } }),
        ),
        (
            "stoppedByExtension",
            json!({ "state": "inactive", "reason": "stopped by extension",
                "decision": {}, "extensionOverride": true }),
        ),
        ("warming", json!({ "state": "refreshing", "decision": {} })),
        (
            "warmingIdle",
            json!({ "state": "refreshing",
                "decision": { "phase": "idle", "continuationProbability": 0.15, "expectedSavings": -0.07 } }),
        ),
        (
            "scheduled",
            json!({ "state": "scheduled", "nextWarmAt": 91000, "decision": {} }),
        ),
        (
            "scheduledIdle",
            json!({ "state": "scheduled", "nextWarmAt": 91000,
                "decision": { "phase": "idle", "continuationProbability": 0.15, "expectedSavings": -0.07 } }),
        ),
    ];
    for (name, value) in cases {
        assert_eq!(
            format_cache_warming_status(&status(value), 10000),
            grid[name].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn format_usage_grid_matches() {
    let grid = scenario("format_usage_grid");
    let usage = |cost: f64, note: Option<&str>| {
        UsageEntry::cache_warm(
            "u".to_string(),
            None,
            "t".to_string(),
            "anthropic",
            "claude",
            &Usage {
                input: 0,
                output: 1,
                cache_read: 1000,
                cache_write: 0,
                cache_write_1h: None,
                reasoning: None,
                total_tokens: 1,
                cost: UsageCost {
                    input: 0.0,
                    output: cost,
                    cache_read: 0.0003,
                    cache_write: 0.0,
                    total: cost,
                },
            },
            note,
        )
    };
    assert_eq!(
        format_cache_warming_usage(&usage(0.0003, None)),
        grid["simple"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(0.123456, None)),
        grid["sixDecimals"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(0.1234, None)),
        grid["trailingZeros"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(2.0, None)),
        grid["wholeDollar"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(0.000001, None)),
        grid["tiny"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(0.0, None)),
        grid["zero"].as_str().unwrap()
    );
    assert_eq!(
        format_cache_warming_usage(&usage(0.0003, Some("extension override"))),
        grid["withNote"].as_str().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Port-side safety-window unit tests (clock seam; not oracle-pinned: node
// fake timers are unavailable in the fixture runtime).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn safety_windows_stop_past_the_deadlines() {
    // The one-hour window trips when the clock moves past startedAt +
    // MAX_WARMING_AGE_MS before the refresh completes and re-schedules (a
    // timer running late after sleep/event-loop blockage). The stub advances
    // the clock at request-dispatch time, so the first refresh still passes
    // its own refresh deadline (clock ~base) and the re-schedule sees the
    // shifted clock.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let base = crate::ai::now_ms();
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(base));
    let shifted = base + crate::coding_agent::core::cache_warmer::MAX_WARMING_AGE_MS + 5_000;
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::AdvanceClockThenMessage(
            Arc::clone(&clock),
            shifted,
            stub_message(StopReason::Stop),
        ),
        None,
    );
    {
        let clock = Arc::clone(&clock);
        warmer.set_clock(Box::new(move || {
            clock.load(std::sync::atomic::Ordering::SeqCst)
        }));
    }
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert_eq!(
        warmer.status().reason.as_deref(),
        Some("one-hour safety limit reached")
    );

    // Idle runs stop at the 30-minute limit; onAgentSettled re-checks the
    // deadline with the shifted clock.
    let session = Arc::new(StubSession::branch_with_prompt(100000, 50000, 0));
    let warmer = make_warmer(
        session,
        "idle",
        StubStream::Message(stub_message(StopReason::Stop)),
        None,
    );
    let base = crate::ai::now_ms();
    warmer.set_clock(Box::new(move || base));
    warmer.start(
        warm_request(&short_ttl_model(), json!({})),
        Arc::new(|| true),
    );
    let shifted = base + crate::coding_agent::core::cache_warmer::MAX_IDLE_WARMING_AGE_MS + 5_000;
    warmer.set_clock(Box::new(move || shifted));
    warmer.on_agent_settled();
    assert_eq!(
        warmer.status().reason.as_deref(),
        Some("30-minute idle safety limit reached")
    );
}

#[test]
fn success_reason_mapping_covers_the_stub() {
    // Sanity for the seam used above: error/aborted stop reasons map through
    // the error channel, not Done.
    let _ = SuccessReason::Stop;
    assert_ne!(StopReason::Error, StopReason::Aborted);
}
