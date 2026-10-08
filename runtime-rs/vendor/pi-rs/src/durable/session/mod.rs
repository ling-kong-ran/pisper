//! Port of `src/session/**`: the Session kernel — one mutation line, the
//! loaded document tracker cache, committed publication, and the transaction
//! surface of one commit callback.
//!
//! Divergences (structural, disclosed):
//!
//! - **D5 (sync transaction).** Upstream `Tx` operations are `async` because
//!   `Storage` is; with the port's synchronous [`Storage`](crate::durable::storage::Storage)
//!   they are synchronous and always settle before the callback returns, so
//!   the pending-operation tracking set (`#pendingOperations` /
//!   `settleFailure`'s drain) collapses into a sealed flag.
//! - **D6 (mutation line).** Upstream serializes jobs through a promise tail
//!   (`#enqueue`); the port holds one fair `tokio::sync::Mutex` for the same
//!   serialized-commit semantics.

pub mod forks;
pub mod observation;
#[cfg(test)]
mod oracle_tests;
#[allow(clippy::module_inception)] // mirrors the upstream `session/` directory layout
pub mod session;
#[cfg(test)]
mod tests;
pub mod transaction;

pub use session::{create_session, Session, SessionHooks};
pub use transaction::{LoadedDocument, Transaction, TransactionScope};
