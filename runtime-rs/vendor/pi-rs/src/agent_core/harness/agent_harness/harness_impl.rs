//! Port of `packages/agent/src/harness/runtime/harness.ts` (408 lines): the
//! runtime [`Harness`] — the implementation of the public [`AgentHarness`]
//! interface — and [`create_agent_harness`] (upstream `createAgentHarness`,
//! the `AgentHarness.create` binding exported by `agent-harness.ts:622`).
//!
//! Disclosed substitutions:
//! - Upstream stores one shared `Models` object across all lanes. The Rust
//!   [`Lane`] takes `Models` by value (private credential/auth fields, no
//!   `Clone`), so each lane receives a catalog rebuilt from the harness's
//!   shared provider list ([`models_for_lane`]): providers registered before
//!   a lane is built propagate; registrations after lane construction reach
//!   only lanes built later (upstream's single live registry reaches every
//!   lane). The harness keeps the original catalog.
//! - Upstream `closePromise` (join of session close and per-lane idle-owner
//!   releases) is a [`tokio::sync::OnceCell`]; concurrent `close` calls await
//!   the same initialization.
//! - The fault path publishes the `fault` event fire-and-forget
//!   (upstream `void this.events.emit(...)`) and delivers it on the
//!   background context: the ported lane fault handler
//!   (`runtime::lane::FaultHandler`) does not thread the faulting operation's
//!   context.
//! - The lane `onFault` handler captures the harness core weakly; a lane
//!   faulting after the harness was dropped returns the cause unchanged
//!   (upstream lanes never outlive their harness).
//! - `assertOpen` throws; the port returns `Err`. Event delivery is awaited
//!   exactly where upstream awaits it (`setConfig`/`setName`/`setLabel`
//!   await, `fault` does not).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;

use super::{
    AcquireLaneOptions, AgentHarness, AgentHarnessOptions, AgentLane, CompactionOutcome,
    HarnessEvent, LaneInfo, NavigationOutcome, OpenOperation, OperationKind, PublicConfigProperty,
    RunOutcome, SessionSnapshot, SliceNotImplemented, SuspendedRun, SuspendedStatus, TaggedError,
    ValueUpdatePayload,
};
use crate::agent_core::harness::config::{
    validate_compaction_settings, validate_retry_policy, validate_tool_names, CompactionSettings,
    DEFAULT_COMPACTION_SETTINGS, DEFAULT_RETRY_POLICY,
};
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::events::HarnessEventBus;
use crate::agent_core::harness::execution::assistant::ToProviderMessagesFn;
use crate::agent_core::harness::hooks::{HookErrorReporter, HookName, HookRegistry};
use crate::agent_core::harness::messages::convert_to_llm;
use crate::agent_core::harness::result::{HarnessClosed, HarnessFault};
use crate::agent_core::harness::runtime::durable::{LaneState, OperationIntent};
use crate::agent_core::harness::runtime::lane::{
    native_tools::NativeRuntimeTools, EmitBatch, FaultHandler, Lane, ReadRuntimeConfig,
    RuntimeConfig, SealKind,
};
use crate::agent_core::harness::runtime::restore::{
    read_lane_storage, restore_lane_state, restore_session, ClassifiedLaneStorage,
};
use crate::agent_core::harness::session::{
    branch_tip, delete_value, entry_label, lane_config, lane_state, session_name, set_value,
    Control, EntryProjector, LaneConfiguration, LaneModel, OperationResultRecord,
    Session as SessionTrait, SessionMutator, StorageBackedSession, Write,
};
use crate::agent_core::harness::types::{AgentHarnessStreamOptions, AgentHarnessTool};
use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::models::{create_models, CreateModelsOptions, Models};
use crate::ai::retry::RetryPolicy;

/// The process-local harness configuration (upstream `Config<TContext>`,
/// `runtime/types.ts:26-39`).
pub struct HarnessConfig<TContext: Clone + Send + Sync + 'static> {
    pub tools: Vec<AgentHarnessTool<TContext>>,
    pub resources: super::Resources,
    pub stream_options: AgentHarnessStreamOptions,
    pub retry_policy: RetryPolicy,
    pub compaction: CompactionSettings,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub tool_execution: ToolExecutionMode,
    /// Upstream `toolContext: TContext | ((context) => TContext | Promise)`.
    pub tool_context:
        Option<crate::agent_core::harness::runtime::drive::tools::ToolContextSource<TContext>>,
    /// Upstream `systemPrompt`: the string form only (see the module docs).
    pub system_prompt: Option<String>,
    /// Upstream `toProviderMessages`; resolved to the default conversion
    /// (`convertToLlm`) at construction when the option is absent.
    pub to_provider_messages: Arc<ToProviderMessagesFn>,
    pub entry_projectors: BTreeMap<String, EntryProjector>,
}

/// One published lane keyed by name, preserving the upstream `Map` insertion
/// order for `lanes()`.
struct LaneDirectory {
    entries: Vec<(String, Arc<Lane>)>,
}

impl LaneDirectory {
    fn new() -> Self {
        LaneDirectory {
            entries: Vec::new(),
        }
    }

    fn get(&self, name: &str) -> Option<Arc<Lane>> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, lane)| Arc::clone(lane))
    }

    fn insert(&mut self, name: &str, lane: Arc<Lane>) {
        self.entries.push((name.to_owned(), lane));
    }

    fn snapshot(&self) -> Vec<Arc<Lane>> {
        self.entries
            .iter()
            .map(|(_, lane)| Arc::clone(lane))
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The harness internals (upstream private fields: `session`, `hooks`,
/// `events`, `lanesByName`, `seed`, `configStore`, `closedError`,
/// `faultError`, `closePromise`). Lane fault handlers reach this weakly, so
/// lanes never keep the harness alive.
struct HarnessCore<TContext: Clone + Send + Sync + 'static> {
    session: Arc<StorageBackedSession>,
    models: Arc<Models>,
    hooks: HookRegistry,
    events: HarnessEventBus<super::HarnessEvent>,
    lanes: Mutex<LaneDirectory>,
    config: Arc<Mutex<HarnessConfig<TContext>>>,
    closed_error: Mutex<Option<Arc<HarnessClosed>>>,
    fault_error: Mutex<Option<Arc<HarnessFault>>>,
    close_gate: tokio::sync::OnceCell<()>,
}

impl<TContext: Clone + Send + Sync + 'static> HarnessCore<TContext> {
    /// Upstream `assertOpen` (`runtime/harness.ts:368-371`): fault wins over
    /// closed.
    fn assert_open(&self) -> anyhow::Result<()> {
        if let Some(fault) = lock(&self.fault_error).clone() {
            return Err(anyhow::Error::new(fault));
        }
        if let Some(closed) = lock(&self.closed_error).clone() {
            return Err(anyhow::Error::new(closed));
        }
        Ok(())
    }

    /// Upstream `fault` (`runtime/harness.ts:309-320`): the first fault wins,
    /// an already-closed harness returns its closed error, every lane seals,
    /// hooks and the event bus close, and the `fault` event publishes without
    /// awaiting delivery.
    fn fault(self: &Arc<Self>, cause: anyhow::Error) -> anyhow::Error {
        if let Some(fault) = lock(&self.fault_error).clone() {
            return anyhow::Error::new(fault);
        }
        if let Some(closed) = lock(&self.closed_error).clone() {
            return anyhow::Error::new(closed);
        }
        let fault = Arc::new(HarnessFault::new(
            "AgentHarness storage or invariant fault",
            cause,
        ));
        *lock(&self.fault_error) = Some(Arc::clone(&fault));
        for lane in lock(&self.lanes).snapshot() {
            lane.seal(SealKind::Fault, fault.message.clone());
        }
        let fault_error: anyhow::Error = anyhow::Error::new(HarnessFault::new(
            fault.message.clone(),
            anyhow::anyhow!("{fault}"),
        ));
        self.hooks.close(clone_anyhow(&fault_error));
        let events = self.events.clone();
        let message = fault.message.clone();
        tokio::spawn(async move {
            events
                .emit(
                    HarnessEvent::Fault {
                        code: "harness_fault".to_owned(),
                        message,
                    },
                    Context::background(),
                )
                .await;
        });
        self.events.close(clone_anyhow(&fault_error));
        anyhow::Error::new(fault)
    }

    /// The lane emit adapter: lane-scoped runtime events lift into the public
    /// union and publish on the harness bus (upstream the lanes call the
    /// injected `emitBatch` bound to `this.events`).
    fn emit_batch_adapter(&self) -> EmitBatch {
        let bus = self.events.clone();
        Arc::new(
            move |batch: Vec<crate::agent_core::harness::runtime::events::HarnessEvent>,
                  context: Context| {
                let bus = bus.clone();
                Box::pin(async move {
                    let lifted: Vec<HarnessEvent> =
                        batch.into_iter().map(HarnessEvent::from).collect();
                    bus.emit_batch(lifted, context).await;
                    Ok(())
                }) as BoxFuture<'static, anyhow::Result<()>>
            },
        )
    }

    /// Upstream `buildLane` (`runtime/harness.ts:334-346`).
    fn build_lane(self: &Arc<Self>, name: &str, state: LaneState) -> Arc<Lane> {
        let on_fault: FaultHandler = {
            let core = Arc::downgrade(self);
            Arc::new(move |cause: anyhow::Error| match core.upgrade() {
                Some(core) => core.fault(cause),
                None => cause,
            })
        };
        let read_config: ReadRuntimeConfig = {
            let config = Arc::clone(&self.config);
            Arc::new(move || runtime_config_from(&lock(&config)))
        };
        let lane = Lane::new(
            name,
            Arc::clone(&self.session),
            models_for_lane(&self.models),
            self.hooks.clone(),
            state,
            on_fault,
            self.emit_batch_adapter(),
            read_config,
        );
        // Upstream passes `installWatch` (the harness event-bus bridge) as a
        // Lane constructor argument (`new Lane(..., installWatch, ...)`); the
        // port installs it right after construction.
        let events = self.events.clone();
        lane.install_watch_handler(Arc::new(move |capture, filter, context| {
            let events = events.clone();
            Box::pin(async move { events.watch_from_snapshot(capture, filter, context).await })
        }));
        lane
    }

    /// The close body (upstream `close`, `runtime/harness.ts:322-332`): the
    /// first close installs the closed error, seals every lane, closes hooks
    /// and the bus, then joins the session close with the lane idle-owner
    /// releases.
    async fn run_close(&self, context: Context) -> anyhow::Result<()> {
        let closed = Arc::new(HarnessClosed);
        *lock(&self.closed_error) = Some(Arc::clone(&closed));
        let mut owners = Vec::new();
        for lane in lock(&self.lanes).snapshot() {
            if let Some(owner) = lane.seal(SealKind::Closed, HarnessClosed::message()) {
                owners.push(owner);
            }
        }
        self.hooks.close(anyhow::Error::new(HarnessClosed));
        self.events.close(anyhow::Error::new(HarnessClosed));
        let (session_close, ()) =
            futures::join!(SessionTrait::close(&*self.session, context), async {
                for owner in owners {
                    owner.cancelled().await;
                }
            });
        session_close
    }
}

fn clone_anyhow(error: &anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

/// Upstream `models` sharing: rebuild one lane catalog from the shared
/// provider list (see the module docs for the divergence note).
fn models_for_lane(models: &Arc<Models>) -> Models {
    let mut rebuilt = create_models(CreateModelsOptions::default());
    for provider in models.get_providers() {
        rebuilt.set_provider(provider);
    }
    rebuilt
}

/// Upstream `Config` → the lane-facing [`RuntimeConfig`] projection
/// (`() => this.configStore.value`, narrowed to the runtime members).
fn runtime_config_from<TContext: Clone + Send + Sync + 'static>(
    config: &HarnessConfig<TContext>,
) -> RuntimeConfig {
    let native_tools = NativeRuntimeTools::new(config.tools.clone(), config.tool_context.clone());
    RuntimeConfig {
        compaction: config.compaction,
        retry_policy: config.retry_policy.clone(),
        stream_options: config.stream_options.clone(),
        resources: config.resources.clone(),
        system_prompt: config.system_prompt.clone(),
        tools: native_tools.declarations(),
        native_tools,
        to_provider_messages: Some(Arc::clone(&config.to_provider_messages)),
        steering_mode: config.steering_mode,
        follow_up_mode: config.follow_up_mode,
        tool_execution: config.tool_execution,
        entry_projectors: Some(config.entry_projectors.clone()),
    }
}

/// The upstream `HookRegistry` error reporter (`runtime/harness.ts:46-58`):
/// report every handler failure as a lane-scoped `handler_error` event.
fn report_hook_error(bus: HarnessEventBus<super::HarnessEvent>) -> HookErrorReporter {
    Arc::new(
        move |error: anyhow::Error, hook: HookName, lane: String, context: Context| {
            let bus = bus.clone();
            Box::pin(async move {
                bus.emit(
                    super::HarnessEvent::HandlerError {
                        kind: super::HandlerErrorKind::Hook,
                        hook: Some(hook.as_str().to_owned()),
                        event: None,
                        error: error.to_string(),
                        stack: None,
                        lane: Some(lane),
                    },
                    context,
                )
                .await;
            }) as BoxFuture<'static, ()>
        },
    )
}

/// Upstream `Harness<TContext>` (`runtime/harness.ts:29`): the runtime
/// implementation of AgentHarness — it manages lanes but is not itself a
/// lane.
pub struct Harness<TContext: Clone + Send + Sync + 'static> {
    core: Arc<HarnessCore<TContext>>,
    seed: LaneConfiguration,
}

impl<TContext: Clone + Send + Sync + 'static> std::fmt::Debug for Harness<TContext> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Harness").finish_non_exhaustive()
    }
}

/// The publication decided inside the `lane()` mutation callback.
enum LanePublication {
    Existing(Arc<Lane>),
    Published {
        lane: Arc<Lane>,
        delivery: Option<BoxFuture<'static, ()>>,
    },
}

fn invalid_lane(lane: &str, reason: &str) -> anyhow::Error {
    anyhow::Error::new(TaggedError::InvalidLane {
        lane: lane.to_owned(),
        reason: reason.to_owned(),
        message: format!(
            "Invalid lane {}: {reason}",
            serde_json::to_string(lane).unwrap_or_default()
        ),
    })
}

impl<TContext: Clone + Send + Sync + 'static> Harness<TContext> {
    /// The attached session, for tests and the lane-drive slices.
    #[allow(dead_code)] // consumed by the tests; the lane-drive slices join later
    pub(super) fn session(&self) -> &StorageBackedSession {
        &self.core.session
    }

    /// Upstream `lane` (`runtime/harness.ts:80-158`): atomically get or
    /// create one complete AgentLane under the session mutation line.
    pub async fn lane(
        &self,
        name: &str,
        options: AcquireLaneOptions,
        context: Context,
    ) -> anyhow::Result<Arc<Lane>> {
        self.core.assert_open()?;
        if name.is_empty() || name.contains('\0') {
            let reason = if name.is_empty() {
                "lane name must not be empty"
            } else {
                // The upstream literal spells the escape sequence.
                r"lane name must not contain \u0000"
            };
            return Err(invalid_lane(name, reason));
        }
        let core = Arc::clone(&self.core);
        let seed = self.seed.clone();
        let name = name.to_owned();
        let session = Arc::clone(&core.session);
        let publication = session
            .mutate(
                move |mutator: &dyn SessionMutator, context: Context| {
                    let core = Arc::clone(&core);
                    let name = name.clone();
                    let options = options.clone();
                    let seed = seed.clone();
                    Box::pin(async move {
                        core.assert_open()?;
                        if let Some(existing) = lock(&core.lanes).get(&name) {
                            return Ok(LanePublication::Existing(existing));
                        }
                        let stored = read_lane_storage(mutator, &name, context.clone()).await?;
                        if let ClassifiedLaneStorage::Lane(stored) = &stored {
                            let restored =
                                restore_lane_state(mutator, &name, stored, context.clone()).await?;
                            let lane = core.build_lane(&name, restored);
                            lock(&core.lanes).insert(&name, Arc::clone(&lane));
                            return Ok(LanePublication::Published {
                                lane,
                                delivery: None,
                            });
                        }
                        let tip_id = match &stored {
                            ClassifiedLaneStorage::Branch { tip } => {
                                tip.value.as_str().map(str::to_owned)
                            }
                            _ => options.create_at.clone(),
                        };
                        if matches!(stored, ClassifiedLaneStorage::Absent) && tip_id.is_some() {
                            let tip_id = tip_id.clone().expect("checked above");
                            let entries = mutator
                                .get_entries(std::slice::from_ref(&tip_id), context.clone())
                                .await?;
                            if !entries.contains_key(&tip_id) {
                                let message = format!("Unknown target: {tip_id}");
                                return Err(anyhow::Error::new(TaggedError::UnknownTarget {
                                    target_id: tip_id,
                                    message,
                                }));
                            }
                        }
                        let attached_configuration = LaneConfiguration {
                            model: seed.model.clone(),
                            thinking_level: seed.thinking_level,
                            active_tool_names: seed.active_tool_names.clone(),
                        };
                        let state = LaneState {
                            tip_id: tip_id.clone(),
                            configuration: attached_configuration.clone(),
                            inbox: Vec::new(),
                            last_operation_id: None,
                            operation: None,
                        };
                        let mut writes: Vec<Write> = Vec::new();
                        if matches!(stored, ClassifiedLaneStorage::Absent) {
                            writes.push(set_value(
                                &branch_tip(&name),
                                match &tip_id {
                                    Some(tip) => serde_json::Value::String(tip.clone()),
                                    None => serde_json::Value::Null,
                                },
                            ));
                        }
                        writes.push(set_value(
                            &lane_config(&name),
                            serde_json::to_value(&attached_configuration)?,
                        ));
                        writes.push(set_value(
                            &lane_state(&name),
                            serde_json::to_value(crate::agent_core::harness::session::LaneState {
                                current_operation_id: None,
                                last_operation_id: None,
                                inbox: serde_json::Value::Array(Vec::new()),
                            })?,
                        ));
                        mutator.commit(writes, context.clone()).await?;
                        let lane = core.build_lane(&name, state);
                        lock(&core.lanes).insert(&name, Arc::clone(&lane));
                        let delivery = core.events.emit_batch(
                            vec![HarnessEvent::LaneCreated {
                                lane: name,
                                at: tip_id,
                            }],
                            context,
                        );
                        Ok(LanePublication::Published {
                            lane,
                            delivery: Some(delivery),
                        })
                    })
                },
                context.clone(),
            )
            .await;
        let publication = match publication {
            Ok(publication) => publication,
            Err(error) => {
                // Upstream catch (`runtime/harness.ts:149-153`): closed wins,
                // InvalidLane/UnknownTarget rethrow, everything else faults.
                if let Some(closed) = lock(&self.core.closed_error).clone() {
                    return Err(anyhow::Error::new(closed));
                }
                if let Some(tagged) = error.downcast_ref::<TaggedError>() {
                    if matches!(
                        tagged,
                        TaggedError::InvalidLane { .. } | TaggedError::UnknownTarget { .. }
                    ) {
                        return Err(error);
                    }
                }
                return Err(self.core.fault(error));
            }
        };
        match publication {
            LanePublication::Existing(lane) => Ok(lane),
            LanePublication::Published { lane, delivery } => {
                if let Some(delivery) = delivery {
                    delivery.await;
                }
                Ok(lane)
            }
        }
    }

    /// Upstream `setName`/`setLabel` bodies (`runtime/harness.ts:177-194`,
    /// `177-219`): one metadata commit plus its awaited `value_update`
    /// delivery.
    async fn set_value_with_event<F, W>(
        &self,
        write: W,
        event: F,
        context: Context,
    ) -> anyhow::Result<()>
    where
        W: FnOnce() -> Write + Send + 'static,
        F: FnOnce() -> HarnessEvent + Send + 'static,
    {
        self.core.assert_open()?;
        let core = Arc::clone(&self.core);
        let session = Arc::clone(&core.session);
        let outcome = session
            .mutate(
                move |mutator: &dyn SessionMutator, context: Context| {
                    let core = Arc::clone(&core);
                    Box::pin(async move {
                        core.assert_open()?;
                        mutator.commit(vec![write()], context.clone()).await?;
                        Ok(core.events.emit_batch(vec![event()], context))
                    })
                },
                context.clone(),
            )
            .await;
        let delivery = match outcome {
            Ok(delivery) => delivery,
            Err(error) => {
                if let Some(closed) = lock(&self.core.closed_error).clone() {
                    return Err(anyhow::Error::new(closed));
                }
                return Err(self.core.fault(error));
            }
        };
        delivery.await;
        Ok(())
    }

    /// Upstream `setConfig` (`runtime/harness.ts:356-366`): replace the
    /// config value and publish the config event (awaited).
    async fn set_config<F>(&self, apply: F, context: Context) -> anyhow::Result<()>
    where
        F: FnOnce(&mut HarnessConfig<TContext>) -> HarnessEvent + Send + 'static,
    {
        self.core.assert_open()?;
        let event = {
            let mut config = lock(&self.core.config);
            let mut next = clone_config(&config);
            let event = apply(&mut next);
            *config = next;
            event
        };
        self.core.events.emit_batch(vec![event], context).await;
        Ok(())
    }
}

/// The config mutex holds the authoritative value; `set_config` clones,
/// applies, and replaces (upstream replaces the whole configStore object).
fn clone_config<TContext: Clone + Send + Sync + 'static>(
    config: &HarnessConfig<TContext>,
) -> HarnessConfig<TContext> {
    HarnessConfig {
        tools: config.tools.clone(),
        resources: config.resources.clone(),
        stream_options: config.stream_options.clone(),
        retry_policy: config.retry_policy.clone(),
        compaction: config.compaction,
        steering_mode: config.steering_mode,
        follow_up_mode: config.follow_up_mode,
        tool_execution: config.tool_execution,
        tool_context: config.tool_context.clone(),
        system_prompt: config.system_prompt.clone(),
        to_provider_messages: Arc::clone(&config.to_provider_messages),
        entry_projectors: config.entry_projectors.clone(),
    }
}

impl<TContext: Clone + Send + Sync + 'static> AgentHarness<TContext> for Harness<TContext> {
    fn lane<'a>(
        &'a self,
        name: &str,
        options: AcquireLaneOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Arc<Lane>>> {
        let name = name.to_owned();
        Box::pin(async move { Harness::lane(self, &name, options, context).await })
    }

    fn lanes<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Vec<LaneInfo>>> {
        Box::pin(async move {
            self.core.assert_open()?;
            let lanes = lock(&self.core.lanes).snapshot();
            let mut infos = Vec::new();
            for lane in lanes {
                let execution = AgentLane::inspect_execution(&*lane, context.clone()).await?;
                infos.push(LaneInfo {
                    name: execution.lane,
                    tip_id: execution.tip_id,
                    operation: execution.current,
                });
            }
            Ok(infos)
        })
    }

    fn get_name<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move {
            self.core.assert_open()?;
            SessionTrait::get_name(&*self.core.session, context).await
        })
    }

    fn set_name<'a>(
        &'a self,
        name: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let write_name = name.clone();
            let event_name = name.clone();
            self.set_value_with_event(
                move || match &write_name {
                    None => delete_value(&session_name()),
                    Some(name) => {
                        set_value(&session_name(), serde_json::Value::String(name.clone()))
                    }
                },
                move || HarnessEvent::ValueUpdate {
                    update: ValueUpdatePayload::SessionName {
                        name: event_name.clone(),
                    },
                },
                context,
            )
            .await
        })
    }

    fn get_label<'a>(
        &'a self,
        target_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        let target_id = target_id.to_owned();
        Box::pin(async move {
            self.core.assert_open()?;
            SessionTrait::get_label(&*self.core.session, &target_id, context).await
        })
    }

    fn set_label<'a>(
        &'a self,
        target_id: &str,
        label: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let write_target = target_id.to_owned();
        let event_target = target_id.to_owned();
        let write_label = label.clone();
        let event_label = label.clone();
        Box::pin(async move {
            self.set_value_with_event(
                move || {
                    let address = entry_label(&write_target);
                    match &write_label {
                        None => delete_value(&address),
                        Some(label) => {
                            set_value(&address, serde_json::Value::String(label.clone()))
                        }
                    }
                },
                move || HarnessEvent::ValueUpdate {
                    update: ValueUpdatePayload::EntryLabel {
                        target_id: event_target.clone(),
                        label: event_label.clone(),
                    },
                },
                context,
            )
            .await
        })
    }

    fn get_tools<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<AgentHarnessTool<TContext>>>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).tools.clone())
        })
    }

    fn set_tools<'a>(
        &'a self,
        tools: Vec<AgentHarnessTool<TContext>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            validate_tool_names(tools.iter().map(|tool| tool.name.as_str()))?;
            self.set_config(
                move |config| {
                    config.tools = tools;
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::Tools,
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_resources<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<super::Resources>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).resources.clone())
        })
    }

    fn set_resources<'a>(
        &'a self,
        resources: super::Resources,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.set_config(
                move |config| {
                    config.resources = resources;
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::Resources,
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_stream_options<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<AgentHarnessStreamOptions>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).stream_options.clone())
        })
    }

    fn set_stream_options<'a>(
        &'a self,
        options: AgentHarnessStreamOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.set_config(
                move |config| {
                    let previous = config.stream_options.clone();
                    config.stream_options = options.clone();
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::StreamOptions {
                            value: options,
                            previous,
                        },
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_retry_policy<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RetryPolicy>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).retry_policy.clone())
        })
    }

    fn set_retry_policy<'a>(
        &'a self,
        policy: RetryPolicy,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            validate_retry_policy(&policy)?;
            self.set_config(
                move |config| {
                    let previous = config.retry_policy.clone();
                    config.retry_policy = policy.clone();
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::RetryPolicy {
                            value: policy,
                            previous,
                        },
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_compaction_settings<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CompactionSettings>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).compaction)
        })
    }

    fn set_compaction_settings<'a>(
        &'a self,
        settings: CompactionSettings,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            validate_compaction_settings(&settings)?;
            self.set_config(
                move |config| {
                    let previous = config.compaction;
                    config.compaction = settings;
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::CompactionSettings {
                            value: settings,
                            previous,
                        },
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_steering_mode<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueMode>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).steering_mode)
        })
    }

    fn set_steering_mode<'a>(
        &'a self,
        mode: QueueMode,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.set_config(
                move |config| {
                    let previous = config.steering_mode;
                    config.steering_mode = mode;
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::SteeringMode {
                            value: mode,
                            previous,
                        },
                    }
                },
                context,
            )
            .await
        })
    }

    fn get_follow_up_mode<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueMode>> {
        let _ = context;
        Box::pin(async move {
            self.core.assert_open()?;
            Ok(lock(&self.core.config).follow_up_mode)
        })
    }

    fn set_follow_up_mode<'a>(
        &'a self,
        mode: QueueMode,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.set_config(
                move |config| {
                    let previous = config.follow_up_mode;
                    config.follow_up_mode = mode;
                    HarnessEvent::ConfigUpdate {
                        lane: None,
                        recovery: false,
                        property: PublicConfigProperty::FollowUpMode {
                            value: mode,
                            previous,
                        },
                    }
                },
                context,
            )
            .await
        })
    }

    fn watch_session<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<super::HarnessWatchHandle<SessionSnapshot>>> {
        let _ = (self, context);
        Box::pin(async { Err(anyhow::Error::new(SliceNotImplemented::new("watchSession"))) })
    }

    fn hooks(&self) -> &HookRegistry {
        &self.core.hooks
    }

    fn events(&self) -> &super::Events {
        &self.core.events
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            // Upstream returns the same closePromise to every caller; the
            // port's OnceCell replays completion (the session close cannot
            // fail in this port, so the error channel is theoretical).
            self.core
                .close_gate
                .get_or_try_init(|| async move { self.core.run_close(context).await })
                .await
                .map(|_| ())
        })
    }
}

/// Upstream `createAgentHarness` (`runtime/harness.ts:375-408`): attach the
/// durable harness to one open session without starting provider, tool, hook,
/// or timer effects. Upstream `AgentHarness.create`
/// (`agent-harness.ts:622`).
pub async fn create_agent_harness<TContext: Clone + Send + Sync + 'static>(
    options: AgentHarnessOptions<TContext>,
    context: Context,
) -> anyhow::Result<(Harness<TContext>, Vec<OpenOperation>)> {
    let tools = options.tools.clone().unwrap_or_default();
    validate_tool_names(tools.iter().map(|tool| tool.name.as_str()))?;
    let retry_policy = options.retry.clone().unwrap_or(DEFAULT_RETRY_POLICY);
    let compaction = options.compaction.unwrap_or(DEFAULT_COMPACTION_SETTINGS);
    validate_retry_policy(&retry_policy)?;
    validate_compaction_settings(&compaction)?;
    let seed = LaneConfiguration {
        model: LaneModel {
            provider: options.model.provider.clone(),
            model_id: options.model.id.clone(),
        },
        thinking_level: options.thinking_level.unwrap_or(ThinkingLevel::Off),
        active_tool_names: options
            .active_tool_names
            .clone()
            .unwrap_or_else(|| tools.iter().map(|tool| tool.name.clone()).collect()),
    };
    let to_provider_messages: Arc<ToProviderMessagesFn> = match options.to_provider_messages {
        Some(callback) => callback,
        None => Arc::new(|messages, _context| Box::pin(async move { convert_to_llm(&messages) })),
    };
    let entry_projectors = options.entry_projectors.clone().unwrap_or_default();
    // Upstream wraps every construction failure in HarnessFault.
    let restored = restore_session(&*options.session, context.clone())
        .await
        .map_err(|error| {
            anyhow::Error::new(HarnessFault::new(
                "AgentHarness storage or invariant fault",
                error,
            ))
        })?;
    let mut open: Vec<OpenOperation> = Vec::new();
    for (lane, state) in &restored {
        if let Some(operation) = &state.operation {
            open.push(OpenOperation {
                lane: lane.clone(),
                operation_id: operation.meta.operation_id.clone(),
                kind: intent_kind(&operation.meta.intent),
                started_at: operation.meta.started_at,
                aborting: matches!(
                    operation.state.scope.control,
                    Control::CancelRequested { .. }
                ),
            });
        }
    }
    let harness_config = HarnessConfig {
        tools,
        resources: options.resources.clone().unwrap_or_default(),
        stream_options: options.stream_options.clone().unwrap_or_default(),
        retry_policy,
        compaction,
        // Upstream defaults (`runtime/harness.ts:66-67`): the harness config
        // defaults both queues to "all" (distinct from the AgentOptions
        // default QueueMode::DEFAULT).
        steering_mode: options.steering_mode.unwrap_or(QueueMode::All),
        follow_up_mode: options.follow_up_mode.unwrap_or(QueueMode::All),
        tool_execution: options.tool_execution.unwrap_or(ToolExecutionMode::DEFAULT),
        tool_context: options.tool_context,
        system_prompt: options.system_prompt.clone(),
        to_provider_messages,
        entry_projectors,
    };
    let models = Arc::new(options.models);
    let events: HarnessEventBus<super::HarnessEvent> = HarnessEventBus::new();
    let core = Arc::new(HarnessCore {
        session: Arc::clone(&options.session),
        models: Arc::clone(&models),
        hooks: HookRegistry::new(report_hook_error(events.clone())),
        events,
        lanes: Mutex::new(LaneDirectory::new()),
        config: Arc::new(Mutex::new(harness_config)),
        closed_error: Mutex::new(None),
        fault_error: Mutex::new(None),
        close_gate: tokio::sync::OnceCell::new(),
    });
    for (lane, state) in restored {
        let built = core.build_lane(&lane, state);
        lock(&core.lanes).insert(&lane, built);
    }
    Ok((Harness { core, seed }, open))
}

fn intent_kind(intent: &OperationIntent) -> OperationKind {
    match intent {
        OperationIntent::Run { .. } => OperationKind::Run,
        OperationIntent::Compaction { .. } => OperationKind::Compaction,
        OperationIntent::Navigation { .. } => OperationKind::Navigation,
    }
}

/// Upstream result-value constructors kept beside the port for the lane
/// slices that map their results into the public aliases.
#[allow(dead_code)]
pub(crate) fn suspended_run(
    operation_id: String,
    deferred: crate::ai::types::options::DeferredHandle,
) -> SuspendedRun {
    SuspendedRun {
        operation_id,
        status: SuspendedStatus::Suspended,
        deferred,
    }
}

#[allow(dead_code)]
pub(crate) fn compaction_outcome(
    compaction: OperationResultRecord,
    run: Option<RunOutcome>,
) -> super::CompactionOutcome {
    CompactionOutcome { compaction, run }
}

#[allow(dead_code)]
pub(crate) fn navigation_outcome(
    navigation: OperationResultRecord,
    run: Option<RunOutcome>,
) -> super::NavigationOutcome {
    NavigationOutcome { navigation, run }
}
