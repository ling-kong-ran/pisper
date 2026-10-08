//! Port of `packages/agent/src/harness/session/jsonl/` (M3b Task 7): the
//! format-4 JSONL session persistence layer — the storage header and options
//! ([`types`]), the v3/v4 header codec ([`codec`]), transaction
//! parse/serialize + atomic publication ([`io`]), the file-backed
//! [`storage::JsonlStorage`], the legacy v3 normalizer ([`legacy_v3`]), the
//! streaming fork ([`fork`]), and the session repository ([`repo`]).
//!
//! Wire format (THE compatibility envelope): line 1 is the header
//! `{"v":4,"kind":"header","id",...}`; each later line is one transaction —
//! a bare committed-write object when the transaction has one write, an
//! array otherwise. Bytes after the last newline are a torn tail and are
//! discarded on open (rewritten atomically).
//!
//! Disclosed substitutions:
//! - [`iso8601`] hand-rolls the two `Date` operations the layer uses
//!   (`Date.parse` on toISOString-shaped stamps, `toISOString`), following
//!   the in-repo Hinnant civil-date precedent (`ai/retry.rs`); other
//!   `Date.parse` input formats are not accepted.

pub mod codec;
pub mod fork;
pub mod io;
pub mod iso8601;
pub mod legacy_v3;
pub mod repo;
pub mod storage;
pub mod types;

pub use codec::*;
pub use fork::*;
pub use io::*;
pub use legacy_v3::*;
pub use repo::*;
pub use storage::*;
pub use types::*;
