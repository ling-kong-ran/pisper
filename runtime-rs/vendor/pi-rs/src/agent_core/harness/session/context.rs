//! Port of `packages/agent/src/harness/session/context.ts` (65 lines): the
//! session-to-model-context projection. [`build_context_entries`] narrows a
//! branch path to the latest compaction checkpoint plus everything after it,
//! [`session_entry_to_context_messages`] projects one entry to its
//! model-visible messages (filtering error/aborted/deferred assistant
//! responses), and [`build_session_context`] additionally routes `custom`
//! entries through application-supplied projectors.
//!
//! Disclosed substitutions:
//! - Upstream `EntryProjector` (`context.ts` via `types.ts:59-62`) returns
//!   `AgentMessage[] | undefined | Promise<...>`; the port folds the sync and
//!   async branches into one boxed future and `undefined` into an empty vec.
//!   It receives the whole [`Entry`] (upstream `CustomEntry`): the port's
//!   closed enum has no separate custom-entry struct, and projectors are only
//!   ever invoked for [`Entry::Custom`].
//! - Projector failures are the `Err` channel (upstream thrown errors reject
//!   the `buildSessionContext` promise and propagate to the caller).

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::messages::{
    create_branch_summary_message, create_compaction_summary_message,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::primitives::StopReason;

use super::types::Entry;

/// Upstream `EntryProjector` (`session/types.ts:59-62`): convert an
/// application-defined custom entry into model context. Only invoked for
/// [`Entry::Custom`]; an empty vec is upstream `undefined`.
pub type EntryProjector = Arc<
    dyn Fn(Entry, Context) -> BoxFuture<'static, anyhow::Result<Vec<AgentMessage>>> + Send + Sync,
>;

/// Upstream `SessionContextBuildOptions` (`context.ts:6-8`).
#[derive(Clone, Default)]
pub struct SessionContextBuildOptions {
    /// Projectors keyed by the custom entry's `customType`.
    pub entry_projectors: Option<BTreeMap<String, EntryProjector>>,
}

/// Upstream `buildContextEntries` (`context.ts:10-22`): the latest compaction
/// checkpoint plus every entry after it, or a copy of the whole path when
/// there is no compaction.
pub fn build_context_entries(path_entries: &[Entry]) -> Vec<Entry> {
    let mut latest: Option<(usize, &Entry)> = None;
    for (index, entry) in path_entries.iter().enumerate().rev() {
        if matches!(entry, Entry::Compaction { .. }) {
            latest = Some((index, entry));
            break;
        }
    }
    match latest {
        None => path_entries.to_vec(),
        Some((index, compaction)) => {
            let mut entries = Vec::with_capacity(1 + path_entries.len() - index - 1);
            entries.push(compaction.clone());
            entries.extend(path_entries[index + 1..].iter().cloned());
            entries
        }
    }
}

/// Upstream `isContextMessage` (`context.ts:24-29`): assistant responses that
/// settled as error/aborted/deferred never reach model context.
fn is_context_message(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::Assistant(assistant) => !matches!(
            assistant.stop_reason,
            StopReason::Error | StopReason::Aborted | StopReason::Deferred
        ),
        _ => true,
    }
}

/// Upstream `sessionEntryToContextMessages` (`context.ts:31-45`).
pub fn session_entry_to_context_messages(entry: &Entry) -> Vec<AgentMessage> {
    match entry {
        Entry::Message { message, .. } => is_context_message(message)
            .then(|| message.clone())
            .into_iter()
            .collect(),
        Entry::Compaction {
            summary,
            tokens_before,
            timestamp,
            retained_tail,
            ..
        } => {
            let mut messages = Vec::with_capacity(1 + retained_tail.len());
            messages.push(AgentMessage::Custom(
                create_compaction_summary_message(summary.clone(), *tokens_before, *timestamp)
                    .to_custom(),
            ));
            messages.extend(
                retained_tail
                    .iter()
                    .filter(|message| is_context_message(message))
                    .cloned(),
            );
            messages
        }
        Entry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } => {
            if summary.is_empty() {
                Vec::new()
            } else {
                vec![AgentMessage::Custom(
                    create_branch_summary_message(summary.clone(), from_id.clone(), *timestamp)
                        .to_custom(),
                )]
            }
        }
        Entry::Custom { .. } => Vec::new(),
    }
}

/// Upstream `buildSessionContext` (`context.ts:47-64`): the model-visible
/// message list for a branch path, with custom entries projected through the
/// registered projectors in branch order. Projector failures propagate
/// (upstream the promise rejects).
pub async fn build_session_context(
    path_entries: &[Entry],
    options: Option<&SessionContextBuildOptions>,
    context: Context,
) -> anyhow::Result<Vec<AgentMessage>> {
    let default_options = SessionContextBuildOptions::default();
    let options = options.unwrap_or(&default_options);
    let entries = build_context_entries(path_entries);
    let mut messages: Vec<AgentMessage> = Vec::new();
    for entry in &entries {
        match entry {
            Entry::Custom { custom_type, .. } => {
                let projector = options
                    .entry_projectors
                    .as_ref()
                    .and_then(|projectors| projectors.get(custom_type));
                if let Some(projector) = projector {
                    let projected = projector(entry.clone(), context.clone()).await?;
                    messages.extend(projected);
                }
            }
            _ => messages.extend(session_entry_to_context_messages(entry)),
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests;
