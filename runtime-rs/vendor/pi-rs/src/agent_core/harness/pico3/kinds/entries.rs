//! Port of `packages/agent/src/harness/pico3/kinds/entries.ts` (39 lines):
//! the core entry-kind witness table (`entries = { user, assistant,
//! toolResult, system, notice, usage, summary, handoff, reset }`). The
//! typed entry shapes are the port's JSON records ([`Entry`]); the
//! witnesses carry the `kind` literal and the `is` check exactly like
//! upstream.

use crate::agent_core::harness::pico3::types::{Entry, EntryKind};

/// Upstream `coreEntry` (`entries.ts:26-27`).
fn core_entry(kind: &str) -> EntryKind {
    EntryKind {
        kind: kind.to_owned(),
    }
}

/// Upstream `entries` (`entries.ts:29-39`).
pub fn builtin_entries() -> Vec<EntryKind> {
    vec![
        core_entry("pi.user"),
        core_entry("pi.assistant"),
        core_entry("pi.tool_result"),
        core_entry("pi.system"),
        core_entry("pi.notice"),
        core_entry("pi.usage"),
        core_entry("pi.summary"),
        core_entry("pi.handoff"),
        core_entry("pi.reset"),
    ]
}

/// Upstream `EntryKind#is` over an optional entry (`entries.ts:27`).
pub fn entry_is(kind: &EntryKind, entry: Option<&Entry>) -> bool {
    kind.is(entry)
}
