//! Port of upstream `coding-agent/src/core/cache-warmer.ts` (HEAD 2bbfcca43,
//! v0.99.1): keeps one prompt cache entry alive by re-sending its request
//! with a one-token output cap before the entry expires.
//!
//! The pure decision surface (`getCacheWarmingDelayMs`, `getPromptCacheTtlMs`,
//! `isReplayable`, the warm-or-stop economics, the `/session` formatters) is
//! byte-pinned against the verbatim upstream sources under
//! `tests/fixtures/core_delta_oracle/cache-warmer/`.
//!
//! # Seams (disclosed)
//!
//! - Upstream takes `Pick<ModelRuntime, "streamSimple">` and
//!   `Pick<SessionManager, "appendUsage" | "getBranch">`. The port keeps the
//!   module decoupled from both via the [`CacheWarmStreamSource`] and
//!   [`CacheWarmSessionStore`] traits; `ModelRuntime` and `SessionManager`
//!   implement them in the later wiring slice. [`UsageEntry`] mirrors the
//!   upstream session-manager `UsageEntry` shape because the ported
//!   `SessionEntry` union does not carry the `"usage"` entry kind yet.
//! - Upstream `CacheWarmingMode` lives on `settings-manager.ts` (the
//!   `cacheWarming` setting, global only); the ported settings manager does
//!   not carry it yet, so the enum is defined here at upstream fidelity and
//!   the settings slice can re-export it.
//! - `setTimeout`/`AbortController` become tokio sleep tasks and
//!   [`CancellationToken`]s; run identity uses a generation counter (the
//!   upstream `this.run === run` checks). `run.timer.unref?.()` has no tokio
//!   equivalent (a spawned task keeps no refd handle).
//! - `Date.now()` rides a private clock seam defaulting to
//!   [`crate::ai::now_ms`], so the safety-window logic is testable on fixed
//!   instants.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::cost::calculate_cost;
use crate::ai::models::ModelsSimpleStreamOptions;
use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{CacheRetention, StopReason, Usage};
use crate::ai::types::Model;
use crate::ai::{now_ms, Context, PromptCacheTier};
use crate::coding_agent::session_manager::SessionEntry;

/// Streaming warming never continues past this long after the real request
/// that started it.
pub const MAX_WARMING_AGE_MS: i64 = 60 * 60_000;
/// Idle warming uses a shorter horizon because continuation estimates become
/// less reliable with age.
pub const MAX_IDLE_WARMING_AGE_MS: i64 = 30 * 60_000;
/// A refresh is sent only when it is expected to save at least this many
/// dollars.
pub const CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS: f64 = 0.05;
/// Chance that a real request arrives before the cache entry expires while
/// the agent sits idle. Measured from our own usage; per-session estimates
/// were not better than this constant.
pub const IDLE_CONTINUATION_PROBABILITY: f64 = 0.15;

/// Upstream `CacheWarmingMode` (settings-manager.ts): the cache-warming
/// profile. `"idle"` also warms between agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingMode {
    Off,
    Streaming,
    Idle,
}

impl CacheWarmingMode {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheWarmingMode::Off => "off",
            CacheWarmingMode::Streaming => "streaming",
            CacheWarmingMode::Idle => "idle",
        }
    }
}

/// Refresh at 90% of the TTL while preserving at least ten seconds of margin.
pub fn get_cache_warming_delay_ms(ttl_ms: f64) -> Option<i64> {
    if ttl_ms <= 10_000.0 {
        return None;
    }
    Some(((ttl_ms * 0.9).min(ttl_ms - 10_000.0)).floor().max(1.0) as i64)
}

/// Lifetime of the prompt cache entry a request writes, from the model's
/// `promptCache` tier for the retention the request used. `None` when the
/// model has no lifetime for that tier or caching is off.
pub fn get_prompt_cache_ttl_ms(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
) -> Option<f64> {
    let retention = options
        .and_then(|options| options.stream.cache_retention)
        .unwrap_or_else(|| {
            if get_provider_env_value(
                "PI_CACHE_RETENTION",
                options.and_then(|options| options.stream.env.as_ref()),
            )
            .as_deref()
                == Some("long")
            {
                CacheRetention::Long
            } else {
                CacheRetention::Short
            }
        });
    if retention == CacheRetention::None {
        return None;
    }
    let tier = match retention {
        CacheRetention::Long => PromptCacheTier::Long,
        _ => PromptCacheTier::Short,
    };
    model
        .prompt_cache
        .as_ref()
        .and_then(|prompt_cache| prompt_cache.0.get(&tier))
        .map(|seconds| seconds * 1000.0)
}

/// Whether replaying the request with a one-token output cap leaves its cache
/// entry untouched. Anthropic's budget-based thinking (Claude models without
/// adaptive thinking) derives `budget_tokens` from `max_tokens`; the replay
/// would get a different budget, which Anthropic keys the message cache on,
/// and the model could still think for thousands of tokens.
pub fn is_replayable(model: &Model, options: Option<&SimpleStreamOptions>) -> bool {
    if !options
        .map(|options| options.reasoning.is_some())
        .unwrap_or(false)
        || model.api != "anthropic-messages"
    {
        return true;
    }
    model
        .anthropic_compat()
        .map(|compat| compat.force_adaptive_thinking == Some(true))
        .unwrap_or(false)
}

/// Prompt size of the most recent real request on the branch, as reported by
/// the provider.
fn last_prompt_tokens(entries: &[SessionEntry]) -> u64 {
    for entry in entries.iter().rev() {
        if let SessionEntry::Message(message_entry) = entry {
            if let crate::agent_core::types::AgentMessage::Assistant(assistant) =
                &message_entry.message
            {
                let usage = &assistant.usage;
                return usage.input + usage.cache_read + usage.cache_write;
            }
        }
    }
    0
}

/// Upstream `price`: the total cost of a usage with only the given token
/// counts filled in.
fn price(model: &Model, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let mut usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: crate::ai::types::primitives::UsageCost::default(),
    };
    calculate_cost(model, &mut usage);
    usage.cost.total
}

/// Upstream `CacheWarmingAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingAction {
    Warm,
    Stop,
}

impl CacheWarmingAction {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheWarmingAction::Warm => "warm",
            CacheWarmingAction::Stop => "stop",
        }
    }
}

/// Upstream `CacheWarmingDecision["phase"]` and `ActiveRun["phase"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingPhase {
    Streaming,
    Idle,
}

/// Inputs and outcome of one warm-or-stop decision, as shown by `/session`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecision {
    /// "streaming" while the agent run that sent the request is still active.
    pub phase: CacheWarmingPhase,
    /// Price of this refresh: a cache read of the prompt plus one output
    /// token.
    pub warm_cost: f64,
    /// Extra price of the next real request if the cache entry is lost.
    pub miss_cost: f64,
    /// Estimated chance that a real request arrives before the entry expires.
    pub continuation_probability: f64,
    /// `continuationProbability * missCost - warmCost`.
    pub expected_savings: f64,
    /// False when the prompt size or the model's prices are unknown.
    pub economics_available: bool,
    /// Pi's decision: "warm" when `expectedSavings` is at least $0.05.
    pub action: CacheWarmingAction,
}

/// Fired before each refresh with pi's decision filled in. Everything else an
/// extension might want (model, idle state, context size) is on the context.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEvent {
    /// Constant `"cache_warming_decision"`.
    pub event_type: String,
    pub warm_cost: f64,
    pub miss_cost: f64,
    pub continuation_probability: f64,
    pub action: CacheWarmingAction,
}

impl CacheWarmingDecisionEvent {
    /// Upstream object literal with `type: "cache_warming_decision"`.
    pub fn new(
        warm_cost: f64,
        miss_cost: f64,
        continuation_probability: f64,
        action: CacheWarmingAction,
    ) -> Self {
        Self {
            event_type: "cache_warming_decision".to_string(),
            warm_cost,
            miss_cost,
            continuation_probability,
            action,
        }
    }
}

/// Upstream `CacheWarmingDecisionEventResult`: an extension may override
/// whether this refresh is sent. `"stop"` ends warming until the next real
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheWarmingDecisionEventResult {
    pub action: Option<CacheWarmingAction>,
}

/// Upstream `CacheWarmingStatus`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingStatus {
    /// "scheduled": a refresh timer is armed; "refreshing": a warm request is
    /// in flight.
    pub state: CacheWarmingState,
    /// Why nothing is scheduled.
    pub reason: Option<String>,
    pub next_warm_at: Option<i64>,
    /// The pending decision, or the decision that stopped warming.
    pub decision: Option<CacheWarmingDecision>,
    /// True when an extension changed `decision.action`.
    pub extension_override: bool,
}

/// Upstream `CacheWarmingStatus["state"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingState {
    Inactive,
    Scheduled,
    Refreshing,
}

/// The request whose prompt cache entry should be kept warm, exactly as it
/// was sent.
#[derive(Clone)]
pub struct CacheWarmRequest {
    pub model: Model,
    pub context: Context,
    pub options: ModelsSimpleStreamOptions,
}

/// Upstream `UsageEntry` (session-manager.ts): the persisted usage record
/// `appendUsage` writes. The port's session-manager slice does not carry the
/// `"usage"` entry kind yet, so the shape lives here for the
/// [`CacheWarmSessionStore`] seam and the `/session` formatter.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    /// Constant `"usage"`.
    #[serde(rename = "type")]
    pub entry_type: String,
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    /// Arbitrary usage category, such as "cache_warm".
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    /// Optional human-readable qualifier for usage notices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl UsageEntry {
    /// Upstream `appendUsage`'s entry literal; the caller supplies id and
    /// timestamp like the session manager does internally.
    pub fn cache_warm(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        provider: &str,
        model: &str,
        usage: &Usage,
        note: Option<&str>,
    ) -> Self {
        Self {
            entry_type: "usage".to_string(),
            id,
            parent_id,
            timestamp,
            kind: "cache_warm".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            usage: *usage,
            note: note.map(str::to_string),
        }
    }
}

/// Upstream `Pick<SessionManager, "appendUsage" | "getBranch">`.
pub trait CacheWarmSessionStore: Send + Sync {
    /// Append a usage entry and return it (upstream `appendUsage`).
    fn append_usage(
        &self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: &Usage,
        note: Option<&str>,
    ) -> UsageEntry;

    /// The current branch's entries (upstream `getBranch()`).
    fn get_branch(&self) -> Vec<SessionEntry>;
}

/// Upstream `Pick<ModelRuntime, "streamSimple">`.
pub trait CacheWarmStreamSource: Send + Sync {
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: ModelsSimpleStreamOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent>;
}

/// Upstream constructor's `decide` seam: lets extensions override
/// `event.action`; failures fall back to pi's decision.
pub type DecideFn = Arc<
    dyn Fn(CacheWarmingDecisionEvent) -> BoxFuture<'static, Result<CacheWarmingAction, String>>
        + Send
        + Sync,
>;

/// Upstream `ActiveRun extends CacheWarmRequest` plus bookkeeping.
struct ActiveRun {
    request: CacheWarmRequest,
    /// False once the session's model or messages no longer match the
    /// request.
    is_current: Arc<dyn Fn() -> bool + Send + Sync>,
    ttl_ms: f64,
    delay_ms: i64,
    /// Latest safe time to send this refresh, leaving half the original
    /// expiry margin.
    refresh_deadline_at: i64,
    started_at: i64,
    controller: CancellationToken,
    phase: CacheWarmingPhase,
    next_warm_at: i64,
    /// Set while a refresh that an extension forced is in flight.
    extension_override: bool,
    /// True while a refresh timer is armed (upstream `timer !== undefined`).
    timer_armed: bool,
    /// Identity for the upstream `this.run === run` checks.
    generation: u64,
}

impl ActiveRun {
    fn deadline(&self) -> i64 {
        self.started_at
            + if self.phase == CacheWarmingPhase::Idle {
                MAX_IDLE_WARMING_AGE_MS
            } else {
                MAX_WARMING_AGE_MS
            }
    }
}

struct WarmerState {
    run: Option<ActiveRun>,
    inactive: CacheWarmingStatus,
    generation: u64,
}

/// The `onWarmed` callback type (upstream `onWarmed?: (entry) => void`).
pub type OnWarmedFn = Box<dyn Fn(&UsageEntry) + Send + Sync>;

/// Shared state behind the cloneable [`CacheWarmer`] handle (the
/// `ModelRuntime`-style `Arc` inner).
struct CacheWarmerInner {
    state: Mutex<WarmerState>,
    models: Arc<dyn CacheWarmStreamSource>,
    session_manager: Arc<dyn CacheWarmSessionStore>,
    get_mode: Box<dyn Fn() -> CacheWarmingMode + Send + Sync>,
    /// Lets extensions override `event.action`; failures fall back to pi's
    /// decision.
    decide: DecideFn,
    /// Called with the persisted usage entry after each successful refresh.
    on_warmed: Mutex<Option<OnWarmedFn>>,
    /// Clock seam (`Date.now()`), injectable for tests.
    clock: Mutex<Box<dyn Fn() -> i64 + Send + Sync>>,
}

/// Keeps one prompt cache entry alive by re-sending its request with a
/// one-token output cap before the entry expires. `start` replaces any
/// previous run; warm requests never extend the fixed safety windows.
#[derive(Clone)]
pub struct CacheWarmer(Arc<CacheWarmerInner>);

impl CacheWarmer {
    /// Upstream constructor; `decide` defaults to echoing `event.action`.
    pub fn new(
        models: Arc<dyn CacheWarmStreamSource>,
        session_manager: Arc<dyn CacheWarmSessionStore>,
        get_mode: Box<dyn Fn() -> CacheWarmingMode + Send + Sync>,
    ) -> Self {
        Self::with_decide(
            models,
            session_manager,
            get_mode,
            Arc::new(|event| Box::pin(async move { Ok(event.action) })),
        )
    }

    /// Upstream constructor with an explicit `decide` seam.
    pub fn with_decide(
        models: Arc<dyn CacheWarmStreamSource>,
        session_manager: Arc<dyn CacheWarmSessionStore>,
        get_mode: Box<dyn Fn() -> CacheWarmingMode + Send + Sync>,
        decide: DecideFn,
    ) -> Self {
        Self(Arc::new(CacheWarmerInner {
            state: Mutex::new(WarmerState {
                run: None,
                inactive: CacheWarmingStatus {
                    state: CacheWarmingState::Inactive,
                    reason: Some("waiting for first request".to_string()),
                    next_warm_at: None,
                    decision: None,
                    extension_override: false,
                },
                generation: 0,
            }),
            models,
            session_manager,
            get_mode,
            decide,
            on_warmed: Mutex::new(None),
            clock: Mutex::new(Box::new(now_ms)),
        }))
    }

    /// Test-only clock injection (upstream tests control `Date.now()` via
    /// fake timers).
    #[cfg(test)]
    pub(crate) fn set_clock(&self, clock: Box<dyn Fn() -> i64 + Send + Sync>) {
        *lock(&self.0.clock) = clock;
    }

    fn now(&self) -> i64 {
        (lock(&self.0.clock))()
    }

    fn inner(&self) -> &CacheWarmerInner {
        &self.0
    }

    /// Upstream `warmer.onWarmed = callback`: called with the persisted usage
    /// entry after each successful refresh.
    pub fn set_on_warmed(&self, callback: OnWarmedFn) {
        *lock(&self.inner().on_warmed) = Some(callback);
    }

    /// Upstream `get status`.
    pub fn status(&self) -> CacheWarmingStatus {
        if (self.inner().get_mode)() == CacheWarmingMode::Off {
            return CacheWarmingStatus {
                state: CacheWarmingState::Inactive,
                reason: Some("cache warming disabled".to_string()),
                next_warm_at: None,
                decision: None,
                extension_override: false,
            };
        }
        let state = lock(&self.inner().state);
        let Some(run) = &state.run else {
            return state.inactive.clone();
        };
        if !(run.is_current)() {
            return CacheWarmingStatus {
                state: CacheWarmingState::Inactive,
                reason: Some("conversation context changed".to_string()),
                next_warm_at: None,
                decision: None,
                extension_override: false,
            };
        }
        let decision = self.evaluate(run);
        let refreshing = !run.timer_armed;
        if !decision.economics_available && !refreshing {
            return CacheWarmingStatus {
                state: CacheWarmingState::Inactive,
                reason: Some("cache economics unavailable".to_string()),
                next_warm_at: None,
                decision: None,
                extension_override: false,
            };
        }
        CacheWarmingStatus {
            state: if refreshing {
                CacheWarmingState::Refreshing
            } else {
                CacheWarmingState::Scheduled
            },
            reason: None,
            next_warm_at: Some(run.next_warm_at),
            decision: Some(decision),
            extension_override: run.extension_override,
        }
    }

    /// Keep the prompt cache entry written by `request` warm while
    /// `is_current` holds.
    pub fn start(
        &self,
        request: CacheWarmRequest,
        is_current: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        self.clear_run();
        let mode = (self.inner().get_mode)();
        if mode == CacheWarmingMode::Off {
            self.stop("cache warming disabled", None);
            return;
        }
        if !is_replayable(&request.model, Some(&request.options.simple)) {
            self.stop("request cannot be replayed safely", None);
            return;
        }
        let Some(ttl_ms) = get_prompt_cache_ttl_ms(&request.model, Some(&request.options.simple))
        else {
            let reason =
                if request.options.simple.stream.cache_retention == Some(CacheRetention::None) {
                    "request disabled prompt caching"
                } else {
                    "cache lifetime unavailable"
                };
            self.stop(reason, None);
            return;
        };
        let Some(delay_ms) = get_cache_warming_delay_ms(ttl_ms) else {
            self.stop("cache lifetime unavailable", None);
            return;
        };
        let generation = {
            let mut state = lock(&self.inner().state);
            state.generation += 1;
            state.generation
        };
        let run = ActiveRun {
            request,
            is_current,
            ttl_ms,
            delay_ms,
            refresh_deadline_at: 0,
            started_at: self.now(),
            controller: CancellationToken::new(),
            phase: CacheWarmingPhase::Streaming,
            next_warm_at: 0,
            extension_override: false,
            timer_armed: false,
            generation,
        };
        self.schedule(run);
    }

    pub fn on_agent_settled(&self) {
        let stop_reason = {
            let mut state = lock(&self.inner().state);
            let Some(run) = state.run.as_mut() else {
                return;
            };
            if (self.inner().get_mode)() == CacheWarmingMode::Streaming {
                Some("agent run settled")
            } else {
                run.phase = CacheWarmingPhase::Idle;
                let deadline = run.started_at + MAX_IDLE_WARMING_AGE_MS;
                if run.next_warm_at > deadline || self.now() >= deadline {
                    Some("30-minute idle safety limit reached")
                } else {
                    None
                }
            }
        };
        if let Some(reason) = stop_reason {
            self.stop(reason, None);
        }
    }

    /// Reconcile an active run after the persisted warming mode changes.
    pub fn on_mode_changed(&self) {
        let reason = {
            let state = lock(&self.inner().state);
            let Some(run) = state.run.as_ref() else {
                return;
            };
            self.get_mode_stop_reason(run)
        };
        if let Some(reason) = reason {
            self.stop(&reason, None);
        }
    }

    pub fn cancel(&self) {
        self.stop("inactive", None);
    }

    fn clear_run(&self) {
        let mut state = lock(&self.inner().state);
        if let Some(run) = state.run.take() {
            run.controller.cancel();
        }
    }

    /// Upstream `stop`: drop the run and remember why.
    fn stop(&self, reason: &str, stopped: Option<(&CacheWarmingDecision, bool)>) {
        self.clear_run();
        let mut state = lock(&self.inner().state);
        state.inactive = CacheWarmingStatus {
            state: CacheWarmingState::Inactive,
            reason: Some(reason.to_string()),
            next_warm_at: None,
            decision: stopped.map(|(decision, _)| decision.clone()),
            extension_override: stopped
                .map(|(_, override_flag)| override_flag)
                .unwrap_or(false),
        };
    }

    fn schedule(&self, mut run: ActiveRun) {
        run.extension_override = false;
        run.next_warm_at = self.now() + run.delay_ms;
        // A timer can run late after sleep or event-loop blockage. Keep half
        // of the planned pre-expiry margin for that delay and request
        // dispatch; a late refresh is likely a full-price cache write, not a
        // cache warm.
        run.refresh_deadline_at =
            run.next_warm_at + ((run.ttl_ms - run.delay_ms as f64) / 2.0).floor() as i64;
        let deadline = run.deadline();
        if run.next_warm_at > deadline || self.now() >= deadline {
            self.stop(
                if run.phase == CacheWarmingPhase::Idle {
                    "30-minute idle safety limit reached"
                } else {
                    "one-hour safety limit reached"
                },
                None,
            );
            return;
        }
        run.timer_armed = true;
        let generation = run.generation;
        let delay = (run.next_warm_at - self.now()).max(0) as u64;
        lock(&self.inner().state).run = Some(run);
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            this.fire(generation).await;
        });
    }

    /// The timer body: clear the timer marker, then run the refresh.
    async fn fire(&self, generation: u64) {
        {
            let mut state = lock(&self.inner().state);
            match state.run.as_mut() {
                Some(run) if run.generation == generation && run.timer_armed => {
                    run.timer_armed = false;
                }
                _ => return,
            }
        }
        self.refresh(generation).await;
    }

    fn refresh_deadline_missed(&self, generation: u64) -> bool {
        let deadline = {
            let state = lock(&self.inner().state);
            match state.run.as_ref() {
                Some(run) if run.generation == generation => run.refresh_deadline_at,
                _ => return true,
            }
        };
        if self.now() <= deadline {
            return false;
        }
        self.stop("cache refresh deadline missed", None);
        true
    }

    /// Upstream `validateRun`: the run must still be the active one, in
    /// mode, and current.
    fn validate_run(&self, generation: u64) -> bool {
        let owned = {
            let state = lock(&self.inner().state);
            matches!(state.run.as_ref(), Some(run) if run.generation == generation)
        };
        if !owned {
            return false;
        }
        let reason = {
            let state = lock(&self.inner().state);
            let run = state.run.as_ref().expect("checked above");
            self.get_mode_stop_reason(run).or_else(|| {
                if !(run.is_current)() {
                    Some("conversation context changed".to_string())
                } else {
                    None
                }
            })
        };
        match reason {
            Some(reason) => {
                self.stop(&reason, None);
                false
            }
            None => true,
        }
    }

    fn get_mode_stop_reason(&self, run: &ActiveRun) -> Option<String> {
        let mode = (self.inner().get_mode)();
        if mode == CacheWarmingMode::Off {
            return Some("cache warming disabled".to_string());
        }
        if mode == CacheWarmingMode::Streaming && run.phase == CacheWarmingPhase::Idle {
            return Some("agent run settled".to_string());
        }
        None
    }

    /// Upstream `evaluate`: the warm-or-stop economics of the active run.
    fn evaluate(&self, run: &ActiveRun) -> CacheWarmingDecision {
        let model = &run.request.model;
        let prompt_tokens = last_prompt_tokens(&self.inner().session_manager.get_branch());
        let cache_hit_cost = price(model, 0, 0, prompt_tokens, 0);
        let cache_miss_cost = if model.cost.cache_write > 0.0 {
            price(model, 0, 0, 0, prompt_tokens)
        } else {
            price(model, prompt_tokens, 0, 0, 0)
        };
        let warm_cost = price(model, 0, 1, prompt_tokens, 0);
        let miss_cost = (cache_miss_cost - cache_hit_cost).max(0.0);
        let continuation_probability = if run.phase == CacheWarmingPhase::Idle {
            IDLE_CONTINUATION_PROBABILITY
        } else {
            1.0
        };
        let economics_available =
            prompt_tokens > 0 && (cache_hit_cost > 0.0 || cache_miss_cost > 0.0);
        let expected_savings = continuation_probability * miss_cost - warm_cost;
        CacheWarmingDecision {
            phase: run.phase,
            warm_cost,
            miss_cost,
            continuation_probability,
            expected_savings,
            economics_available,
            action: if expected_savings >= CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS {
                CacheWarmingAction::Warm
            } else {
                CacheWarmingAction::Stop
            },
        }
    }

    /// Upstream `refresh` (the timer body). `generation` pins run identity
    /// against concurrent `start`/`stop` calls.
    async fn refresh(&self, generation: u64) {
        if !self.validate_run(generation) {
            return;
        }
        if self.refresh_deadline_missed(generation) {
            return;
        }
        let decision = {
            let state = lock(&self.inner().state);
            match state.run.as_ref() {
                Some(run) if run.generation == generation => self.evaluate(run),
                _ => return,
            }
        };
        let CacheWarmingDecision {
            warm_cost,
            miss_cost,
            continuation_probability,
            action,
            ..
        } = &decision;
        let mut action = *action;
        // Extension failures fall back to pi's own decision (upstream
        // try/catch around `await this.decide(...)`).
        let event = CacheWarmingDecisionEvent::new(
            *warm_cost,
            *miss_cost,
            *continuation_probability,
            action,
        );
        if let Ok(resolved) = (self.inner().decide)(event).await {
            action = resolved;
        }
        if !self.validate_run(generation) || self.refresh_deadline_missed(generation) {
            return;
        }
        let extension_override = action != decision.action;
        if action == CacheWarmingAction::Stop {
            let reason = if extension_override {
                "stopped by extension"
            } else if decision.economics_available {
                "expected savings below threshold"
            } else {
                "cache economics unavailable"
            };
            self.stop(reason, Some((&decision, extension_override)));
            return;
        }
        {
            let mut state = lock(&self.inner().state);
            if let Some(run) = state.run.as_mut() {
                if run.generation == generation {
                    run.extension_override = extension_override;
                }
            }
        }
        self.perform_refresh(generation, extension_override).await;
        let still_current = {
            let state = lock(&self.inner().state);
            matches!(state.run.as_ref(), Some(run) if run.generation == generation)
        };
        if still_current {
            let run = {
                let mut state = lock(&self.inner().state);
                match state.run.take() {
                    Some(run) if run.generation == generation => run,
                    other => {
                        // Lost the run concurrently; put it back untouched.
                        state.run = other;
                        return;
                    }
                }
            };
            self.schedule(run);
        }
    }

    /// The awaited stream half of upstream `refresh`.
    async fn perform_refresh(&self, generation: u64, extension_override: bool) {
        let (model, context, options, controller) = {
            let state = lock(&self.inner().state);
            match state.run.as_ref() {
                Some(run) if run.generation == generation => (
                    run.request.model.clone(),
                    run.request.context.clone(),
                    run.request.options.clone(),
                    run.controller.clone(),
                ),
                _ => return,
            }
        };
        let mut options = options;
        // Upstream `{ ...run.options, maxTokens: 1, maxRetries: 0, signal }`.
        options.simple.stream.max_tokens = Some(1);
        options.simple.stream.max_retries = Some(0);
        options.simple.stream.signal = Some(controller);
        let mut events = self.inner().models.stream_simple(&model, &context, options);
        let message = reduce_stream(&mut events, &model).await;
        if !self.validate_run(generation) {
            return;
        }
        if message.stop_reason != StopReason::Error && message.stop_reason != StopReason::Aborted {
            let note = extension_override.then_some("extension override");
            let entry = self.inner().session_manager.append_usage(
                "cache_warm",
                &message.provider,
                message.response_model.as_deref().unwrap_or(&message.model),
                &message.usage,
                note,
            );
            if let Ok(on_warmed) = self.inner().on_warmed.lock() {
                if let Some(on_warmed) = on_warmed.as_ref() {
                    on_warmed(&entry);
                }
            }
        }
    }
}

/// Upstream `.result()` on an event stream (the `reduceStream` convention
/// used by `model_runtime`).
async fn reduce_stream(
    events: &mut mpsc::Receiver<AssistantMessageEvent>,
    model: &Model,
) -> AssistantMessage {
    let mut partial = PartialAssistant::new();
    while let Some(event) = events.recv().await {
        if let Err(error) = partial.apply(&event) {
            return setup_error_message(
                model,
                format!("reducer rejected {}: {error}", event.event_type()),
            );
        }
    }
    partial
        .message()
        .cloned()
        .unwrap_or_else(|| setup_error_message(model, "stream ended without events"))
}

fn setup_error_message(model: &Model, message: impl std::fmt::Display) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(message.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn format_dollars(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.3}", value.abs())
    } else {
        format!("${value:.3}")
    }
}

fn format_cache_warming_economics(decision: &CacheWarmingDecision) -> String {
    if !decision.economics_available {
        return "cache economics unavailable".to_string();
    }
    let probability = (decision.continuation_probability * 100.0).round() as i64;
    let probability_text = if decision.phase == CacheWarmingPhase::Streaming {
        format!("{probability}% continuation probability while agent is running")
    } else {
        format!("{probability}% continuation probability")
    };
    let comparison = if decision.action == CacheWarmingAction::Warm {
        ">="
    } else {
        "<"
    };
    format!(
        "{probability_text}, expected savings {} {} ${:.3}",
        format_dollars(decision.expected_savings),
        comparison,
        CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS
    )
}

fn format_cache_warming_decision_time(next_warm_at: Option<i64>, now: i64) -> String {
    let Some(next_warm_at) = next_warm_at else {
        return "Decision now".to_string();
    };
    if next_warm_at <= now {
        return "Decision now".to_string();
    }
    // Math.ceil(diff / 1000); diff is positive here.
    let mut remaining_seconds = (next_warm_at - now + 999) / 1000;
    let hours = remaining_seconds / 3600;
    remaining_seconds %= 3600;
    let minutes = remaining_seconds / 60;
    let seconds = remaining_seconds % 60;
    let mut parts: Vec<String> = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    format!("Decision in {}", parts.join(" "))
}

/// One-line status for `/session`.
pub fn format_cache_warming_status(status: &CacheWarmingStatus, now: i64) -> String {
    let Some(decision) = &status.decision else {
        return inactive_line(status);
    };
    // A decision is attached once pi (or an extension) acted on it;
    // "inactive" without one never got that far.
    if status.state == CacheWarmingState::Inactive
        && !decision.economics_available
        && !status.extension_override
    {
        return inactive_line(status);
    }
    let details = if status.extension_override {
        format!(
            "extension override, {}",
            format_cache_warming_economics(decision)
        )
    } else {
        format!(
            "{} -> {}",
            format_cache_warming_economics(decision),
            decision.action.as_str()
        )
    };
    if status.state == CacheWarmingState::Inactive {
        return format!("Stopped ({details})");
    }
    if status.state == CacheWarmingState::Refreshing {
        return format!("Warming cache ({details})");
    }
    format!(
        "{} ({details})",
        format_cache_warming_decision_time(status.next_warm_at, now)
    )
}

fn inactive_line(status: &CacheWarmingStatus) -> String {
    format!(
        "Inactive ({})",
        status.reason.as_deref().unwrap_or("unknown reason")
    )
}

/// One-line transcript text for persisted cache-warming usage.
pub fn format_cache_warming_usage(entry: &UsageEntry) -> String {
    let note = match &entry.note {
        Some(note) => format!(" ({note})"),
        None => String::new(),
    };
    format!(
        "Cache warmed{note}: ${}",
        format_js_fixed6(entry.usage.cost.total)
    )
}

/// `value.toFixed(6).replace(/(\.\d{3}\d*?)0+$/, "$1")`: six decimals with
/// trailing zeros stripped past the third decimal.
fn format_js_fixed6(value: f64) -> String {
    let formatted = format!("{value:.6}");
    let Some(dot) = formatted.find('.') else {
        return formatted;
    };
    // `(\.\d{3}\d*?)0+$`: trailing zeros are stripped, but the first three
    // decimals never are ("2.000000" -> "2.000", "0.123450" -> "0.12345",
    // "0.123400" -> "0.1234").
    let trimmed = formatted.trim_end_matches('0');
    let keep = (dot + 4).max(trimmed.len());
    formatted[..keep].to_string()
}

#[cfg(test)]
#[path = "cache_warmer_tests.rs"]
mod tests;
