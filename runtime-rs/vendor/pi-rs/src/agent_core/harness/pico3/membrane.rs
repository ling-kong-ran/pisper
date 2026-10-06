//! Port of `packages/agent/src/harness/pico3/membrane.ts` (123 lines): the
//! transaction-scoped revocable membrane over chord-tracked documents.
//!
//! # Disclosed substitution
//!
//! Upstream's membrane is a `Proxy` factory: every object reached through a
//! document view is wrapped lazily, all wrappers of one transaction share one
//! liveness flag, identity is preserved by a `WeakMap`, mutations forward to
//! the tracked target so the chord tracker records ops, and — at transaction
//! finish — `revoke()` makes every retained wrapper (root or nested) throw on
//! any operation. It also rejects wrapper-into-wrapper assignment, clones
//! plain inputs, and blocks prototype/accessor tricks.
//!
//! Rust has no proxies and the borrow checker already enforces most of the
//! contract structurally: a document view borrows the transaction, so it
//! cannot outlive the callback, cannot be stored past the commit, and cannot
//! alias another document. What remains expressible — and what the port
//! keeps verbatim — is the liveness flag: every document access through the
//! transaction goes through [`Membrane::check`], `revoke()` flips the flag in
//! a `finally` position ([`crate::agent_core::harness::pico3::session::Session::commit`]),
//! and the failure message is upstream's ("document proxy ({what}) used
//! outside its transaction", `membrane.ts:31`). The wrapper-identity and
//! clone-plain-input guarantees live in the transaction's document accessors
//! (all writes pass through serde round-trips, `session.ts` `plain`), and the
//! prototype tricks have no Rust equivalent.

use crate::agent_core::harness::pico3::types::NamedMessage;

/// Upstream `Membrane` (`membrane.ts:13-123`), reduced to its observable
/// liveness contract; see the module docs.
#[derive(Debug)]
pub struct Membrane {
    alive: bool,
    what: String,
}

impl Membrane {
    /// Upstream `new Membrane(what)` (`membrane.ts:19-21`).
    pub fn new(what: impl Into<String>) -> Membrane {
        Membrane {
            alive: true,
            what: what.into(),
        }
    }

    /// Upstream `revoke()` (`membrane.ts:23-25`): every view handed out by
    /// this membrane fails from now on.
    pub fn revoke(&mut self) {
        self.alive = false;
    }

    /// The upstream "used outside its transaction" `TypeError`
    /// (`membrane.ts:31`): every proxied operation checks liveness first.
    pub fn check(&self) -> Result<(), NamedMessage> {
        if self.alive {
            Ok(())
        } else {
            Err(NamedMessage {
                name: "TypeError",
                message: format!(
                    "document proxy ({}) used outside its transaction",
                    self.what
                ),
            })
        }
    }

    /// Upstream `get alive` is implicit; the transaction asserts through
    /// [`Membrane::check`]. Exposed for tests.
    pub fn is_alive(&self) -> bool {
        self.alive
    }
}
