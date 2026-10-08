//! Port of `src/errors.ts`: the error taxonomy shared by the Session, its
//! storage backends, and the submission boundary. `Display` reproduces the
//! upstream `error.message` text byte-for-byte; `name` maps to the concrete
//! type.

use std::fmt;

use super::ids::ConversationId;

/// A transaction read a table after its first table write. Read every
/// required row before writing (upstream `ReadAfterWrite`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAfterWrite {
    pub method: String,
}

impl ReadAfterWrite {
    /// Upstream message: `` `Tx.${method}() cannot read tables after the first table write` ``.
    pub fn new(method: impl Into<String>) -> Self {
        ReadAfterWrite {
            method: method.into(),
        }
    }
}

impl fmt::Display for ReadAfterWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tx.{}() cannot read tables after the first table write",
            self.method
        )
    }
}

impl std::error::Error for ReadAfterWrite {}

/// Storage rejected a batch before any durable effect; the owning Session may
/// continue safely (upstream `StorageRejected`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageRejected {
    pub message: String,
}

impl StorageRejected {
    pub fn new(message: impl Into<String>) -> Self {
        StorageRejected {
            message: message.into(),
        }
    }
}

impl fmt::Display for StorageRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StorageRejected {}

/// A submission reached a busy conversation and was not admitted (upstream
/// `ConversationBusy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationBusy {
    pub conversation_id: ConversationId,
}

impl ConversationBusy {
    /// Upstream message: `` `Conversation ${conversationId} is busy` ``.
    pub fn new(conversation_id: ConversationId) -> Self {
        ConversationBusy { conversation_id }
    }
}

impl fmt::Display for ConversationBusy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Conversation {} is busy", self.conversation_id)
    }
}

impl std::error::Error for ConversationBusy {}

/// A plain upstream `new Error(message)` throw (session, transaction, and
/// harness call sites); `Display` is the exact `error.message` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainError {
    pub message: String,
}

impl PlainError {
    pub fn new(message: impl Into<String>) -> Self {
        PlainError {
            message: message.into(),
        }
    }
}

impl fmt::Display for PlainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlainError {}
