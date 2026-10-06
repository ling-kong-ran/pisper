//! Port of `src/harness/inbox.ts`: the built-in `pi.inbox` queue of one
//! conversation's submissions waiting for a boundary, and the boundary
//! placement rules of spec §6.
//!
//! Divergences (structural, disclosed): upstream mutates the `Draft<InboxState>`
//! document value through typed JS objects; the port mutates the same JSON
//! (`{items: [...]}`) through `serde_json` maps, with item wire order
//! `{id, mode, content? | entry?}` per the `inbox.ts` construction sites.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::super::documents::{define_doc, DefinitionScope, DocToken};
use super::super::errors::PlainError;
use super::super::ids::{ConversationId, EntryId, SubmissionId};
use super::super::session::transaction::Transaction;
use super::super::types::{
    DocumentFork, DocumentHistory, EntryDraft, EntryHead, JsonObject, SubmissionSettlement,
};
use super::config::{conversation_config, ConversationConfigState};
use super::types::{QueueMode, UserInput};

/// A queued submission (`inbox.ts` `InboxItem`): user input for a run, or a
/// passive entry write. `entry` is an `EntryDraft`, stored as plain JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxItem {
    Input {
        id: SubmissionId,
        mode: InputMode,
        content: Value,
    },
    Write {
        id: SubmissionId,
        entry: JsonObject,
    },
}

/// `steer` or `followUp` (`inbox.ts` `InboxItem` input mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Steer,
    FollowUp,
}

impl InputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputMode::Steer => "steer",
            InputMode::FollowUp => "followUp",
        }
    }

    fn from_str(text: &str) -> Option<Self> {
        match text {
            "steer" => Some(InputMode::Steer),
            "followUp" => Some(InputMode::FollowUp),
            _ => None,
        }
    }
}

impl InboxItem {
    pub fn id(&self) -> SubmissionId {
        match self {
            InboxItem::Input { id, .. } | InboxItem::Write { id, .. } => *id,
        }
    }

    /// Parse one wire item.
    pub fn from_json(value: &Value) -> Option<InboxItem> {
        let object = value.as_object()?;
        let id = object.get("id").and_then(Value::as_i64)?;
        let mode = object.get("mode").and_then(Value::as_str)?;
        match mode {
            "steer" | "followUp" => Some(InboxItem::Input {
                id,
                mode: InputMode::from_str(mode)?,
                content: object.get("content").cloned()?,
            }),
            "write" => Some(InboxItem::Write {
                id,
                entry: object.get("entry").and_then(Value::as_object).cloned()?,
            }),
            _ => None,
        }
    }

    /// Serialize in the upstream construction order.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        match self {
            InboxItem::Input { id, mode, content } => {
                map.insert(String::from("id"), Value::from(*id));
                map.insert(String::from("mode"), Value::from(mode.as_str()));
                map.insert(String::from("content"), content.clone());
            }
            InboxItem::Write { id, entry } => {
                map.insert(String::from("id"), Value::from(*id));
                map.insert(String::from("mode"), Value::from("write"));
                map.insert(String::from("entry"), Value::Object(entry.clone()));
            }
        }
        Value::Object(map)
    }
}

/// Built-in queue of one conversation's submissions (`inbox.ts` `InboxState`).
pub fn inbox_doc() -> DocToken {
    define_doc(super::super::documents::DocDefinition {
        kind: String::from("pi.inbox"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Initial),
        family: false,
        initial: Arc::new(|_| {
            let mut map = Map::new();
            map.insert(String::from("items"), Value::Array(Vec::new()));
            map
        }),
        migrate: None,
        checkpoint_when: Some(Arc::new(|value, _, _| {
            value
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| items.is_empty())
        })),
    })
    .expect("the built-in inbox document definition is valid")
}

/// What a boundary reads before the commit's first table write (`inbox.ts`
/// `Boundary`), and the newest head it has seen so far.
pub struct Boundary {
    pub conversation_id: ConversationId,
    /// The inbox document draft, mutated in place by [`apply_boundary`].
    pub inbox: super::super::session::transaction::DocumentDraft,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    /// Start of the active range, the newest head marker's `head`; advanced
    /// by heads written in this commit.
    pub head: Option<EntryId>,
}

/// Selected user items, in ID order, and whether a `head: "self"` write (a
/// reset) was placed (`inbox.ts` `BoundaryResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryResult {
    pub users: Vec<SubmissionId>,
    pub reset: bool,
}

/// The inbox items of a boundary's draft document.
pub fn items_of(boundary: &Boundary) -> Result<Vec<InboxItem>, PlainError> {
    read_items(&boundary.inbox)
}

/// Read `{items: [...]}` out of an inbox draft.
pub fn read_items(
    draft: &super::super::session::transaction::DocumentDraft,
) -> Result<Vec<InboxItem>, PlainError> {
    let value = draft
        .read(&[String::from("items").into()])
        .map_err(|error| PlainError::new(error.message().to_string()))?
        .unwrap_or_else(|| Value::Array(Vec::new()));
    Ok(value
        .as_array()
        .map(|items| items.iter().filter_map(InboxItem::from_json).collect())
        .unwrap_or_default())
}

/// Write the items back to an inbox draft.
fn write_items(
    draft: &super::super::session::transaction::DocumentDraft,
    items: Vec<InboxItem>,
) -> Result<(), PlainError> {
    draft
        .set(
            &[String::from("items").into()],
            Value::Array(items.iter().map(InboxItem::to_json).collect()),
        )
        .map_err(|error| PlainError::new(error.message().to_string()))
}

/// Read what a boundary needs (`inbox.ts` `prepareBoundary`). Table reads
/// must precede the commit's first table write, so callers prepare the
/// boundary at the start of their commit.
pub fn prepare_boundary(
    tx: &Transaction,
    conversation_id: ConversationId,
) -> Result<Boundary, PlainError> {
    let head = tx
        .latest_head_marker(conversation_id)?
        .and_then(|marker| marker.head);
    let inbox = inbox_doc();
    let draft = tx.doc(&inbox.definition, Some(conversation_id), None, None)?;
    let config = conversation_config();
    let config_value = tx.doc(&config.definition, Some(conversation_id), None, None)?;
    let config_state = config_value
        .read(&[])
        .map_err(|error| PlainError::new(error.message().to_string()))?
        .and_then(|value| value.as_object().cloned())
        .map(|object| ConversationConfigState::from_json(&object))
        .transpose()?
        .unwrap_or_else(ConversationConfigState::initial);
    Ok(Boundary {
        conversation_id,
        inbox: draft,
        steering_mode: config_state.steering_mode.unwrap_or(QueueMode::OneAtATime),
        follow_up_mode: config_state.follow_up_mode.unwrap_or(QueueMode::OneAtATime),
        head,
    })
}

/// Place the queued items a boundary selects (`inbox.ts` `applyBoundary`,
/// spec §6): every write, the first or all steers, and at `final` the first
/// or all follow-ups. A selected reset turns a `postTools` boundary into
/// `final`. Writes are placed first and user items after them, each in ID
/// order, so user items queued before a reset run in the new context. A
/// write whose head targets an entry before the active range, including a
/// range started earlier in this commit, is stale. Selected and stale items
/// are removed positionally.
pub fn apply_boundary(
    tx: &Transaction,
    boundary: &mut Boundary,
    at: At,
    now: f64,
) -> Result<BoundaryResult, PlainError> {
    let conversation_id = boundary.conversation_id;
    let mut items = read_items(&boundary.inbox)?;
    let reset = items.iter().any(|item| match item {
        InboxItem::Write { entry, .. } => entry
            .get("head")
            .map(|head| head == &Value::from("self"))
            .unwrap_or(false),
        _ => false,
    });
    let final_boundary = matches!(at, At::Final) || reset;
    let pick = |mode: InputMode, queue_mode: QueueMode| -> Vec<usize> {
        let indexes: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, InboxItem::Input { mode: item_mode, .. } if *item_mode == mode))
            .map(|(index, _)| index)
            .collect();
        if queue_mode == QueueMode::All {
            indexes
        } else {
            indexes.into_iter().take(1).collect()
        }
    };
    let writes: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, InboxItem::Write { .. }))
        .map(|(index, _)| index)
        .collect();
    let mut users: Vec<usize> = pick(InputMode::Steer, boundary.steering_mode);
    if final_boundary {
        users.extend(pick(InputMode::FollowUp, boundary.follow_up_mode));
    }
    users.sort_unstable();

    // `appendEntry()` copies the drafts' values; the items are removed only
    // afterwards.
    for index in &writes {
        let entry = match &items[*index] {
            InboxItem::Write { entry, .. } => entry.clone(),
            _ => continue,
        };
        let draft = entry_to_draft(&entry)?;
        if is_stale(boundary, &draft) {
            tx.settle_submission(
                items[*index].id(),
                SubmissionSettlement::Unanswered {
                    reason: String::from("stale"),
                    detail: None,
                },
            )?;
            continue;
        }
        let entry = tx.append_entry(conversation_id, draft)?;
        // `boundary.head = draft.head === "self" ? entry.id : draft.head`
        // (`inbox.ts:91`); `Tx#appendEntry` resolved "self" to the entry ID.
        if entry.head.is_some() {
            boundary.head = entry.head;
        }
        tx.place_submission(items[*index].id(), entry.id)?;
    }
    let mut placed: Vec<SubmissionId> = Vec::new();
    for index in &users {
        let (id, content) = match &items[*index] {
            InboxItem::Input { id, content, .. } => (*id, content.clone()),
            _ => continue,
        };
        let user_input: UserInput =
            serde_json::from_value(content).map_err(|error| PlainError::new(error.to_string()))?;
        let mut draft = EntryDraft::new(super::super::entries::USER_ENTRY_ENTRY_KIND);
        draft.model = Some(vec![crate::ai::types::Message::User(
            crate::ai::types::UserMessage {
                content: user_input,
                timestamp: now as i64,
            },
        )]);
        let entry = tx.append_entry(conversation_id, draft)?;
        tx.place_submission(id, entry.id)?;
        placed.push(id);
    }
    let mut removed: Vec<usize> = writes
        .iter()
        .copied()
        .chain(users.iter().copied())
        .collect();
    removed.sort_unstable_by(|left, right| right.cmp(left));
    for index in removed {
        if index < items.len() {
            items.remove(index);
        }
    }
    write_items(&boundary.inbox, items)?;
    Ok(BoundaryResult {
        users: placed,
        reset,
    })
}

/// Which boundary [`apply_boundary`] places at (`inbox.ts`
/// `"postTools" | "final"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    PostTools,
    Final,
}

/// The entry draft stored as plain JSON in an inbox write item.
fn entry_to_draft(entry: &JsonObject) -> Result<EntryDraft, PlainError> {
    serde_json::from_value(Value::Object(entry.clone()))
        .map_err(|error| PlainError::new(error.to_string()))
}

/// Whether a head write targets an entry before the active range (`inbox.ts`
/// `isStale`), so placing it would bring back cut history.
pub fn is_stale(boundary: &Boundary, entry: &EntryDraft) -> bool {
    match entry.head {
        Some(EntryHead::Id(head)) => boundary
            .head
            .is_some_and(|boundary_head| head < boundary_head),
        _ => false,
    }
}

/// Remove a withdrawn submission's item (`inbox.ts` `removeInboxItem`); the
/// caller settles the submission.
pub fn remove_inbox_item(
    tx: &Transaction,
    conversation_id: ConversationId,
    id: SubmissionId,
) -> Result<(), PlainError> {
    let inbox = inbox_doc();
    let draft = tx.doc(&inbox.definition, Some(conversation_id), None, None)?;
    let mut items = read_items(&draft)?;
    if let Some(index) = items.iter().position(|item| item.id() == id) {
        items.remove(index);
    }
    write_items(&draft, items)
}

/// Withdraw every queued input of a conversation (`inbox.ts`
/// `withdrawQueuedInputs`), as `Conversation.abort()` and abort cascades do:
/// each settles `unanswered` with `aborted` and leaves the inbox; queued
/// writes stay for later placement.
pub fn withdraw_queued_inputs(
    tx: &Transaction,
    conversation_id: ConversationId,
) -> Result<(), PlainError> {
    let inbox = inbox_doc();
    let draft = tx.doc(&inbox.definition, Some(conversation_id), None, None)?;
    let mut items = read_items(&draft)?;
    for index in (0..items.len()).rev() {
        let item = items[index].clone();
        if matches!(item, InboxItem::Write { .. }) {
            continue;
        }
        tx.settle_submission(
            item.id(),
            SubmissionSettlement::Unanswered {
                reason: String::from("aborted"),
                detail: None,
            },
        )?;
        items.remove(index);
    }
    write_items(&draft, items)
}

/// Queued-record constructor used by `submissions.ts` (`inbox.ts` through
/// `SubmissionRecord`); item JSON for one queued input.
pub fn input_item_json(id: SubmissionId, mode: InputMode, content: Value) -> Value {
    InboxItem::Input { id, mode, content }.to_json()
}
