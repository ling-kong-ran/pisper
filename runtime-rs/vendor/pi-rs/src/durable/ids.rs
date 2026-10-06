//! Port of `src/ids.ts` plus the branded ID/sequence aliases declared in
//! `src/types.ts` (see the module-level divergence note D1).
//!
//! Upstream:
//!
//! ```ts
//! export function idFromNumber<I extends Id<string>>(value: number): I {
//!     return value as I;
//! }
//! export function seqFromNumber(value: number): Seq {
//!     return value as Seq;
//! }
//! ```
//!
//! The brand is a compile-time-only marker over a `number`, so the port's
//! aliases are plain `i64` (JSON-safe integers, the upstream storage
//! contract) and the two functions are identities. They are kept as
//! documentation of the trusted boundaries.

/// Erased nominal number identifying one durable record kind
/// (`types.ts` `Id<Kind, Type>`).
pub type Id = i64;

/// Conversation record identity (`types.ts` `ConversationId`).
pub type ConversationId = i64;

/// Entry record identity (`types.ts` `EntryId`).
pub type EntryId = i64;

/// Task record identity (`types.ts` `TaskId`).
pub type TaskId = i64;

/// Submission record identity (`types.ts` `SubmissionId`).
pub type SubmissionId = i64;

/// Document incarnation identity (`types.ts` `DocumentId`).
pub type DocumentId = i64;

/// Strictly increasing sequence assigned to one atomic storage commit; gaps
/// are permitted (`types.ts` `Seq`).
pub type Seq = i64;

/// The root conversation always uses this reserved ID (`types.ts`
/// `ROOT_CONVERSATION_ID`).
pub const ROOT_CONVERSATION_ID: ConversationId = 1;

/// Apply an erased ID brand at a trusted numeric allocation or decoding
/// boundary. Identity in the port (upstream `ids.ts` `idFromNumber`).
pub fn id_from_number<I: IdMarker>(value: i64) -> I {
    I::from_number(value)
}

/// Apply the erased commit-sequence brand at a trusted storage boundary.
/// Identity in the port (upstream `ids.ts` `seqFromNumber`).
pub fn seq_from_number(value: i64) -> Seq {
    value
}

/// Sealing trait standing in for the erased `I extends Id<string>` brand
/// parameter, so `id_from_number` call sites keep their upstream shape. Every
/// ID alias is `i64`, so a single implementation covers them all.
pub trait IdMarker {
    fn from_number(value: i64) -> Self;
}

impl IdMarker for i64 {
    fn from_number(value: i64) -> Self {
        value
    }
}
