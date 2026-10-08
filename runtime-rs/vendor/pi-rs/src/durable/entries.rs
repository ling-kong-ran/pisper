//! Port of `src/entries.ts`: typed entry kinds whose guard narrows by
//! `EntryRecord.kind`, plus the built-in harness entry kinds. The upstream
//! `D extends JsonValue` type parameter is type-level only and is carried by
//! the caller in Rust as well.

/// Typed entry kind with a narrowing guard (upstream `Entry<D>`). The `is`
/// predicate compares the entry's `kind` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: String,
}

/// `defineEntry(kind)` (`entries.ts:5-9`): rejects empty kinds with the
/// upstream `TypeError` text as a panic — a definition-time programmer error,
/// like upstream.
pub fn define_entry(kind: impl Into<String>) -> Entry {
    let kind = kind.into();
    if kind.is_empty() {
        panic!("Entry kind must be a non-empty string");
    }
    Entry { kind }
}

impl Entry {
    /// `entry.is(candidate)` — `true` when the record exists and its kind
    /// matches (`entries.ts:7`).
    pub fn is(&self, entry: Option<&super::types::EntryRecord>) -> bool {
        match entry {
            Some(record) => record.kind == self.kind,
            None => false,
        }
    }
}

/// User input: `model` is `[UserMessage]`. Written by submissions.
pub const USER_ENTRY_ENTRY_KIND: &str = "pi.user";
/// Provider result with any stop reason: `model` is `[AssistantMessage]`.
/// Written by generation.
pub const ASSISTANT_ENTRY_KIND: &str = "pi.assistant";
/// Positional prompt and tool change: `model` is `[SystemMessage]` with empty
/// `content`.
pub const SYSTEM_ENTRY_KIND: &str = "pi.system";
/// Tool result: `model` is `[ToolResultMessage]`, whose content ends with the
/// rendered diagnostics block; `data` holds the structured diagnostics,
/// possibly none.
pub const TOOL_RESULT_ENTRY_KIND: &str = "pi.tool-result";
/// Start of a new context: always `head: "self"`, with `model` absent for a
/// plain reset or `[UserMessage]` carrying the handoff text.
pub const RESET_ENTRY_KIND: &str = "pi.reset";

/// `UserEntry` (`entries.ts:16`).
pub fn user_entry() -> Entry {
    define_entry(USER_ENTRY_ENTRY_KIND)
}

/// `AssistantEntry` (`entries.ts:18`).
pub fn assistant_entry() -> Entry {
    define_entry(ASSISTANT_ENTRY_KIND)
}

/// `SystemEntry` (`entries.ts:20`).
pub fn system_entry() -> Entry {
    define_entry(SYSTEM_ENTRY_KIND)
}

/// `ToolResultEntry` (`entries.ts:25-26`).
pub fn tool_result_entry() -> Entry {
    define_entry(TOOL_RESULT_ENTRY_KIND)
}

/// `ResetEntry` (`entries.ts:29`).
pub fn reset_entry() -> Entry {
    define_entry(RESET_ENTRY_KIND)
}

/// Start of a new context: always `head: "self"`, with `model` absent for a
/// plain reset or `[UserMessage]` carrying the handoff text.
pub const COMPACTION_ENTRY_KIND: &str = "pi.compaction";

/// `CompactionEntry` (`entries.ts:31-34`, v1.0.0): `model` is `[UserMessage]`
/// with the wrapped summary, `head` the first kept entry. Written by
/// compaction tasks, directly or through a write submission. `data` carries
/// `{ reason: CompactionReason }`.
pub fn compaction_entry() -> Entry {
    define_entry(COMPACTION_ENTRY_KIND)
}
