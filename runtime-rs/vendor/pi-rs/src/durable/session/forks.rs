//! Port of `src/session/forks.ts`: definition-free document copies selected by
//! one conversation fork.

use std::sync::Arc;

use crate::agent_core::chord_support::context::Context;

use super::super::documents::address_id;
use super::super::errors::PlainError;
use super::super::storage::Storage;
use super::super::types::{
    DocumentAddress, DocumentCopySource, DocumentCreate, DocumentFork, DocumentPoint,
    DocumentQuery, DocumentScope,
};

const SCAN_PAGE_SIZE: usize = 256;

/// One definition-free document copy to create with a forked conversation
/// (`forks.ts` `ForkDocumentCopy`).
#[derive(Debug, Clone)]
pub struct ForkDocumentCopy {
    pub record: DocumentCreate,
    pub source: DocumentCopySource,
}

/// Select every persisted conversation document copied by one fork
/// (`forks.ts` `prepareForkDocumentCopies`).
pub fn prepare_fork_document_copies(
    storage: &Arc<dyn Storage>,
    parent_conversation_id: i64,
    at: i64,
    child_conversation_id: i64,
    context: &Context,
) -> Result<Vec<ForkDocumentCopy>, PlainError> {
    let entry = storage
        .entry_visible(parent_conversation_id, at, context)
        .map_err(|error| PlainError::new(error.to_string()))?
        .ok_or_else(|| {
            PlainError::new(format!(
                "Entry {at} is not visible from conversation {parent_conversation_id}"
            ))
        })?;

    let mut copies: Vec<ForkDocumentCopy> = Vec::new();
    let mut copied_addresses: Vec<String> = Vec::new();
    collect_copies(
        storage,
        &DocumentScope::Conversation {
            conversation_id: entry.entry.conversation_id,
        },
        DocumentPoint::Seq(entry.commit_seq),
        DocumentFork::AsOf,
        child_conversation_id,
        &mut copies,
        &mut copied_addresses,
        context,
    )?;
    collect_copies(
        storage,
        &DocumentScope::Conversation {
            conversation_id: parent_conversation_id,
        },
        DocumentPoint::Current,
        DocumentFork::Current,
        child_conversation_id,
        &mut copies,
        &mut copied_addresses,
        context,
    )?;
    Ok(copies)
}

/// `collectCopies` (`forks.ts:51-82`).
#[allow(clippy::too_many_arguments)]
fn collect_copies(
    storage: &Arc<dyn Storage>,
    scope: &DocumentScope,
    at: DocumentPoint,
    policy: DocumentFork,
    child_conversation_id: i64,
    copies: &mut Vec<ForkDocumentCopy>,
    copied_addresses: &mut Vec<String>,
    context: &Context,
) -> Result<(), PlainError> {
    let mut cursor_state: Option<super::super::types::Cursor> = None;
    loop {
        let query = DocumentQuery {
            scope: *scope,
            at,
            kind: None,
        };
        let page = storage
            .scan_documents(query, SCAN_PAGE_SIZE, cursor_state.as_ref(), context)
            .map_err(|error| PlainError::new(error.to_string()))?;
        for source in &page.items {
            // Fork-aware conversation documents only, matching the policy
            // (`source.scope.kind !== "conversation" || source.fork !== policy`).
            let DocumentScope::Conversation { .. } = source.scope else {
                continue;
            };
            if source.fork != Some(policy) {
                continue;
            }
            let id = storage
                .mint_id()
                .map_err(|error| PlainError::new(error.to_string()))?;
            let record = DocumentCreate {
                id,
                kind: source.kind.clone(),
                key: source.key.clone(),
                scope: DocumentScope::Conversation {
                    conversation_id: child_conversation_id,
                },
                history: source.history,
                fork: source.fork,
            };
            let copy_address = address_id(&DocumentAddress {
                kind: record.kind.clone(),
                scope: record.scope,
                key: record.key.clone(),
            });
            if copied_addresses.contains(&copy_address) {
                let member = match &record.key {
                    Some(key) => format!("{}/{}", record.kind, key),
                    None => record.kind.clone(),
                };
                return Err(PlainError::new(format!(
                    "Fork selects multiple source documents for {member}"
                )));
            }
            copied_addresses.push(copy_address);
            copies.push(ForkDocumentCopy {
                record,
                source: DocumentCopySource { id: source.id, at },
            });
        }
        cursor_state = page.next;
        if cursor_state.is_none() {
            break;
        }
    }
    Ok(())
}
