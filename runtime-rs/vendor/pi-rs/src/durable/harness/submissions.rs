//! Port of `src/harness/submissions.ts`: admission, waits, and withdrawal of
//! the durable submissions of one Harness.
//!
//! Divergences (structural, disclosed): upstream `Submissions` is driven by
//! the promise-based commit/close listeners and `Waiters`; the port composes
//! the same commit listener over the Session surface with the util
//! [`Waiters`] (register-on-line, settle-off-line split, disclosed in the
//! scheduler module docs), and `SubmissionHandle` is a concrete struct
//! instead of a promise-returning object (D7 extension in
//! [`super::types`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{Message, UserMessage};

use super::super::entries::{user_entry, USER_ENTRY_ENTRY_KIND};
use super::super::errors::{ConversationBusy, PlainError};
use super::super::harness::inbox::{
    apply_boundary, is_stale, prepare_boundary, remove_inbox_item, At, InputMode,
};
use super::super::harness::live::live_doc;
use super::super::harness::types::{
    AbortResult, AbortSubmissionResult, SubmissionDraft, UserInput, WhenBusy,
};
use super::super::ids::{ConversationId, SubmissionId};
use super::super::session::session::Session;
use super::super::session::transaction::Transaction;
use super::super::storage::Storage;
use super::super::types::{
    EntryDraft, SubmissionRecord, SubmissionSettlement, SubmissionStatus, SubmissionType,
};
use super::super::util::{closed_error, Waiters};
use super::generation::start_run;

/// A settled submission record (`SettledSubmissionRecord`): the port keeps
/// the record as-is; every waiter resolves with one whose status is settled.
pub type SettledSubmissionRecord = SubmissionRecord;

/// Whether the record reached a terminal status (`isSettled`).
pub fn is_settled(record: &SubmissionRecord) -> bool {
    matches!(
        record.status,
        SubmissionStatus::Done | SubmissionStatus::Unanswered
    )
}

/// Admission, waits, and withdrawal of the durable submissions of one
/// Harness (`Submissions`).
pub struct Submissions {
    session: Arc<Session>,
    storage: Arc<dyn Storage>,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
    /// Enable task scheduling; submitting or waiting asks for progress.
    resume: Arc<dyn Fn() + Send + Sync>,
    waiters: Waiters<SubmissionRecord>,
    closed: AtomicBool,
}

impl Submissions {
    /// Wire the commit and close listeners (`constructor`).
    pub fn new(
        session: Arc<Session>,
        storage: Arc<dyn Storage>,
        now: Arc<dyn Fn() -> f64 + Send + Sync>,
        resume: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Arc<Submissions>, PlainError> {
        let submissions = Arc::new(Submissions {
            session,
            storage,
            now,
            resume,
            waiters: Waiters::new(),
            closed: AtomicBool::new(false),
        });
        let observer = Arc::downgrade(&submissions);
        submissions
            .session
            .subscribe_commits(Arc::new(move |publication, _context| {
                if let Some(submissions) = observer.upgrade() {
                    submissions.observe(publication);
                }
            }))?;
        let closer = Arc::downgrade(&submissions);
        submissions.session.subscribe_close(Arc::new(move || {
            if let Some(submissions) = closer.upgrade() {
                submissions.closed.store(true, Ordering::SeqCst);
                submissions.waiters.reject_all();
            }
        }))?;
        Ok(submissions)
    }

    /// Admit a submission in one commit (spec §6, `submit`). A known request
    /// ID returns its existing submission without writing. A busy
    /// conversation queues it in `pi.inbox`, or rejects `whenBusy: "reject"`
    /// input with `ConversationBusy` without writing. An idle conversation
    /// with queued items queues it behind them and runs a final boundary.
    /// Otherwise idle input places a user entry and starts a run, and an
    /// idle write appends its entry and settles `done`.
    pub async fn submit(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        draft: SubmissionDraft,
        context: Context,
    ) -> Result<SubmissionId, PlainError> {
        (self.resume)();
        let submissions = Arc::clone(self);
        let id = self
            .session
            .commit(
                move |tx| {
                    let submissions = submissions.clone();
                    async move {
                        submissions
                            .submit_commit(tx.as_ref(), conversation_id, draft)
                            .await
                    }
                },
                context,
            )
            .await?;
        Ok(id)
    }

    /// The submit commit body (`submit`'s `commitWith` callback).
    async fn submit_commit(
        self: Arc<Self>,
        tx: &Transaction,
        conversation_id: ConversationId,
        draft: SubmissionDraft,
    ) -> Result<SubmissionId, PlainError> {
        let draft_type = draft.submission_type();
        if let Some(request_id) = draft.request_id() {
            if let Some(existing) = tx.submission_by_request(conversation_id, request_id)? {
                if existing.r#type != draft_type {
                    return Err(PlainError::new(format!(
                        "Request {request_id} already identifies a submission of type {}",
                        if existing.r#type == SubmissionType::Input {
                            "input"
                        } else {
                            "write"
                        }
                    )));
                }
                return Ok(existing.id);
            }
        }
        let live = live_doc();
        let live_draft = tx.doc(&live.definition, Some(conversation_id), None, None)?;
        let live_value = super::super::harness::live::read_live(&live_draft)?;
        let busy = live_value
            .and_then(|value| value.get("run").cloned())
            .map(|run| !run.is_null())
            .unwrap_or(false);
        if busy
            && draft_type == SubmissionType::Input
            && matches!(
                draft,
                SubmissionDraft::Input {
                    when_busy: Some(WhenBusy::Reject),
                    ..
                }
            )
        {
            return Err(PlainError::new(
                ConversationBusy::new(conversation_id).to_string(),
            ));
        }
        let request_id = draft.request_id().map(str::to_owned);
        // A boundary reads the table, so it is prepared before the first
        // table write; a busy one needs none.
        let boundary = if busy {
            None
        } else {
            Some(prepare_boundary(tx, conversation_id)?)
        };
        let boundary_empty = boundary
            .as_ref()
            .map(|boundary| {
                super::super::harness::inbox::items_of(boundary)
                    .map(|items| items.is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if boundary.is_none() || !boundary_empty {
            let queued =
                SubmissionRecord::queued(conversation_id, request_id.clone(), draft_type, 0);
            let created = tx.create_submission(queued)?;
            // Hosts may leave optional fields `undefined`; drafts take strict
            // JSON.
            let value = match &draft {
                SubmissionDraft::Write { entry, .. } => serde_json::to_value(entry)
                    .map_err(|error| PlainError::new(error.to_string()))?,
                SubmissionDraft::Input { content, .. } => serde_json::to_value(content)
                    .map_err(|error| PlainError::new(error.to_string()))?,
            };
            let inbox_items = match &boundary {
                Some(boundary) => super::super::harness::inbox::items_of(boundary)?,
                None => {
                    let inbox = super::super::harness::inbox::inbox_doc();
                    let draft_doc = tx.doc(&inbox.definition, Some(conversation_id), None, None)?;
                    super::super::harness::inbox::read_items(&draft_doc)?
                }
            };
            let mut items = inbox_items;
            match &draft {
                SubmissionDraft::Write { .. } => {
                    items.push(super::super::harness::inbox::InboxItem::Write {
                        id: created.id,
                        entry: value.as_object().cloned().unwrap_or_default(),
                    });
                }
                SubmissionDraft::Input { when_busy, .. } => {
                    let mode = match when_busy {
                        Some(WhenBusy::Steer) => InputMode::Steer,
                        _ => InputMode::FollowUp,
                    };
                    items.push(super::super::harness::inbox::InboxItem::Input {
                        id: created.id,
                        mode,
                        content: value,
                    });
                }
            }
            write_inbox_items(tx, conversation_id, items)?;
            if let Some(mut boundary) = boundary {
                let now = (self.now)();
                let placed = apply_boundary(tx, &mut boundary, At::Final, now)?;
                if !placed.users.is_empty() {
                    let live_draft = tx.doc(&live.definition, Some(conversation_id), None, None)?;
                    start_run(tx, conversation_id, &live_draft, placed.users)?;
                }
            }
            return Ok(created.id);
        }
        let boundary = boundary.expect("idle boundary above");
        if draft_type == SubmissionType::Write {
            let SubmissionDraft::Write { entry, .. } = &draft else {
                unreachable!("write branch");
            };
            let entry_draft: EntryDraft = entry.clone();
            if is_stale(&boundary, &entry_draft) {
                let created = tx.create_submission(SubmissionRecord {
                    status: SubmissionStatus::Unanswered,
                    reason: Some(String::from("stale")),
                    ..SubmissionRecord::queued(
                        conversation_id,
                        request_id,
                        SubmissionType::Write,
                        0,
                    )
                })?;
                return Ok(created.id);
            }
            let entry = tx.append_entry(conversation_id, entry_draft)?;
            let created = tx.create_submission(SubmissionRecord::settled_direct(
                conversation_id,
                request_id,
                SubmissionType::Write,
                SubmissionStatus::Done,
                entry.id,
                0,
            ))?;
            return Ok(created.id);
        }
        let SubmissionDraft::Input { content, .. } = &draft else {
            unreachable!("input branch");
        };
        let content: UserInput = serde_json::from_value(
            serde_json::to_value(content).map_err(|error| PlainError::new(error.to_string()))?,
        )
        .map_err(|error| PlainError::new(error.to_string()))?;
        let now = (self.now)();
        let mut entry_draft = EntryDraft::new(USER_ENTRY_ENTRY_KIND);
        entry_draft.model = Some(vec![Message::User(UserMessage {
            content,
            timestamp: now as i64,
        })]);
        let entry = tx.append_entry(conversation_id, entry_draft)?;
        let created = tx.create_submission(SubmissionRecord::settled_direct(
            conversation_id,
            request_id,
            SubmissionType::Input,
            SubmissionStatus::Placed,
            entry.id,
            0,
        ))?;
        start_run(tx, conversation_id, &live_draft, vec![created.id])?;
        Ok(created.id)
    }

    /// Handle for an existing submission, or `None` (`get`).
    pub async fn get(
        &self,
        id: SubmissionId,
        context: Context,
    ) -> Result<Option<SubmissionId>, PlainError> {
        let found = self
            .session
            .read_on_line(|| {
                let storage = Arc::clone(&self.storage);
                let context = context.clone();
                async move { storage.submission(id, &context).map_err(storage_error) }
            })
            .await?;
        Ok(found.map(|record| record.id))
    }

    /// The committed record (`status`).
    pub async fn status(
        &self,
        id: SubmissionId,
        context: Context,
    ) -> Result<SubmissionRecord, PlainError> {
        let found = self
            .session
            .read_on_line(|| {
                let storage = Arc::clone(&self.storage);
                let context = context.clone();
                async move { storage.submission(id, &context).map_err(storage_error) }
            })
            .await?;
        found.ok_or_else(|| PlainError::new(format!("Submission {id} does not exist")))
    }

    /// Resolve with the settled record, registering on the line so no
    /// settling publication falls between the check and the registration
    /// (`wait`).
    pub async fn wait(
        self: &Arc<Self>,
        id: SubmissionId,
        context: Context,
    ) -> Result<SettledSubmissionRecord, PlainError> {
        (self.resume)();
        enum Found {
            Ready(SubmissionRecord),
            Wait(super::super::util::WaiterHandle<SubmissionRecord>),
        }
        let submissions = Arc::clone(self);
        let found = self
            .session
            .read_on_line(|| {
                let submissions = submissions.clone();
                let context = context.clone();
                async move {
                    let record = submissions
                        .storage
                        .submission(id, &context)
                        .map_err(storage_error)?;
                    let Some(record) = record else {
                        return Err(PlainError::new(format!("Submission {id} does not exist")));
                    };
                    if is_settled(&record) {
                        return Ok(Found::Ready(record));
                    }
                    // Close rejects registered waiters synchronously and may
                    // begin during the read.
                    if submissions.closed.load(Ordering::SeqCst) {
                        return Err(PlainError::new(closed_error().to_string()));
                    }
                    Ok(Found::Wait(
                        submissions.waiters.register(&id.to_string(), &context)?,
                    ))
                }
            })
            .await?;
        match found {
            Found::Ready(record) => Ok(record),
            Found::Wait(waiter) => waiter
                .wait()
                .await
                .map_err(|_| PlainError::new(closed_error().to_string())),
        }
    }

    /// Withdraw a queued submission and remove its inbox item; placed inputs
    /// and settled submissions are reported (`abort`). With
    /// `conversation_id`, a submission of another conversation reports
    /// `not_found`.
    pub async fn abort(
        self: &Arc<Self>,
        id: SubmissionId,
        context: Context,
        conversation_id: Option<ConversationId>,
    ) -> Result<AbortSubmissionResult, PlainError> {
        let submissions = Arc::clone(self);
        let result = self
            .session
            .commit(
                move |tx| {
                    let submissions = submissions.clone();
                    async move { submissions.abort_commit(tx.as_ref(), id, conversation_id) }
                },
                context,
            )
            .await?;
        Ok(result)
    }

    fn abort_commit(
        &self,
        tx: &Transaction,
        id: SubmissionId,
        conversation_id: Option<ConversationId>,
    ) -> Result<AbortSubmissionResult, PlainError> {
        let record = tx.submission(id)?;
        let Some(record) = record else {
            return Ok(AbortSubmissionResult::NotFound);
        };
        if let Some(conversation_id) = conversation_id {
            if record.conversation_id != conversation_id {
                return Ok(AbortSubmissionResult::NotFound);
            }
        }
        if record.status == SubmissionStatus::Queued {
            tx.settle_submission(
                id,
                SubmissionSettlement::Unanswered {
                    reason: String::from("aborted"),
                    detail: None,
                },
            )?;
            remove_inbox_item(tx, record.conversation_id, id)?;
            return Ok(AbortSubmissionResult::Found(AbortResult::Aborted));
        }
        Ok(AbortSubmissionResult::Found(match record.status {
            SubmissionStatus::Placed => AbortResult::AlreadyPlaced,
            _ => AbortResult::Settled,
        }))
    }

    /// The commit listener (`#observe`): resolve every settled record's
    /// waiters.
    fn observe(&self, publication: &super::super::types::CommitPublication) {
        for change in &publication.changes {
            let super::super::types::CommitChange::Table(
                super::super::types::TableCommitChange::Submission { value },
            ) = change
            else {
                continue;
            };
            if is_settled(value) {
                self.waiters.resolve(&value.id.to_string(), value.clone());
            }
        }
    }
}

fn storage_error(error: super::super::storage::StorageError) -> PlainError {
    PlainError::new(error.to_string())
}

/// Rewrite the inbox items of one conversation through its draft document.
fn write_inbox_items(
    tx: &Transaction,
    conversation_id: ConversationId,
    items: Vec<super::super::harness::inbox::InboxItem>,
) -> Result<(), PlainError> {
    let inbox = super::super::harness::inbox::inbox_doc();
    let draft = tx.doc(&inbox.definition, Some(conversation_id), None, None)?;
    let mut value = Map::new();
    value.insert(
        String::from("items"),
        Value::Array(
            items
                .iter()
                .map(super::super::harness::inbox::InboxItem::to_json)
                .collect(),
        ),
    );
    draft
        .set(
            &[crate::chord::delta::Seg::Key(String::from("items"))],
            Value::Object(value),
        )
        .map_err(|error| PlainError::new(error.message()))
}

/// The user entry guard re-export for the placement path.
#[allow(unused_imports)]
use user_entry as _user_entry;
