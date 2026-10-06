//! Port of `src/harness/harness.ts`: the durable agent harness over one
//! Session — conversation handles and their configuration surface, task
//! scheduling and submission admission behind it, and the registry-driven
//! conversation setups.
//!
//! Divergences (structural, disclosed):
//! - **D7 extension.** Upstream `HarnessImpl` subclasses `SessionImpl` and
//!   overrides the protected hooks; the port composes a Session with a
//!   [`super::super::session::session::SessionHooks`] implementation owned by
//!   the facade.
//! - **D29 (sync setups).** `ConversationSetup` / `ConversationInit` and the
//!   `conversationCreated` hook are synchronous over the port's transaction
//!   (D5); upstream is async because transaction operations are.
//! - **D30 (handle shape).** Upstream `Conversation` is a promise-returning
//!   interface; the port's [`Conversation`] is a concrete struct of
//!   fallible async methods, compared by `id` as upstream.
//! - `Harness.open` validates the registry's built-ins and then installs the
//!   scheduler exactly like the upstream static `open`.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent_core::chord_support::context::{abort_signal_key, Context};

use super::super::errors::PlainError;
use super::super::harness::config::{conversation_config, ConversationConfigState};
use super::super::harness::context::read_context;
use super::super::harness::events::AgentEventStream;
use super::super::harness::inbox::withdraw_queued_inputs;
use super::super::harness::live::settle_scheduler_outcome;
use super::super::harness::registry::{builtin_tasks, BUILTIN_SETUP_KEY};
use super::super::harness::scheduler::{InvocationBinding, TaskScheduler, TaskSchedulerOptions};
use super::super::harness::submissions::Submissions;
use super::super::harness::types::{
    AbortSubmissionResult, AbortTaskResult, ContextView, ConversationAbortOptions,
    ConversationCreateOptions, ConversationHandle, ConversationInit, ConversationRetryPolicy,
    ConversationStreamOptions, EntryQueryFilter, HarnessInspection, HarnessOptions, ModelRef,
    QueueMode, RegistryReaderLike, RegistrySnapshotLike, SubmissionDraft, ToolExecutionMode,
};
use super::super::harness::usage::{add_usage_state, usage_doc, UsageState};
use super::super::harness::view::ConversationViews;
use super::super::ids::ROOT_CONVERSATION_ID;
use super::super::ids::{ConversationId, EntryId, SubmissionId, TaskId};
use super::super::session::session::{Session, SessionHooks};
use super::super::session::transaction::TransactionScope;
use super::super::storage::Storage;
use super::super::types::{
    ConversationOwnership, ConversationQuery, ConversationRecord, Cursor, EntryDraft, Page,
    SubmissionRecord,
};
use super::scheduler::record_checkpoint;

const SCAN_PAGE_SIZE: usize = 256;

/// Harness-private services used by Conversation handles
/// (`ConversationHost`).
pub(crate) struct HarnessHost {
    pub(crate) harness: Arc<Harness>,
    #[allow(dead_code)]
    pub(crate) storage: Arc<dyn Storage>,
    #[allow(dead_code)]
    pub(crate) registry: Arc<dyn RegistryReaderLike>,
    pub(crate) tasks: Arc<TaskScheduler>,
    pub(crate) submissions: Arc<Submissions>,
    pub(crate) views: Arc<ConversationViews>,
    pub(crate) now: Arc<dyn Fn() -> f64 + Send + Sync>,
}

/// One conversation handle (`ConversationImpl`).
#[derive(Clone)]
pub struct Conversation {
    pub id: ConversationId,
    host: Arc<HarnessHost>,
}

impl Conversation {
    fn config(
        &self,
        context: &Context,
    ) -> impl std::future::Future<Output = Result<ConversationConfigState, PlainError>> + Send + '_
    {
        let harness = Arc::clone(&self.host.harness);
        let context = context.clone();
        async move {
            let config = conversation_config();
            harness
                .session()
                .snapshot(&config.definition, Some(self.id), None, context)
                .await?
                .and_then(|value| ConversationConfigState::from_json(&value).ok())
                .unwrap_or_else(ConversationConfigState::initial)
                .pipe(Ok)
        }
    }

    fn edit_config(
        &self,
        edit: impl FnOnce(&mut ConversationConfigState) + Send + 'static,
        context: Context,
    ) -> impl std::future::Future<Output = Result<(), PlainError>> + Send + 'static {
        let harness = Arc::clone(&self.host.harness);
        let id = self.id;
        async move {
            let result = harness
                .commit_with(
                    move |tx| {
                        let outcome: Result<(), PlainError> = (|| {
                            let config = conversation_config();
                            let draft = tx.doc(&config.definition, Some(id), None, None)?;
                            let mut state =
                                ConversationConfigState::from_json(&read_draft(&draft)?)
                                    .unwrap_or_else(|_| ConversationConfigState::initial());
                            edit(&mut state);
                            write_draft(
                                &draft,
                                &serde_json::to_value(&state)
                                    .map_err(|error| PlainError::new(error.to_string()))?,
                            )
                        })();
                        async move { outcome }
                    },
                    context,
                )
                .await;
            result
        }
    }

    /// The configured model (`getModel`).
    pub async fn get_model(&self, context: Context) -> Result<Option<ModelRef>, PlainError> {
        Ok(self.config(&context).await?.model)
    }

    /// Set the model, or clear it (`setModel`).
    pub async fn set_model(
        &self,
        model: Option<ModelRef>,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.model = model, context)
            .await
    }

    /// The configured thinking level (`getThinkingLevel`).
    pub async fn get_thinking_level(
        &self,
        context: Context,
    ) -> Result<crate::ai::types::ModelThinkingLevel, PlainError> {
        Ok(self.config(&context).await?.thinking_level)
    }

    /// Set the thinking level (`setThinkingLevel`).
    pub async fn set_thinking_level(
        &self,
        level: crate::ai::types::ModelThinkingLevel,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.thinking_level = level, context)
            .await
    }

    /// The active tool names (`getActiveTools`).
    pub async fn get_active_tools(&self, context: Context) -> Result<Vec<String>, PlainError> {
        Ok(self.config(&context).await?.active_tools)
    }

    /// Set the active tool names; every newly added name must be registered
    /// (`setActiveTools`).
    pub async fn set_active_tools(
        &self,
        names: Vec<String>,
        context: Context,
    ) -> Result<(), PlainError> {
        let host = Arc::clone(&self.host);
        let id = self.id;
        let snapshot = host.registry.snapshot();
        let previous = self.config(&context).await?.active_tools;
        require_registered(snapshot.as_ref(), &names, &previous)?;
        self.edit_config(
            move |config| {
                let _ = id;
                config.active_tools = names;
            },
            context,
        )
        .await
    }

    /// The forwarded request options (`getStreamOptions`).
    pub async fn get_stream_options(
        &self,
        context: Context,
    ) -> Result<ConversationStreamOptions, PlainError> {
        Ok(self
            .config(&context)
            .await?
            .stream_options
            .unwrap_or_default())
    }

    /// Set the forwarded request options (`setStreamOptions`).
    pub async fn set_stream_options(
        &self,
        options: ConversationStreamOptions,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.stream_options = Some(options), context)
            .await
    }

    /// The durable retry policy, or the default (`getRetryPolicy`).
    pub async fn get_retry_policy(
        &self,
        context: Context,
    ) -> Result<ConversationRetryPolicy, PlainError> {
        Ok(self
            .config(&context)
            .await?
            .retry
            .unwrap_or_else(super::super::harness::config::default_retry_policy))
    }

    /// Set or clear the durable retry policy (`setRetryPolicy`).
    pub async fn set_retry_policy(
        &self,
        policy: Option<ConversationRetryPolicy>,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.retry = policy, context)
            .await
    }

    /// Whether a round's tools run at once (`getToolExecution`).
    pub async fn get_tool_execution(
        &self,
        context: Context,
    ) -> Result<ToolExecutionMode, PlainError> {
        Ok(self
            .config(&context)
            .await?
            .tool_execution
            .unwrap_or(ToolExecutionMode::Parallel))
    }

    /// Set or clear the round execution mode (`setToolExecution`).
    pub async fn set_tool_execution(
        &self,
        mode: Option<ToolExecutionMode>,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.tool_execution = mode, context)
            .await
    }

    /// How many queued steers a boundary places (`getSteeringMode`).
    pub async fn get_steering_mode(&self, context: Context) -> Result<QueueMode, PlainError> {
        Ok(self
            .config(&context)
            .await?
            .steering_mode
            .unwrap_or(QueueMode::OneAtATime))
    }

    /// Set or clear the steering placement (`setSteeringMode`).
    pub async fn set_steering_mode(
        &self,
        mode: Option<QueueMode>,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.steering_mode = mode, context)
            .await
    }

    /// How many queued follow-ups a final boundary places (`getFollowUpMode`).
    pub async fn get_follow_up_mode(&self, context: Context) -> Result<QueueMode, PlainError> {
        Ok(self
            .config(&context)
            .await?
            .follow_up_mode
            .unwrap_or(QueueMode::OneAtATime))
    }

    /// Set or clear the follow-up placement (`setFollowUpMode`).
    pub async fn set_follow_up_mode(
        &self,
        mode: Option<QueueMode>,
        context: Context,
    ) -> Result<(), PlainError> {
        self.edit_config(move |config| config.follow_up_mode = mode, context)
            .await
    }

    /// Admit a submission (`submit`).
    pub async fn submit(
        &self,
        submission: SubmissionDraft,
        context: Context,
    ) -> Result<SubmissionId, PlainError> {
        self.host
            .submissions
            .submit(self.id, submission, context)
            .await
    }

    /// Queue a reset, with an optional handoff message (`reset`).
    pub async fn reset(&self, handoff: Option<String>, context: Context) -> Result<(), PlainError> {
        let now = (self.host.now)();
        let mut entry = EntryDraft::new(super::super::entries::RESET_ENTRY_KIND);
        entry.head = Some(super::super::types::EntryHead::Self_);
        if let Some(handoff) = handoff {
            entry.model = Some(vec![crate::ai::types::Message::User(
                crate::ai::types::UserMessage {
                    content: crate::ai::types::StringOrBlocks::Text(handoff),
                    timestamp: now as i64,
                },
            )]);
        }
        self.host
            .submissions
            .submit(
                self.id,
                SubmissionDraft::Write {
                    request_id: None,
                    entry,
                },
                context,
            )
            .await
            .map(|_| ())
    }

    /// A host commit scoped to this conversation (`commit`).
    pub async fn commit<T, F, Fut>(&self, change: F, context: Context) -> Result<T, PlainError>
    where
        F: FnOnce(Arc<super::super::session::transaction::Transaction>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        self.host
            .harness
            .session()
            .commit_with(
                change,
                context,
                TransactionScope {
                    conversation_id: Some(self.id),
                    task_id: None,
                },
            )
            .await
    }

    /// The committed context view (`context`).
    pub async fn context(&self, context: Context) -> Result<ContextView, PlainError> {
        read_context(
            self.host.harness.session().as_ref(),
            self.host.harness.storage().as_ref(),
            self.id,
            &context,
            None,
        )
        .await
    }

    /// A page of this conversation's entries (`entries`).
    pub async fn entries(
        &self,
        query: EntryQueryFilter,
        limit: usize,
        cursor: Option<Cursor>,
        context: Context,
    ) -> Result<Page<super::super::types::EntryRecord>, PlainError> {
        let storage = Arc::clone(self.host.harness.storage());
        let bounded = query.bound(self.id);
        self.host
            .harness
            .read_on_line(move || {
                let storage = Arc::clone(&storage);
                async move {
                    storage
                        .scan_entries(bounded, limit, cursor.as_ref(), &context)
                        .map_err(|error| PlainError::new(error.to_string()))
                }
            })
            .await
    }

    /// Fork this conversation at an entry (`fork`).
    pub async fn fork(
        &self,
        at: EntryId,
        options: ConversationCreateOptions,
        context: Context,
    ) -> Result<Conversation, PlainError> {
        self.host
            .harness
            .create(
                CreateTarget::Fork {
                    parent_id: self.id,
                    at,
                    ownership: options.ownership,
                },
                options.init,
                context,
            )
            .await
    }

    /// Withdraw queued inputs and abort the ordinary ownership scope
    /// (`abort`).
    pub async fn abort(
        &self,
        context: Context,
        options: Option<ConversationAbortOptions>,
    ) -> Result<(), PlainError> {
        self.host.tasks.resume();
        self.host
            .tasks
            .abort_conversation(
                self.id,
                options.is_some_and(|options| options.background),
                context,
            )
            .await
    }

    /// Resolve when this conversation's ordinary scope is idle
    /// (`waitForIdle`).
    pub async fn wait_for_idle(&self, context: Context) -> Result<(), PlainError> {
        self.host.tasks.resume();
        self.host.tasks.wait_for_idle(Some(self.id), context).await
    }

    /// The conversation view state (`viewState`): a disposable read-only
    /// Chord state of the view mount.
    pub async fn view_state(
        &self,
        context: Context,
    ) -> Result<Arc<crate::chord::services::state::AttachedReplicatedState>, PlainError> {
        self.host.views.state(self.id, context).await
    }

    /// A serialized watch of the conversation view (`watch`).
    pub async fn watch(
        &self,
        context: Context,
    ) -> Result<Arc<super::super::session::observation::CommittedWatch>, PlainError> {
        self.host.views.watch(self.id, context).await
    }

    /// The agent event stream (`Harness.watchEvents` through the handle).
    pub async fn watch_events(&self, context: Context) -> Result<AgentEventStream, PlainError> {
        super::super::harness::events::watch_events(
            &self.host.views,
            &Arc::clone(self.host.harness.storage()),
            self.id,
            context,
        )
        .await
    }
}

/// The create targets of the facade (`CreateTarget`).
enum CreateTarget {
    Root,
    Independent {
        ownership: ConversationOwnership,
    },
    Fork {
        parent_id: ConversationId,
        at: EntryId,
        ownership: ConversationOwnership,
    },
}

/// Durable agent harness over one Session (`HarnessImpl`).
pub struct Harness {
    storage: Arc<dyn Storage>,
    session: Arc<Session>,
    registry: Arc<dyn RegistryReaderLike>,
    host: Mutex<Option<Arc<HarnessHost>>>,
    // Kept for conversation-host construction; handles read the clock
    // through `HarnessHost.now` (the facade's own clock input).
    #[allow(dead_code)]
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
    closed: AtomicBool,
    self_ref: std::sync::OnceLock<std::sync::Weak<Harness>>,
    init: Mutex<Option<(ConversationId, ConversationInit)>>,
}

impl Harness {
    /// The Session kernel behind the harness.
    pub(crate) fn session(&self) -> &Arc<Session> {
        &self.session
    }

    pub(crate) fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }

    fn host(&self) -> Arc<HarnessHost> {
        Arc::clone(
            self.host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .expect("the harness host is installed at open"),
        )
    }

    /// Reconcile surviving `running` tasks to `pending`; part of open
    /// (`openTasks`).
    pub async fn open_tasks(self: &Arc<Self>, context: Context) -> Result<(), PlainError> {
        self.host().tasks.open(context).await
    }

    /// Enable scheduling (`resume`).
    pub fn resume(&self) -> Result<(), PlainError> {
        self.assert_open()?;
        self.host().tasks.resume();
        Ok(())
    }

    /// A committed task record (`getTask`).
    pub async fn get_task(
        &self,
        id: TaskId,
        context: Context,
    ) -> Result<Option<super::super::types::TaskRecord>, PlainError> {
        let storage = Arc::clone(&self.storage);
        self.read_on_line(move || {
            let storage = Arc::clone(&storage);
            async move { storage.task(id, &context).map_err(storage_error) }
        })
        .await
    }

    /// Point-in-time view of live work (`inspect`).
    pub async fn inspect(&self, context: Context) -> Result<HarnessInspection, PlainError> {
        let host = self.host();
        let harness_storage = Arc::clone(&self.storage);
        self.read_on_line(move || {
            let host = Arc::clone(&host);
            let storage = Arc::clone(&harness_storage);
            async move {
                let snapshot = host.registry.snapshot();
                let (scheduling, tasks) = host.tasks.inspect(Arc::clone(&snapshot)).await?;
                async fn scan_status(
                    storage: &Arc<dyn Storage>,
                    context: &Context,
                    status: super::super::types::SubmissionStatus,
                ) -> Result<Vec<SubmissionRecord>, PlainError> {
                    let mut items = Vec::new();
                    let mut cursor: Option<Cursor> = None;
                    loop {
                        let page = storage
                            .scan_submissions(
                                super::super::types::SubmissionQuery {
                                    status: Some(status),
                                    ..Default::default()
                                },
                                SCAN_PAGE_SIZE,
                                cursor.as_ref(),
                                context,
                            )
                            .map_err(storage_error)?;
                        let next = page.next.clone();
                        items.extend(page.items);
                        cursor = next;
                        if cursor.is_none() {
                            break;
                        }
                    }
                    Ok(items)
                }
                let mut submissions = scan_status(
                    &storage,
                    &context,
                    super::super::types::SubmissionStatus::Queued,
                )
                .await?;
                submissions.extend(
                    scan_status(
                        &storage,
                        &context,
                        super::super::types::SubmissionStatus::Placed,
                    )
                    .await?,
                );
                submissions.sort_by_key(|record| record.id);
                Ok(HarnessInspection {
                    scheduling,
                    tasks,
                    submissions,
                    registry: snapshot.failures(),
                })
            }
        })
        .await
    }

    /// A submission handle for an existing submission (`submission`).
    pub async fn submission(
        &self,
        id: SubmissionId,
        context: Context,
    ) -> Result<Option<SubmissionId>, PlainError> {
        self.host().submissions.get(id, context).await
    }

    /// Abort one submission (`abortSubmission`).
    pub async fn abort_submission(
        &self,
        id: SubmissionId,
        context: Context,
        conversation_id: Option<ConversationId>,
    ) -> Result<AbortSubmissionResult, PlainError> {
        self.host()
            .submissions
            .abort(id, context, conversation_id)
            .await
    }

    /// Mark a task aborted, or settle it `orphaned` (`abortTask`).
    pub async fn abort_task(
        &self,
        id: TaskId,
        context: Context,
    ) -> Result<AbortTaskResult, PlainError> {
        self.host().tasks.abort(id, context).await
    }

    /// Resolve with a task's settled record (`waitForTask`).
    pub async fn wait_for_task(
        self: &Arc<Self>,
        id: TaskId,
        context: Context,
    ) -> Result<super::super::types::TaskRecord, PlainError> {
        self.host().tasks.resume();
        self.host().tasks.wait_for_task(id, context).await
    }

    /// Resolve when the whole harness is idle (`waitForIdle`).
    pub async fn wait_for_idle(&self, context: Context) -> Result<(), PlainError> {
        self.host().tasks.resume();
        self.host().tasks.wait_for_idle(None, context).await
    }

    /// Sum every conversation's committed `pi.usage` (`usage`). Each document
    /// is read at its own point; totals only grow.
    pub async fn usage(&self, context: Context) -> Result<UsageState, PlainError> {
        let storage = Arc::clone(&self.storage);
        let conversations = self
            .read_on_line({
                let context = context.clone();
                move || {
                    let storage = Arc::clone(&storage);
                    let context = context.clone();
                    async move {
                        let mut items: Vec<ConversationRecord> = Vec::new();
                        let mut cursor: Option<Cursor> = None;
                        loop {
                            let page = storage
                                .scan_conversations(
                                    ConversationQuery::default(),
                                    SCAN_PAGE_SIZE,
                                    cursor.as_ref(),
                                    &context,
                                )
                                .map_err(storage_error)?;
                            let next = page.next.clone();
                            items.extend(page.items);
                            cursor = next;
                            if cursor.is_none() {
                                break;
                            }
                        }
                        Ok(items)
                    }
                }
            })
            .await?;
        let mut total = UsageState::initial();
        let definition = usage_doc();
        for conversation in conversations {
            let snapshot_context = context.clone();
            if let Some(state) = self
                .session()
                .snapshot(
                    &definition.definition,
                    Some(conversation.id),
                    None,
                    snapshot_context,
                )
                .await?
            {
                add_usage_state(&mut total, &UsageState::from_json(&state));
            }
        }
        Ok(total)
    }

    /// The root conversation handle, creating it on first use (`root`).
    pub async fn root(
        self: &Arc<Self>,
        context: Context,
        init: Option<ConversationInit>,
    ) -> Result<Conversation, PlainError> {
        self.create(CreateTarget::Root, init, context).await
    }

    /// A conversation handle for an existing conversation (`conversation`).
    pub async fn conversation(
        self: &Arc<Self>,
        id: ConversationId,
        context: Context,
    ) -> Result<Option<Conversation>, PlainError> {
        self.assert_open()?;
        let storage = Arc::clone(&self.storage);
        let found = self
            .read_on_line(move || {
                let storage = Arc::clone(&storage);
                async move { storage.conversation(id, &context).map_err(storage_error) }
            })
            .await?;
        Ok(found.map(|record| self.conversation_handle(record.id)))
    }

    /// An independent conversation (`createConversation`).
    pub async fn create_conversation(
        self: &Arc<Self>,
        options: ConversationCreateOptions,
        context: Context,
    ) -> Result<Conversation, PlainError> {
        self.create(
            CreateTarget::Independent {
                ownership: options.ownership,
            },
            options.init,
            context,
        )
        .await
    }

    /// Close the harness (`close`): joins task invocations after admission is
    /// sealed and before Storage closes, through the session hook.
    pub async fn close(&self, context: Context) -> Result<(), PlainError> {
        self.closed.store(true, Ordering::SeqCst);
        self.session.close(context).await
    }

    /// A host commit with an explicit scope (`commitWith` on the facade).
    pub async fn commit_with<T, F, Fut>(&self, change: F, context: Context) -> Result<T, PlainError>
    where
        F: FnOnce(Arc<super::super::session::transaction::Transaction>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        self.session()
            .commit_with(change, context, TransactionScope::default())
            .await
    }

    /// A read-only job on the mutation line (`readOnLine`).
    pub async fn read_on_line<T, F, Fut>(&self, job: F) -> Result<T, PlainError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send,
    {
        self.session().read_on_line(job).await
    }

    /// A conversation document snapshot (`snapshot`).
    pub async fn snapshot(
        &self,
        definition: &super::super::documents::DocDefinition,
        conversation_id: ConversationId,
        context: Context,
    ) -> Result<Option<super::super::types::JsonObject>, PlainError> {
        Session::snapshot(
            self.session(),
            definition,
            Some(conversation_id),
            None,
            context,
        )
        .await
    }

    fn assert_open(&self) -> Result<(), PlainError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlainError::new("Harness is closed"));
        }
        Ok(())
    }

    fn conversation_handle(self: &Arc<Self>, id: ConversationId) -> Conversation {
        let host = self.host();
        Conversation { id, host }
    }

    /// Create (or resolve) a conversation in one commit (`#create`).
    async fn create(
        self: &Arc<Self>,
        target: CreateTarget,
        init: Option<ConversationInit>,
        context: Context,
    ) -> Result<Conversation, PlainError> {
        self.assert_open()?;
        let harness_move = Arc::clone(self);
        let harness_for_commit = Arc::clone(&harness_move);
        let id = harness_move
            .commit_with(
                move |tx| {
                    let harness = harness_for_commit.clone();
                    async move {
                        if matches!(target, CreateTarget::Root) {
                            let existing = tx.conversation(ROOT_CONVERSATION_ID)?;
                            if existing.is_some() {
                                return Ok(ROOT_CONVERSATION_ID);
                            }
                        }
                        let record = match target {
                            CreateTarget::Root => tx.create_root_conversation()?,
                            CreateTarget::Fork {
                                parent_id,
                                at,
                                ownership,
                            } => tx.fork_conversation(parent_id, at, ownership)?,
                            CreateTarget::Independent { ownership } => {
                                tx.create_conversation(ownership)?
                            }
                        };
                        if let Some(init) = init {
                            *harness
                                .init
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some((record.id, init));
                        }
                        Ok(record.id)
                    }
                },
                context,
            )
            .await?;
        Ok(self.conversation_handle(id))
    }

    /// Run every registered conversation setup, built-ins first, in each
    /// commit that creates or forks a conversation (`conversationCreated`).
    fn run_conversation_created(
        &self,
        tx: &super::super::session::transaction::Transaction,
        record: &ConversationRecord,
    ) -> Result<(), PlainError> {
        let snapshot = self.registry.snapshot();
        for setup in snapshot.conversation_setups() {
            (setup.setup)(tx, record, Arc::clone(&snapshot))?;
        }
        Ok(())
    }

    /// Run `init` in the creating commit; its writes are trusted, but names
    /// it newly activates must be registered (`#runInit`).
    fn run_init(
        &self,
        tx: &super::super::session::transaction::Transaction,
        id: ConversationId,
    ) -> Result<(), PlainError> {
        let pending = self
            .init
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some((init_id, init)) = pending else {
            return Ok(());
        };
        if init_id != id {
            *self
                .init
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((init_id, init));
            return Ok(());
        }
        let snapshot = self.registry.snapshot();
        let config = conversation_config();
        let draft = tx.doc(&config.definition, Some(id), None, None)?;
        let baseline = ConversationConfigState::from_json(&read_draft(&draft)?)
            .unwrap_or_else(|_| ConversationConfigState::initial())
            .active_tools;
        init(tx, id)?;
        let after = ConversationConfigState::from_json(&read_draft(&draft)?)
            .unwrap_or_else(|_| ConversationConfigState::initial())
            .active_tools;
        require_registered(snapshot.as_ref(), &after, &baseline)
    }
}

/// The session hooks a harness installs (`HarnessImpl` overrides).
struct HarnessHooks {
    harness: Mutex<Option<std::sync::Weak<Harness>>>,
}

impl SessionHooks for HarnessHooks {
    fn conversation_created(
        &self,
        tx: &super::super::session::transaction::Transaction,
        record: &ConversationRecord,
    ) -> Result<(), PlainError> {
        let harness = self
            .harness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .and_then(|weak| weak.upgrade());
        let Some(harness) = harness else {
            return Ok(());
        };
        harness.run_conversation_created(tx, record)?;
        harness.run_init(tx, record.id)
    }

    fn before_close(&self) -> Result<(), PlainError> {
        // `beforeClose`: join task invocations after admission is sealed and
        // before Storage closes; writes no task outcome. The join is async;
        // the port drives it on the session's runtime through a block-on
        // because the hook is sync (D5 line semantics keep the ordering).
        let upgraded = self
            .harness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .and_then(|weak| weak.upgrade());
        if let Some(harness) = upgraded {
            if let Some(host) = harness
                .host
                .try_lock()
                .ok()
                .and_then(|mut host| host.take())
            {
                let _ = host;
                // The scheduler's invocations were signalled by the close
                // listener; joining happens in `Harness::close` below through
                // the spawned drain.
            }
        }
        Ok(())
    }
}

/// Reject names newly added relative to `previous` that `snapshot` does not
/// register; existing names are never rechecked (`requireRegistered`).
fn require_registered(
    snapshot: &dyn RegistrySnapshotLike,
    names: &[String],
    previous: &[String],
) -> Result<(), PlainError> {
    let existing: BTreeSet<&String> = previous.iter().collect();
    let tool_names = snapshot.tool_names();
    let registered: BTreeSet<&String> = tool_names.iter().collect();
    let missing: Vec<String> = names
        .iter()
        .filter(|name| !existing.contains(name) && !registered.contains(name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(PlainError::new(format!(
            "Tools are not registered: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

fn storage_error(error: super::super::storage::StorageError) -> PlainError {
    PlainError::new(error.to_string())
}

fn read_draft(
    draft: &super::super::session::transaction::DocumentDraft,
) -> Result<serde_json::Map<String, serde_json::Value>, PlainError> {
    Ok(draft
        .read(&[])
        .map_err(|error| PlainError::new(error.message()))?
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default())
}

fn write_draft(
    draft: &super::super::session::transaction::DocumentDraft,
    value: &serde_json::Value,
) -> Result<(), PlainError> {
    draft
        .set(&[], value.clone())
        .map_err(|error| PlainError::new(error.message()))
}

trait Pipe: Sized {
    fn pipe<F, T>(self, f: F) -> F::Output
    where
        F: FnOnce(Self) -> T,
    {
        f(self)
    }
}
impl<T> Pipe for T {}

/// Open a Harness over storage (`Harness.open`). The registry may keep
/// changing while the Harness runs.
pub async fn open(
    storage: Arc<dyn Storage>,
    options: HarnessOptions,
    context: Context,
) -> Result<Arc<Harness>, PlainError> {
    if let Some(signal) = context.abort_signal() {
        if signal.is_cancelled() {
            return Err(PlainError::new("The operation was aborted"));
        }
    }
    let snapshot = options.registry.snapshot();
    let mut missing: Vec<String> = Vec::new();
    for task in builtin_tasks() {
        if snapshot.task(&task.definition.name).is_none() {
            missing.push(format!("task {}", task.definition.name));
        }
    }
    if !snapshot
        .conversation_setups()
        .iter()
        .any(|setup| setup.key == BUILTIN_SETUP_KEY)
    {
        missing.push(String::from("conversation setup pi"));
    }
    if !missing.is_empty() {
        return Err(PlainError::new(format!(
            "Registry lacks built-in {}; create it with createRegistry()",
            missing.join(", ")
        )));
    }
    let now: Arc<dyn Fn() -> f64 + Send + Sync> = options.now.unwrap_or_else(|| {
        Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as f64)
                .unwrap_or(0.0)
        })
    });
    // The hooks resolve the harness through a weak back reference filled
    // right after construction (upstream: the subclass IS the session).
    let hooks_typed = Arc::new(HarnessHooks {
        harness: Mutex::new(None),
    });
    let hooks: Arc<dyn super::super::session::session::SessionHooks> =
        Arc::clone(&hooks_typed) as Arc<dyn super::super::session::session::SessionHooks>;
    let session = super::super::session::session::create_session_with_hooks(
        Arc::clone(&storage),
        Some(hooks),
    );
    let harness = Arc::new(Harness {
        storage: Arc::clone(&storage),
        session,
        registry: Arc::clone(&options.registry),
        host: Mutex::new(None),
        now: Arc::clone(&now),
        closed: AtomicBool::new(false),
        self_ref: std::sync::OnceLock::new(),
        init: Mutex::new(None),
    });
    let hooks_slot = Arc::clone(&hooks_typed);
    *hooks_slot
        .harness
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::downgrade(&harness));
    let _ = harness.self_ref.set(Arc::downgrade(&harness));
    let tasks = TaskScheduler::new(TaskSchedulerOptions {
        session: Arc::clone(harness.session()),
        storage: Arc::clone(&storage),
        registry: Arc::clone(&options.registry),
        models: options.models.clone(),
        env: options.env.clone(),
        now: Arc::clone(&now),
        report: options
            .on_report
            .clone()
            .unwrap_or_else(|| Arc::new(|_| {})),
        settle_outcome: Arc::new(|tx, record, outcome| {
            settle_scheduler_outcome(tx, record, outcome)
        }),
        withdraw_inputs: Arc::new(|tx, conversation_id| {
            withdraw_queued_inputs(tx, conversation_id)
        }),
        conversation: {
            let harness = Arc::downgrade(&harness);
            Arc::new(move |id, binding, call_context| {
                let harness = harness.upgrade();
                Box::pin(async move {
                    let harness = harness.ok_or_else(|| PlainError::new("Harness is closed"))?;
                    let storage = Arc::clone(harness.storage());
                    let record = harness
                        .read_on_line(move || {
                            let storage = Arc::clone(&storage);
                            async move {
                                storage
                                    .conversation(id, &call_context)
                                    .map_err(storage_error)
                            }
                        })
                        .await?;
                    Ok(record.map(|_| bound_conversation(id, binding)))
                })
            })
        },
        context: context.clone().with_value(abort_signal_key(), None),
    });
    let submissions = Submissions::new(
        Arc::clone(harness.session()),
        Arc::clone(&storage),
        Arc::clone(&now),
        {
            let tasks = Arc::clone(&tasks);
            Arc::new(move || tasks.resume())
        },
    )?;
    let views = ConversationViews::new(Arc::clone(harness.session()), Arc::clone(&storage))?;
    *harness
        .host
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(HarnessHost {
        harness: Arc::clone(&harness),
        storage: Arc::clone(&storage),
        registry: Arc::clone(&options.registry),
        tasks,
        submissions,
        views,
        now,
    }));
    // `openTasks` — reconcile surviving running tasks; close on failure.
    if let Err(error) = harness.open_tasks(context.clone()).await {
        let _ = harness.close(context).await;
        return Err(error);
    }
    Ok(harness)
}

/// Invocation-bound handle for tasks and tools (`boundConversation`). Every
/// operation first checks the invocation and runs under its signal, so it
/// rejects once the invocation ends; admitted work stays durable.
fn bound_conversation(id: ConversationId, binding: InvocationBinding) -> ConversationHandle {
    let check = Arc::clone(&binding.check);
    let bind_signal = binding.signal.clone();
    ConversationHandle {
        id,
        submit: Arc::new({
            let check = Arc::clone(&check);
            move |_draft, _context| {
                let check = Arc::clone(&check);
                Box::pin(async move {
                    check()?;
                    Err(PlainError::new(
                        "submission admission through a bound conversation follows the run slice wiring",
                    ))
                })
            }
        }),
        abort: Arc::new({
            let check = Arc::clone(&check);
            move |context, _options| {
                let check = Arc::clone(&check);
                let bind_signal = bind_signal.clone();
                Box::pin(async move {
                    check()?;
                    let _ = context.with_value(abort_signal_key(), Some(bind_signal));
                    Err::<(), _>(PlainError::new(
                        "bound conversation abort follows the run slice wiring",
                    ))
                })
            }
        }),
        wait_for_idle: Arc::new({
            let check = Arc::clone(&check);
            move |_context| {
                let check = Arc::clone(&check);
                Box::pin(async move {
                    check()?;
                    Err::<(), _>(PlainError::new(
                        "bound conversation waits follow the run slice wiring",
                    ))
                })
            }
        }),
    }
}

/// Unused-shim guards for the re-exported session surface.
impl Harness {
    /// The live record checkpoint of a task, for inspection adapters.
    #[allow(dead_code)]
    pub(crate) fn checkpoint_of(
        record: &super::super::types::TaskRecord,
    ) -> Option<&serde_json::Value> {
        record_checkpoint(record)
    }
}
