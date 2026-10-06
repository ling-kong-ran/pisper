#![allow(dead_code)]
//! The real-harness runtime fixture for the Task 9 oracle ports — the port
//! of `test/harness/pico3/helpers.ts` (409 lines): `open()` builds a full
//! [`Harness`] over memory/jsonl storage with the built-in kinds, the fake
//! provider ([`fake`]), the [`Gate`], and the `tool` fixture.
//!
//! Disclosed substitutions: `models.resolve` returns a [`ModelInfo`]; the
//! upstream tests mutate the resolved model object directly
//! (`modelObj.contextWindow = 200`), the fixture exposes
//! [`FakeModels::set_window`]. The tokio sleep in `sleep`/`until_phase`
//! polls instead of the upstream `setTimeout` loop.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::harness::{Harness, HarnessOptions, RootOptions};
use crate::agent_core::harness::pico3::runtime::{
    ModelInfo, Models, ToolApi, ToolDeclaration, ToolResult,
};
use crate::agent_core::harness::pico3::types::{Entry, Id, Input, ModelRef, Task};

pub use crate::agent_core::harness::pico3::kinds::collapse::CollapseHandlers;
pub use crate::agent_core::harness::pico3::kinds::generation::GenerationHandlers;
pub use crate::agent_core::harness::pico3::kinds::tool::ToolHandlers;

/// Upstream `ctx` (`helpers.ts:51`).
pub fn ctx() -> Context {
    Context::background()
}

/// Upstream `model` (`helpers.ts:52`).
pub fn model() -> ModelRef {
    ModelRef {
        provider: "anthropic".to_owned(),
        model_id: "fake-1".to_owned(),
    }
}

/// Upstream `kinds(entries)` (`helpers.ts:54-55`): kind tags with `*` for
/// heads.
pub fn kinds(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| {
            let tag = e.kind.trim_start_matches("pi.").to_owned();
            if e.head.is_some() {
                format!("{tag}*")
            } else {
                tag
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Upstream `contentOf(entry)` (`helpers.ts:56-59`).
pub fn content_of(entry: &Entry) -> String {
    let content = entry
        .model
        .as_ref()
        .and_then(|model| model.first())
        .and_then(|message| message.get("content").cloned());
    match content {
        Some(Value::String(text)) => text,
        Some(other) => serde_json::to_string(&other).expect("content"),
        None => "null".to_owned(),
    }
}

/// The outcome status helper (`t.outcome?.status`).
pub fn outcome_status(task: &Task) -> String {
    task.outcome
        .as_ref()
        .map(|outcome| outcome.status.clone())
        .unwrap_or_default()
}

/// Upstream `failureOf(t)` (`helpers.ts:38-39`).
pub fn failure_of(task: &Task) -> Option<Value> {
    task.outcome
        .as_ref()
        .filter(|outcome| outcome.status == "failed")
        .and_then(|outcome| outcome.failure.clone())
}

/// Upstream `resultOf(t)` (`helpers.ts:40`).
pub fn result_of(task: &Task) -> Option<Value> {
    task.outcome
        .as_ref()
        .and_then(|outcome| outcome.result.clone())
}

// ---------------------------------------------------------------------------
// Gate (`helpers.ts:64-93`)
// ---------------------------------------------------------------------------

/// Upstream `Gate` (`helpers.ts:64-93`).
#[derive(Clone)]
pub struct Gate {
    inner: Arc<GateInner>,
}

struct GateInner {
    state: Mutex<GateState>,
    arrived: AtomicUsize,
}

#[derive(Default)]
struct GateState {
    opened: bool,
    waiters: Vec<(usize, oneshot::Sender<()>)>,
}

// Cancellation/drop removes only this caller, never another gate waiter.
struct GateWaiter {
    inner: Arc<GateInner>,
    id: usize,
}

impl Drop for GateWaiter {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .expect("gate state")
            .waiters
            .retain(|(id, _)| *id != self.id);
    }
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate {
            inner: Arc::new(GateInner {
                state: Mutex::new(GateState::default()),
                arrived: AtomicUsize::new(0),
            }),
        }
    }

    /// Upstream `gate.open()`.
    pub fn open(&self) {
        let mut state = self.inner.state.lock().expect("gate state");
        state.opened = true;
        for (_, waiter) in state.waiters.drain(..) {
            let _ = waiter.send(());
        }
    }

    /// Upstream `gate.close()`.
    pub fn close(&self) {
        self.inner.state.lock().expect("gate state").opened = false;
    }

    /// Upstream `gate.wait(ctx)`.
    pub async fn wait(&self, ctx: Context) -> anyhow::Result<()> {
        let id = self.inner.arrived.fetch_add(1, Ordering::SeqCst);
        let receiver = {
            // Test open + registration under one mutex: an opener must see
            // either an already-open gate or this registered receiver.
            let mut state = self.inner.state.lock().expect("gate state");
            if state.opened {
                return Ok(());
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.push((id, sender));
            receiver
        };
        let _waiter = GateWaiter {
            inner: self.inner.clone(),
            id,
        };
        tokio::select! {
            result = receiver => {
                result.map_err(|_| anyhow::anyhow!("gate dropped"))
            }
            _ = async {
                if let Some(signal) = ctx.abort_signal() {
                    signal.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                Err(anyhow::anyhow!("aborted"))
            }
        }
    }

    /// Upstream `gate.arrivals(n)` — resolve once `n` callers arrived.
    pub async fn arrivals(&self, n: usize) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(4000);
        while self.inner.arrived.load(Ordering::SeqCst) < n {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "gate: only {}/{} arrivals",
                    self.inner.arrived.load(Ordering::SeqCst),
                    n
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Fake provider (`helpers.ts:99-209`)
// ---------------------------------------------------------------------------

/// Upstream `Response` (`helpers.ts:99-104`).
#[derive(Clone, Default)]
pub struct Response {
    pub text: Option<String>,
    pub tool_calls: Vec<(String, Value)>,
    pub error: Option<String>,
    pub stop: Option<String>,
}

/// Upstream `echoScript` (`helpers.ts:213-227`): tool calls on
/// "tool:<names>", errors on "error:<msg>", else an echo answer.
pub fn echo_script(messages: &[Value], _call: usize) -> Response {
    let last = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) != Some("system"))
        .cloned()
        .unwrap_or(Value::Null);
    let role = last.get("role").and_then(Value::as_str).unwrap_or("");
    if role == "toolResult" {
        return Response {
            text: Some("after tools ".to_owned()),
            ..Default::default()
        };
    }
    let user = last
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Some(names) = user.strip_prefix("tool:") {
        return Response {
            tool_calls: names
                .split(',')
                .map(|name| (name.to_owned(), json!({ "v": name })))
                .collect(),
            ..Default::default()
        };
    }
    if let Some(error) = user.strip_prefix("error:") {
        return Response {
            error: Some(error.to_owned()),
            ..Default::default()
        };
    }
    Response {
        text: Some(format!("answer to {user} ")),
        ..Default::default()
    }
}

/// Whether the request should be gated (upstream `gateWhen`).
pub type GateWhenFn = Arc<dyn Fn(&[Value]) -> bool + Send + Sync>;
/// Upstream `respond(messages, call)`.
pub type RespondFn = Arc<dyn Fn(&[Value], usize) -> Response + Send + Sync>;

/// Upstream `fake(options)` (`helpers.ts:122-209`): the fake Models.
pub struct FakeModels {
    calls: AtomicUsize,
    requests: Mutex<Vec<Vec<Value>>>,
    respond: RespondFn,
    gate: std::sync::RwLock<Option<Gate>>,
    gate_when: Option<GateWhenFn>,
    token_delay_ms: u64,
    window: std::sync::Mutex<(i64, i64)>,
}

impl FakeModels {
    pub fn new(respond: RespondFn) -> Arc<FakeModels> {
        Arc::new(FakeModels {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            respond,
            gate: std::sync::RwLock::new(None),
            gate_when: None,
            token_delay_ms: 0,
            window: std::sync::Mutex::new((200_000, 8192)),
        })
    }

    /// Upstream `fake({ respond: …, gate })`.
    pub fn with_gate(self: Arc<Self>, gate: Gate) -> Arc<Self> {
        *self.gate.write().expect("gate") = Some(gate);
        self
    }

    /// Upstream `fake({ gateWhen })`.
    pub fn with_gate_when(mut self, gate_when: GateWhenFn) -> Self {
        self.gate_when = Some(gate_when);
        self
    }

    /// Upstream `fake({ tokenDelayMs })`.
    pub fn with_token_delay(mut self, ms: u64) -> Self {
        self.token_delay_ms = ms;
        self
    }

    /// The test-side `modelObj.contextWindow = …` mutation.
    pub fn set_window(&self, context_window: i64, max_tokens: i64) {
        *self.window.lock().expect("window") = (context_window, max_tokens);
    }

    /// Upstream `models.calls`.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Upstream `models.requests`.
    pub fn requests(&self) -> Vec<Vec<Value>> {
        self.requests.lock().expect("requests").clone()
    }

    /// The resolved model metadata (upstream `models.resolve(...)`).
    pub fn info(&self) -> ModelInfo {
        let (context_window, max_tokens) = *self.window.lock().expect("window");
        ModelInfo {
            id: "fake-1".to_owned(),
            name: "Fake".to_owned(),
            api: "anthropic-messages".to_owned(),
            provider: "anthropic".to_owned(),
            context_window,
            max_tokens,
        }
    }
}

impl Models for FakeModels {
    fn resolve(&self, _model: &ModelRef) -> Option<ModelInfo> {
        Some(self.info())
    }

    fn stream(
        &self,
        model: ModelInfo,
        request: crate::agent_core::harness::pico3::runtime::GenerationRequest,
        ctx: Context,
    ) -> crate::agent_core::harness::pico3::runtime::EventStream {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("requests")
            .push(request.messages.clone());
        let response = (self.respond)(&request.messages, call);
        let gate = self.gate.read().expect("gate").clone();
        let gate_when = self.gate_when.clone();
        let token_delay = self.token_delay_ms;
        let info = model;
        // The planned token sequence (`helpers.ts`'s generator body), as an
        // unfold state machine: Wait(gate), Sleep, Emit(event).
        use crate::ai::types::events::{
            AssistantMessageEvent as Event, ErrorReason, SuccessReason,
        };
        use crate::ai::types::primitives::StopReason;
        let timestamp = crate::ai::now_ms();
        let mut minimal = AssistantMessageShape {
            content: Vec::new(),
            api: info.api.clone(),
            provider: info.provider.clone(),
            model: info.id.clone(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: serde_json::from_value(json!({
                "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            }))
            .expect("usage"),
            stop_reason: StopReason::Pending,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp,
        };
        enum Step {
            Emit(Box<Event>),
            Gate,
            Delay,
        }
        let emit = |event| Step::Emit(Box::new(event));
        let mut steps: std::collections::VecDeque<Step> = std::collections::VecDeque::new();
        steps.push_back(emit(Event::Start {
            message: minimal.clone(),
        }));
        let should_gate = gate.is_some()
            && gate_when
                .as_ref()
                .map(|when| when(&request.messages))
                .unwrap_or(true);
        if should_gate {
            steps.push_back(Step::Gate);
        }
        if let Some(error) = &response.error {
            minimal.stop_reason = StopReason::Error;
            minimal.error_message = Some(error.clone());
            steps.push_back(emit(Event::Error {
                reason: ErrorReason::Error,
                error: minimal,
            }));
        } else {
            let mut content: Vec<Value> = Vec::new();
            let mut tokens = 0usize;
            if let Some(text) = &response.text {
                steps.push_back(emit(Event::TextStart {
                    content_index: content.len(),
                }));
                content.push(json!({ "type": "text", "text": "" }));
                let index = content.len() - 1;
                for word in text.split(' ').filter(|w| !w.is_empty()) {
                    if token_delay > 0 {
                        steps.push_back(Step::Delay);
                    }
                    let delta = format!("{word} ");
                    let current = content[index]
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    content[index]["text"] = Value::String(format!("{current}{delta}"));
                    tokens += 1;
                    steps.push_back(emit(Event::TextDelta {
                        content_index: index,
                        delta: delta.clone(),
                    }));
                }
                let final_text = content[index]
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                steps.push_back(emit(Event::TextEnd {
                    content_index: index,
                    content: final_text,
                }));
            }
            for (index, (name, arguments)) in response.tool_calls.iter().enumerate() {
                let call_id = format!("call_{call}_{index}");
                steps.push_back(emit(Event::ToolcallStart {
                    content_index: content.len(),
                }));
                content.push(json!({ "type": "toolCall", "id": call_id, "name": name, "arguments": arguments }));
                let tool_call: crate::ai::types::ToolCall =
                    serde_json::from_value(content.last().cloned().expect("pushed"))
                        .expect("tool call");
                steps.push_back(emit(Event::ToolcallEnd {
                    content_index: content.len() - 1,
                    tool_call,
                }));
            }
            let reason = if !response.tool_calls.is_empty() {
                SuccessReason::ToolUse
            } else if response.stop.as_deref() == Some("length") {
                SuccessReason::Length
            } else {
                SuccessReason::Stop
            };
            minimal.content = serde_json::from_value(json!(content)).expect("blocks");
            minimal.usage = serde_json::from_value(json!({
                "input": request.messages.len() * 50,
                "output": tokens,
                "cacheRead": 0,
                "cacheWrite": 0,
                "totalTokens": request.messages.len() * 50 + tokens,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            }))
            .expect("usage");
            minimal.stop_reason = StopReason::from(reason);
            steps.push_back(emit(Event::Done {
                reason,
                message: minimal,
            }));
        }
        let state = (steps, ctx, gate);
        let stream = futures::stream::unfold(
            state,
            move |(mut steps, ctx, gate): (
                std::collections::VecDeque<Step>,
                Context,
                Option<Gate>,
            )| async move {
                loop {
                    match steps.pop_front() {
                        Some(Step::Gate) => {
                            if let Some(gate_handle) = gate.clone() {
                                if gate_handle.wait(ctx.clone()).await.is_err() {
                                    return Some((
                                        Err(anyhow::anyhow!("aborted")),
                                        (steps, ctx, gate),
                                    ));
                                }
                            }
                        }
                        Some(Step::Delay) => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                FakeDelay(token_delay).0,
                            ))
                            .await;
                            if let Some(signal) = ctx.abort_signal() {
                                if signal.is_cancelled() {
                                    return Some((
                                        Err(anyhow::anyhow!("aborted")),
                                        (steps, ctx, gate),
                                    ));
                                }
                            }
                        }
                        Some(Step::Emit(event)) => {
                            return Some((Ok(*event), (steps, ctx, gate)));
                        }
                        None => return None,
                    }
                }
            },
        );
        Box::pin(stream)
    }

    fn fetch_deferred(
        &self,
        _model: ModelInfo,
        _handle: Value,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Value>> {
        Box::pin(async { Err(anyhow::anyhow!("no deferred")) })
    }

    fn cancel_deferred(
        &self,
        _model: ModelInfo,
        _handle: Value,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Alias for the typed assistant message used by the fake.
use crate::ai::types::message::AssistantMessage as AssistantMessageShape;

/// Newtype so the token-delay duration stays in a `Send` unfold state.
struct FakeDelay(u64);

// ---------------------------------------------------------------------------
// Tool fixture (`helpers.ts:232-258`)
// ---------------------------------------------------------------------------

/// Upstream `tool(name, opts)` — a counting test tool.
#[derive(Clone)]
pub struct TestTool {
    pub declaration: Arc<ToolDeclaration>,
    calls: Arc<AtomicUsize>,
}

impl TestTool {
    /// The number of invocations (`t.calls`).
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The underlying declaration (for `open({ tools: [...] })`).
    pub fn declaration(&self) -> Arc<ToolDeclaration> {
        self.declaration.clone()
    }
}

/// Upstream `tool()` options (`helpers.ts:233-242`).
#[derive(Default)]
pub struct ToolOptions {
    pub replay: Option<String>,
    pub gate: Option<Gate>,
    pub is_error: Option<bool>,
    pub throws: Option<String>,
    pub control: Option<Value>,
    pub output: Option<crate::agent_core::harness::pico3::runtime::OutputBounds>,
}

/// Upstream `tool(name, opts)` (`helpers.ts:233-258`).
pub fn tool(name: &str, opts: ToolOptions) -> TestTool {
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = opts.gate.clone();
    let throws = opts.throws.clone();
    let is_error = opts.is_error.unwrap_or(false);
    let control = opts.control.clone();
    let tool_name = name.to_owned();
    let calls_for_execute = calls.clone();
    let declaration = Arc::new(ToolDeclaration {
        name: name.to_owned(),
        description: name.to_owned(),
        parameters: json!({
            "type": "object",
            "properties": { "v": { "type": "string" } },
            "required": ["v"],
        }),
        replay: opts.replay.clone(),
        output: opts.output,
        execute: Arc::new(
            move |args: Value,
                  _api: ToolApi,
                  ctx: Context|
                  -> BoxFuture<'static, anyhow::Result<ToolResult>> {
                let calls = calls_for_execute.clone();
                let gate = gate.clone();
                let throws = throws.clone();
                let control = control.clone();
                let tool_name = tool_name.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(gate) = &gate {
                        gate.wait(ctx.clone()).await?;
                    }
                    if let Some(message) = &throws {
                        anyhow::bail!("{message}");
                    }
                    let value = args
                        .get("v")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let content =
                        json!({ "type": "text", "text": format!("{tool_name}({value})") });
                    Ok(ToolResult {
                        content: Some(vec![content]),
                        is_error: if is_error { Some(true) } else { None },
                        details: None,
                        diagnostics: None,
                        control,
                    })
                })
            },
        ),
    });
    TestTool { declaration, calls }
}

// ---------------------------------------------------------------------------
// Harness factory with crash/reopen (`helpers.ts:262-377`)
// ---------------------------------------------------------------------------

/// Upstream `Env` (`helpers.ts:305-316`).
pub struct Env {
    /// Upstream `env.h`.
    pub h: Arc<Harness>,
    /// Upstream `env.root`.
    pub root: Conversation,
    /// Upstream `env.storage`.
    pub storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage>,
    /// Upstream `env.dir`.
    pub dir: Option<std::path::PathBuf>,
}

impl Env {
    /// Upstream `env.entries(c)` — ascending.
    pub async fn entries(&self, conversation_id: Id) -> anyhow::Result<Vec<Entry>> {
        let mut entries = self
            .h
            .entries(
                crate::agent_core::harness::pico3::types::EntryScan {
                    conversation_id,
                    limit: 1000,
                    ..Default::default()
                },
                ctx(),
            )
            .await?;
        entries.reverse();
        Ok(entries)
    }

    /// Upstream `env.tasks(c)`.
    pub async fn tasks(&self, conversation_id: Option<Id>) -> anyhow::Result<Vec<Task>> {
        self.h
            .session()
            .commit(
                crate::agent_core::harness::pico3::types::Invoker::Host {
                    conversation_id: Some(1),
                },
                ctx(),
                Default::default(),
                move |tx: &mut crate::agent_core::harness::pico3::session::Tx, _l: Context| {
                    Box::pin(async move {
                        tx.tasks(&crate::agent_core::harness::pico3::types::TaskScan {
                            conversation_id,
                            ..Default::default()
                        })
                        .await
                    })
                },
            )
            .await
            .map(|result| result.value)
    }

    /// Upstream `env.input(id)`.
    pub async fn input(&self, id: Id) -> anyhow::Result<Option<Input>> {
        self.h.input_handle(id).result(ctx()).await
    }

    /// Upstream `env.close()`.
    pub async fn close(&self, ctx: Context) -> anyhow::Result<()> {
        self.h.close(ctx).await
    }

    /// Live (non-terminal) tasks.
    pub async fn live_tasks(&self, conversation_id: Option<Id>) -> anyhow::Result<Vec<Task>> {
        Ok(self
            .tasks(conversation_id)
            .await?
            .into_iter()
            .filter(|task| {
                task.status != crate::agent_core::harness::pico3::types::TaskStatus::Terminal
            })
            .collect())
    }
}

/// Upstream `ConversationHandle` (aliased for fixture readability).
pub type Conversation = crate::agent_core::harness::pico3::harness::ConversationHandle;

/// `open()` options (`helpers.ts:318-329` parameter object).
#[derive(Default)]
pub struct OpenOptions {
    /// Register recovery hooks before dispatching persisted work.
    pub paused: bool,
    pub models: Option<Arc<FakeModels>>,
    pub tools: Vec<Arc<ToolDeclaration>>,
    pub task_kinds: Vec<Arc<dyn Kind>>,
    pub root_rewindable: Option<JsonObject>,
    pub root_sticky: Option<JsonObject>,
    pub dir: Option<std::path::PathBuf>,
    pub process_host: Option<Arc<dyn crate::agent_core::harness::pico3::runtime::ProcessHost>>,
    pub plugins: std::collections::HashMap<
        String,
        crate::agent_core::harness::pico3::runtime::PluginHandler,
    >,
}

use crate::agent_core::harness::pico3::runtime::Kind;
use crate::agent_core::harness::pico3::types::JsonObject;

/// Upstream `open(opts)` (`helpers.ts:318-377`).
pub async fn open(opts: OpenOptions) -> anyhow::Result<Env> {
    let storage: Arc<dyn crate::agent_core::harness::pico3::types::Storage> = match &opts.dir {
        Some(dir) => {
            crate::agent_core::harness::pico3::jsonl::JsonlStorage::open(dir, false).await?
        }
        None => Arc::new(crate::agent_core::harness::pico3::memory::MemoryStorage::new()),
    };
    let models = match &opts.models {
        Some(models) => models.clone(),
        None => FakeModels::new(Arc::new(echo_script)),
    };
    let mut options = HarnessOptions::new(models.clone());
    options.tools = opts.tools.clone();
    options.task_kinds = opts.task_kinds.clone();
    options.plugins = opts.plugins.clone();
    options.process_host = opts.process_host.clone();
    options.root = Some(RootOptions {
        // `root: { rewindable: { model, selectedTools: tools } }`
        // (`helpers.ts:342`).
        rewindable: opts.root_rewindable.clone().or_else(|| {
            Some(
                json!({
                    "model": { "provider": "anthropic", "modelId": "fake-1" },
                    "selectedTools": opts
                        .tools
                        .iter()
                        .map(|tool| Value::String(tool.name.clone()))
                        .collect::<Vec<_>>(),
                })
                .as_object()
                .cloned()
                .expect("object"),
            )
        }),
        sticky: opts.root_sticky.clone(),
    });
    let h = Harness::open(storage.clone(), options, ctx()).await?;
    if !opts.paused {
        h.resume();
    }
    let root = Conversation {
        harness: h.clone(),
        id: 1,
    };
    Ok(Env {
        h,
        root,
        storage,
        dir: opts.dir.clone(),
    })
}

/// The root handle's input handle (upstream `root.send(...)` result).
pub type RootInput = crate::agent_core::harness::pico3::harness::InputHandle;

/// `untilPhase` (`helpers.ts:388-399`).
pub async fn until_phase(env: &Env, kind: &str, phase: Option<&str>) -> anyhow::Result<Task> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(6000);
    loop {
        let found = env.tasks(None).await?.into_iter().find(|task| {
            task.kind == kind
                && task.status != crate::agent_core::harness::pico3::types::TaskStatus::Terminal
                && task
                    .checkpoint
                    .as_ref()
                    .and_then(|checkpoint| checkpoint.get("phase").and_then(Value::as_str))
                    == phase
        });
        if let Some(task) = found {
            return Ok(task);
        }
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!(
                "timeout waiting for {kind}@{phase:?}; tasks={:?}",
                env.tasks(None).await?
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

/// `untilTerminal` (`helpers.ts:401-409`).
pub async fn until_terminal(env: &Env, id: Id) -> anyhow::Result<Task> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(6000);
    loop {
        let found = env
            .tasks(None)
            .await?
            .into_iter()
            .find(|task| task.id == id);
        if let Some(task) = found {
            if task.status == crate::agent_core::harness::pico3::types::TaskStatus::Terminal {
                return Ok(task);
            }
        }
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("timeout waiting for task {id}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Register the standard hook handler struct on the generation kind
/// (`open({ hooks: { generation } })`).
pub fn install_generation_hooks(
    env: &Env,
    handlers: GenerationHandlers,
) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
    let namespace = env.h.namespace(
        "test.hooks",
        crate::agent_core::harness::pico3::types::NamespaceDefaultsValue::default(),
        None,
    )?;
    let kind = env.h.builtin_kind("pi.generation").expect("registered");
    env.h.hooks(&namespace, &kind, Arc::new(handlers))
}

/// Register tool hooks (`open({ hooks: { tool } })`).
pub fn install_tool_hooks(
    env: &Env,
    handlers: ToolHandlers,
) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
    let namespace = env.h.namespace(
        "test.hooks",
        crate::agent_core::harness::pico3::types::NamespaceDefaultsValue::default(),
        None,
    )?;
    let kind = env.h.builtin_kind("pi.tool").expect("registered");
    env.h.hooks(&namespace, &kind, Arc::new(handlers))
}

/// Register collapse hooks (`open({ hooks: { collapse } })`).
pub fn install_collapse_hooks(
    env: &Env,
    handlers: CollapseHandlers,
) -> anyhow::Result<Box<dyn Fn() + Send + Sync>> {
    let namespace = env.h.namespace(
        "test.hooks",
        crate::agent_core::harness::pico3::types::NamespaceDefaultsValue::default(),
        None,
    )?;
    let kind = env.h.builtin_kind("pi.collapse").expect("registered");
    env.h.hooks(&namespace, &kind, Arc::new(handlers))
}
