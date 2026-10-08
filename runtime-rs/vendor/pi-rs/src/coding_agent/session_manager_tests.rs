//! Ports of the upstream session-manager test suites
//! (`test/session-manager/*.test.ts`, sha256 at migration time:
//! build-context `e4f1c5ef9316`, custom-session-id `551b05e9d008`,
//! file-operations `71b66d7b2fb5`, labels `072829ad416a`,
//! load-entries `fd0ce2facdce`, migration `7e936fe5402e`,
//! save-entry `530b961602e3`, tree-traversal `3a80de63661b`) plus byte-for-byte
//! comparisons against the node oracle
//! (`tests/fixtures/session_manager_oracle/session_manager.oracle.json`).
//!
//! Oracle scrub contract (mirrors the JS side documented in
//! tests/fixtures/session_manager_oracle/oracle_session_manager.mjs):
//! - scenario root path occurrences → `<root>`, process cwd → `<cwd>`
//! - `<date>T<hh>-<mm>-<ss>-<mmm>Z_` filename stamps → `<stamp>_`
//! - string values under `timestamp` shaped as ISO-Z →
//!   `1970-01-01T00:00:00.000Z`; numeric values under `timestamp`/`created`
//!   → `0` (`modified` is fixture-pinned and kept)
//! - strings longer than 128 chars → first 32 chars + `<len=<original>>`
//! - canonical mode key-sorts every object (serde_json's BTreeMap ordering)
//!
//! The oracle runs upstream with deterministic id stubs mirrored by
//! [`test_id_seam`]: entry ids "00000001"-style, session ids "@u<n>" — each
//! oracle block is transcribed here operation-by-operation inside one test
//! function so the counter sequence matches.

use std::sync::OnceLock;

use regex::Regex as Regexp;
use serde::Serialize;
use serde_json::{json, Value};

use super::{
    assert_valid_session_id, build_context_entries, build_session_context,
    find_most_recent_session, get_default_session_dir_with, get_latest_compaction_entry,
    load_entries_from_file, migrate_session_entries, parse_session_entries,
    session_entry_to_context_messages, BranchSummaryEntry, CompactionEntry, CustomEntry,
    CustomMessageEntry, FileEntry, LabelEntry, LeafRef, MessageEntry, ModelChangeEntry,
    NewSessionOptions, SessionEntry, SessionHeader, SessionManager, SessionManagerError,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::content::TextContent;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, StringOrBlocks, SystemMessage, TextOrImageBlock, UserMessage,
};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
use crate::coding_agent::session_manager::test_id_seam;

// ---------------------------------------------------------------------------
// Oracle harness
// ---------------------------------------------------------------------------

const ORACLE_JSON: &str =
    include_str!("../../tests/fixtures/session_manager_oracle/session_manager.oracle.json");

fn oracle() -> &'static Value {
    static ORACLE: OnceLock<Value> = OnceLock::new();
    ORACLE.get_or_init(|| serde_json::from_str(ORACLE_JSON).expect("oracle json parses"))
}

fn oracle_cap(path: &[&str]) -> &'static str {
    let mut value = oracle();
    for key in path {
        value = value
            .get(key)
            .unwrap_or_else(|| panic!("oracle key missing: {path:?}"));
    }
    value
        .as_str()
        .unwrap_or_else(|| panic!("oracle cap {path:?} is not a string"))
}

fn oracle_file_bytes(key: &str) -> &'static str {
    oracle()
        .get("fileBytes")
        .and_then(|bytes| bytes.get(key))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("oracle fileBytes missing {key}"))
}

fn oracle_error(key: &str) -> &'static str {
    oracle()
        .get("errors")
        .and_then(|errors| errors.get(key))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("oracle errors missing {key}"))
}

fn iso_regexp() -> &'static Regexp {
    static RE: OnceLock<Regexp> = OnceLock::new();
    RE.get_or_init(|| {
        Regexp::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$").expect("iso regex")
    })
}

fn stamp_regexp() -> &'static Regexp {
    static RE: OnceLock<Regexp> = OnceLock::new();
    RE.get_or_init(|| {
        Regexp::new(r"\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_").expect("stamp regex")
    })
}

fn process_cwd_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn canonical_path_or_same(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn scrub_string(root: &str, key: &str, value: &str) -> String {
    let mut scrubbed = value.replace(root, "<root>");
    scrubbed = scrubbed.replace(&process_cwd_string(), "<cwd>");
    scrubbed = stamp_regexp()
        .replace_all(&scrubbed, "<stamp>_")
        .into_owned();
    if (key == "timestamp" || key == "labelTimestamp") && iso_regexp().is_match(&scrubbed) {
        scrubbed = "1970-01-01T00:00:00.000Z".to_string();
    }
    if scrubbed.chars().count() > 128 {
        let prefix: String = scrubbed.chars().take(32).collect();
        scrubbed = format!("{prefix}<len={}>", value.len());
    }
    scrubbed
}

fn scrub_value(root: &str, key: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(scrub_string(root, key, text)),
        Value::Number(_) => {
            if key == "timestamp" || key == "created" {
                json!(0)
            } else {
                value.clone()
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| scrub_value(root, key, item))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(name, item)| (name.clone(), scrub_value(root, name, item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Canonical (key-sorted) scrubbed JSON of a serializable value.
fn canon(root: &str, value: &impl Serialize) -> String {
    let serialized = serde_json::to_value(value).expect("serializable");
    let mut scrubbed = scrub_value(root, "<no-key>", &serialized);
    // environment-anchored: both sides normalized. Root-relative fixture
    // inputs (`/project`) resolve against the live drive while the capture
    // stores the capture machine's `C:` form, so both sides go through the
    // shared anchor scrub (separators + drive/root anchors) before the
    // canonical comparison.
    crate::coding_agent::oracle_scrub::scrub_value(&mut scrubbed);
    // environment-anchored: both sides normalized. On POSIX the
    // root-anchored inputs stay `/...` and the shared scrub's root-anchored
    // branch prepends the `<DRV>:/` placeholder to a leading `/`, rendering
    // `<DRV>://...`; the win32 capture resolves onto the live drive and
    // renders `<DRV>:/...`. Collapse the duplicated separator on BOTH sides
    // (upstream-on-linux reports the same POSIX path) so the pin covers the
    // path, not the scrub branch.
    collapse_drive_placeholder(&mut scrubbed);
    // The Node oracle explicitly sorts its canonical comparison tree. Keep
    // this test-only normalization separate from raw JSONL wire assertions.
    scrubbed.sort_all_objects();
    serde_json::to_string(&scrubbed).expect("canon string")
}

/// environment-anchored companion to `canon`: collapse the `<DRV>://`
/// rendering (see `canon`) on both comparison sides.
fn collapse_drive_placeholder(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = text.replace("<DRV>://", "<DRV>:/");
        }
        Value::Array(items) => {
            for item in items {
                collapse_drive_placeholder(item);
            }
        }
        Value::Object(object) => {
            for (_, child) in object.iter_mut() {
                collapse_drive_placeholder(child);
            }
        }
        _ => {}
    }
}

/// Order-preserving scrub of raw JSONL file text (textual replacements keep
/// the serialized key order intact).
fn scrub_file_text(root: &str, text: &str) -> String {
    let iso_field = Regexp::new(r#""timestamp":"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z""#)
        .expect("iso field");
    let numeric_field = Regexp::new(r#""timestamp":-?\d+"#).expect("numeric field");
    // JSON strings escape backslashes, so path occurrences inside JSONL lines
    // carry doubled separators; replace the escaped forms too. The
    // canonicalized forms are replaced as well (environment-anchored: CI temp
    // dirs can be 8.3 short paths while product code canonicalizes).
    let root_escaped = root.replace('\\', "\\\\");
    let cwd = process_cwd_string();
    let cwd_escaped = cwd.replace('\\', "\\\\");
    let root_canonical = canonical_path_or_same(root);
    let root_canonical_escaped = root_canonical.replace('\\', "\\\\");
    let cwd_canonical = canonical_path_or_same(&cwd);
    let cwd_canonical_escaped = cwd_canonical.replace('\\', "\\\\");
    text.split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                return line.to_string();
            }
            let mut scrubbed = line
                .replace(&root_escaped, "<root>")
                .replace(&cwd_escaped, "<cwd>")
                .replace(&root_canonical_escaped, "<root>")
                .replace(&cwd_canonical_escaped, "<cwd>")
                .replace(root, "<root>")
                .replace(&cwd, "<cwd>")
                .replace(&root_canonical, "<root>")
                .replace(&cwd_canonical, "<cwd>");
            scrubbed = stamp_regexp()
                .replace_all(&scrubbed, "<stamp>_")
                .into_owned();
            scrubbed = iso_field
                .replace_all(&scrubbed, r#""timestamp":"1970-01-01T00:00:00.000Z""#)
                .into_owned();
            scrubbed = numeric_field
                .replace_all(&scrubbed, r#""timestamp":0"#)
                .into_owned();
            scrubbed
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn canonical_comparison_does_not_sort_wire_scrubbing() {
    let raw = r#"{"z":{"y":1,"a":2},"a":[{"z":3,"b":4}]}"#;
    let value: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        canon("<unused-root>", &value),
        r#"{"a":[{"b":4,"z":3}],"z":{"a":2,"y":1}}"#
    );
    assert_eq!(scrub_file_text("<unused-root>", raw), raw);
    assert_eq!(serde_json::to_string(&value).unwrap(), raw);
}

/// environment-anchored: both sides normalized — route a raw oracle canon
/// rendering through the same canon scrub/sort pipeline used for actual
/// values. The oracle stores the rendering as a JSON string, so it is parsed
/// first to keep the re-serialization single-layer.
fn oracle_canon_value(value: &Value) -> String {
    let raw = value.as_str().unwrap_or_default();
    match serde_json::from_str::<Value>(raw) {
        Ok(parsed) => canon("<unused-root>", &parsed),
        Err(_) => canon("<unused-root>", &value),
    }
}

fn oracle_canon(path: &[&str]) -> String {
    let expected: serde_json::Value =
        serde_json::from_str(oracle_cap(path)).expect("oracle canon json");
    canon("<unused-root>", &expected)
}

/// environment-anchored: both sides normalized — same pipeline for the raw
/// oracle error texts (stored as canon renderings, i.e. JSON-quoted).
fn oracle_error_canon(key: &str) -> String {
    oracle_canon_value(&Value::String(oracle_error(key).to_string()))
}

fn assert_canon_matches(root: &str, value: &impl Serialize, oracle_path: &[&str]) {
    assert_eq!(
        canon(root, value),
        oracle_canon(oracle_path),
        "oracle mismatch at {oracle_path:?}"
    );
}

fn assert_file_bytes_match(root: &str, actual: &str, key: &str) {
    // environment-anchored: both sides normalized. The separators inside a
    // `<root>`/`<cwd>`-anchored path are the host join separators (upstream
    // on POSIX writes posix joins where the win32 capture wrote `\`; inside
    // JSON strings the backslash form appears JSON-escaped). Unify every
    // separator within those path tokens to `/` on both sides so the byte
    // comparison stays host-independent.
    static PATH_TOKEN: OnceLock<Regexp> = OnceLock::new();
    let path_token = PATH_TOKEN.get_or_init(|| {
        // Path characters as they appear inside the JSONL strings; `\`
        // matches the (doubled) JSON-escaped separators.
        Regexp::new(r#"(<root>|<cwd>)[A-Za-z0-9_./<>:\\-]*"#).expect("path token regex")
    });
    let normalize_placeholders = |text: &str| -> String {
        path_token
            .replace_all(text, |caps: &regex::Captures| {
                // JSON-escaped backslash pair -> posix separator.
                caps[0].replace("\\\\", "/")
            })
            .into_owned()
    };
    assert_eq!(
        normalize_placeholders(&scrub_file_text(root, actual)),
        normalize_placeholders(oracle_file_bytes(key)),
        "file bytes for {key}"
    );
}

fn scenario_dir(tag: &str) -> tempfile::TempDir {
    tempfile::TempDir::with_prefix(format!("pi-session-manager-{tag}-")).expect("tempdir")
}

fn temp_path(dir: &std::path::Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

fn read_file(path: &str) -> String {
    std::fs::read_to_string(path).expect("readable file")
}

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

fn usage_fixture(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    total_tokens: u64,
) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens,
        cost: UsageCost::default(),
    }
}

fn big_usage() -> Usage {
    Usage {
        input: 10,
        output: 20,
        cache_read: 30,
        cache_write: 40,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 100,
        cost: UsageCost {
            input: 0.1,
            output: 0.2,
            cache_read: 0.3,
            cache_write: 0.4,
            total: 1.0,
        },
    }
}

/// upstream `userMsg` (test/utilities.ts, fixed timestamp)
fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: 1,
    })
}

/// upstream `assistantMsg` (test/utilities.ts, fixed timestamp)
fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "test".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: usage_fixture(1, 1, 0, 0, 2),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    })
}

/// the build-context fixture message entry
fn msg_entry(id: &str, parent_id: Option<&str>, role: &str, text: &str) -> SessionEntry {
    let message = if role == "user" {
        json!({ "role": role, "content": text, "timestamp": 1 })
    } else {
        json!({
            "role": role,
            "content": [{ "type": "text", "text": text }],
            "api": "anthropic-messages",
            "provider": "anthropic",
            "model": "claude-test",
            "usage": {
                "input": 1,
                "output": 1,
                "cacheRead": 0,
                "cacheWrite": 0,
                "totalTokens": 2,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        })
    };
    SessionEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        message: serde_json::from_value(message).expect("fixture message"),
    })
}

fn compaction_entry(
    id: &str,
    parent_id: Option<&str>,
    summary: &str,
    first_kept: &str,
) -> SessionEntry {
    SessionEntry::Compaction(CompactionEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        summary: summary.to_string(),
        first_kept_entry_id: Some(first_kept.to_string()),
        tokens_before: 1000,
        details: None,
        usage: None,
        from_hook: None,
        system_message: None,
        first_kept_entry_index: None,
    })
}

fn branch_summary_entry(
    id: &str,
    parent_id: Option<&str>,
    summary: &str,
    from_id: &str,
) -> SessionEntry {
    SessionEntry::BranchSummary(BranchSummaryEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        from_id: from_id.to_string(),
        summary: summary.to_string(),
        details: None,
        usage: None,
        from_hook: None,
    })
}

fn custom_entry(id: &str, parent_id: Option<&str>, custom_type: &str, data: Value) -> SessionEntry {
    SessionEntry::Custom(CustomEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        custom_type: custom_type.to_string(),
        data: Some(data),
    })
}

fn thinking_level_entry(id: &str, parent_id: Option<&str>, level: &str) -> SessionEntry {
    SessionEntry::ThinkingLevelChange(super::ThinkingLevelChangeEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        thinking_level: level.to_string(),
    })
}

fn model_change_entry(
    id: &str,
    parent_id: Option<&str>,
    provider: &str,
    model_id: &str,
) -> SessionEntry {
    SessionEntry::ModelChange(ModelChangeEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        provider: provider.to_string(),
        model_id: model_id.to_string(),
    })
}

fn custom_message_entry(
    id: &str,
    parent_id: Option<&str>,
    custom_type: &str,
    content: Value,
    display: bool,
    details: Option<Value>,
) -> SessionEntry {
    SessionEntry::CustomMessage(CustomMessageEntry {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
        custom_type: custom_type.to_string(),
        content: Some(serde_json::from_value(content).expect("fixture content")),
        display,
        details,
    })
}

// ---------------------------------------------------------------------------
// Oracle: buildSessionContext grid (build-context.test.ts scenarios)
// ---------------------------------------------------------------------------

fn system_message_fixture() -> SystemMessage {
    SystemMessage {
        content: StringOrBlocks::Text("system prompt".to_string()),
        sections: None,
        tools_added: None,
        tools_removed: None,
        timestamp: 123,
    }
}

fn context_case_entries(name: &str) -> Vec<SessionEntry> {
    match name {
        "empty" => vec![],
        "single-user" => vec![msg_entry("m1", None, "user", "hello")],
        "simple-conversation" => vec![
            msg_entry("m1", None, "user", "hello"),
            msg_entry("m2", Some("m1"), "assistant", "hi there"),
            msg_entry("m3", Some("m2"), "user", "how are you"),
            msg_entry("m4", Some("m3"), "assistant", "great"),
        ],
        "thinking-level" => vec![
            msg_entry("m1", None, "user", "hello"),
            thinking_level_entry("t1", Some("m1"), "high"),
            msg_entry("m2", Some("t1"), "assistant", "thinking hard"),
        ],
        "model-from-assistant" => vec![
            msg_entry("m1", None, "user", "hello"),
            msg_entry("m2", Some("m1"), "assistant", "hi"),
        ],
        "model-change-overwritten" => vec![
            msg_entry("m1", None, "user", "hello"),
            model_change_entry("mc1", Some("m1"), "openai", "gpt-4"),
            msg_entry("m2", Some("mc1"), "assistant", "hi"),
        ],
        "compaction-includes-summary" => vec![
            msg_entry("m1", None, "user", "first"),
            msg_entry("m2", Some("m1"), "assistant", "response1"),
            msg_entry("m3", Some("m2"), "user", "second"),
            msg_entry("m4", Some("m3"), "assistant", "response2"),
            compaction_entry("c1", Some("m4"), "Summary of first two turns", "m3"),
            msg_entry("m5", Some("c1"), "user", "third"),
            msg_entry("m6", Some("m5"), "assistant", "response3"),
        ],
        "compaction-keeps-from-first" => vec![
            msg_entry("m1", None, "user", "first"),
            msg_entry("m2", Some("m1"), "assistant", "response"),
            compaction_entry("c1", Some("m2"), "Empty summary", "m1"),
            msg_entry("m3", Some("c1"), "user", "second"),
        ],
        "multiple-compactions-latest" => vec![
            msg_entry("m1", None, "user", "a"),
            msg_entry("m2", Some("m1"), "assistant", "b"),
            compaction_entry("c1", Some("m2"), "First summary", "m1"),
            msg_entry("m3", Some("c1"), "user", "c"),
            msg_entry("m4", Some("m3"), "assistant", "d"),
            compaction_entry("c2", Some("m4"), "Second summary", "m4"),
            msg_entry("m5", Some("c2"), "user", "e"),
        ],
        "context-entries-with-custom" => vec![
            msg_entry("m1", None, "user", "first"),
            custom_entry("cu1", Some("m1"), "old-state", json!({ "hidden": true })),
            msg_entry("m2", Some("cu1"), "assistant", "response1"),
            custom_entry("cu2", Some("m2"), "kept-card", json!({ "title": "Kept" })),
            msg_entry("m3", Some("cu2"), "user", "second"),
            compaction_entry("c1", Some("m3"), "Summary", "cu2"),
            custom_entry("cu3", Some("c1"), "after-card", json!({ "title": "After" })),
            msg_entry("m4", Some("cu3"), "assistant", "response2"),
        ],
        "settings-from-full-path" => vec![
            msg_entry("m1", None, "user", "first"),
            thinking_level_entry("t1", Some("m1"), "high"),
            msg_entry("m2", Some("t1"), "assistant", "response1"),
            msg_entry("m3", Some("m2"), "user", "second"),
            compaction_entry("c1", Some("m3"), "Summary", "m3"),
        ],
        "branch-a" | "branch-b" => vec![
            msg_entry("m1", None, "user", "start"),
            msg_entry("m2", Some("m1"), "assistant", "response"),
            msg_entry("m3", Some("m2"), "user", "branch A"),
            msg_entry("m4", Some("m2"), "user", "branch B"),
        ],
        "branch-summary-in-path" => vec![
            msg_entry("m1", None, "user", "start"),
            msg_entry("m2", Some("m1"), "assistant", "response"),
            msg_entry("m3", Some("m2"), "user", "abandoned path"),
            branch_summary_entry("b1", Some("m2"), "Summary of abandoned work", "m3"),
            msg_entry("m4", Some("b1"), "user", "new direction"),
        ],
        "complex-tree" | "complex-tree-branch-leaf" => vec![
            msg_entry("m1", None, "user", "start"),
            msg_entry("m2", Some("m1"), "assistant", "r1"),
            msg_entry("m3", Some("m2"), "user", "q2"),
            msg_entry("m4", Some("m3"), "assistant", "r2"),
            compaction_entry("c1", Some("m4"), "Compacted history", "m3"),
            msg_entry("m5", Some("c1"), "user", "q3"),
            msg_entry("m6", Some("m5"), "assistant", "r3"),
            msg_entry("m7", Some("m3"), "user", "wrong path"),
            msg_entry("m8", Some("m7"), "assistant", "wrong response"),
            branch_summary_entry("b1", Some("m3"), "Tried wrong approach", "m8"),
            msg_entry("m9", Some("b1"), "user", "better approach"),
        ],
        "leaf-not-found" | "leaf-null" => vec![
            msg_entry("m1", None, "user", "hello"),
            msg_entry("m2", Some("m1"), "assistant", "hi"),
        ],
        "orphaned-entries" => vec![
            msg_entry("m1", None, "user", "hello"),
            msg_entry("m2", Some("missing"), "assistant", "orphan"),
        ],
        "compaction-with-system-message" => vec![
            msg_entry("m1", None, "user", "first"),
            msg_entry("m2", Some("m1"), "assistant", "response"),
            {
                let mut compaction =
                    match compaction_entry("c1", Some("m2"), "Summary with system", "m1") {
                        SessionEntry::Compaction(compaction) => compaction,
                        other => unreachable!("{other:?}"),
                    };
                compaction.system_message = Some(system_message_fixture());
                SessionEntry::Compaction(compaction)
            },
            msg_entry("m3", Some("c1"), "user", "second"),
        ],
        "custom-message-entries-in-context" => vec![
            msg_entry("m1", None, "user", "first"),
            custom_message_entry(
                "cm1",
                Some("m1"),
                "note",
                json!("plain note"),
                true,
                Some(json!({ "meta": 1 })),
            ),
            custom_message_entry(
                "cm2",
                Some("cm1"),
                "blocks",
                json!([{ "type": "text", "text": "block note" }]),
                false,
                None,
            ),
            msg_entry("m2", Some("cm2"), "assistant", "reply"),
        ],
        other => panic!("unknown oracle context case {other}"),
    }
}

fn context_case_leaf(leaf: Option<&Value>) -> LeafRef<'_> {
    match leaf {
        None => LeafRef::Unspecified,
        Some(Value::Null) => LeafRef::Null,
        Some(Value::String(id)) => LeafRef::Id(id),
        other => panic!("unexpected leaf {other:?}"),
    }
}

#[test]
fn build_session_context_matches_oracle_grid() {
    for case in oracle()["buildContext"]
        .as_array()
        .expect("buildContext array")
    {
        let name = case["name"].as_str().expect("case name");
        let leaf = context_case_leaf(case.get("leafId"));
        let case_entries = context_case_entries(name);
        let ctx = build_session_context(&case_entries, leaf);
        let ctx_entries = build_context_entries(&case_entries, leaf);

        let ids: Vec<&str> = ctx_entries.iter().filter_map(SessionEntry::id).collect();
        let expected_ids: Vec<String> = case["ids"]
            .as_array()
            .expect("ids")
            .iter()
            .map(|id| id.as_str().expect("id str").to_string())
            .collect();
        assert_eq!(ids, expected_ids, "context entry ids for {name}");
        assert_eq!(
            ctx.thinking_level,
            case["thinkingLevel"].as_str().expect("thinkingLevel"),
            "thinkingLevel for {name}"
        );
        assert_eq!(
            serde_json::to_value(&ctx.model).unwrap(),
            case["model"],
            "model for {name}"
        );
        let roles: Vec<&str> = ctx.messages.iter().map(AgentMessage::role).collect();
        let expected_roles: Vec<&str> = case["roles"]
            .as_array()
            .expect("roles")
            .iter()
            .map(|role| role.as_str().expect("role str"))
            .collect();
        assert_eq!(roles, expected_roles, "roles for {name}");
        assert_eq!(
            canon("<root>", &ctx.messages),
            oracle_canon_value(&case["canon"]),
            "canon messages for {name}"
        );
    }
}

#[test]
fn session_entry_to_context_messages_matches_oracle_battery() {
    let system_null = json!({
        "type": "message", "id": "n1", "parentId": null, "timestamp": "2025-01-01T00:00:00Z",
        "message": { "role": "system", "content": null, "timestamp": 5 }
    });
    let battery: Vec<(&str, SessionEntry)> = vec![
        ("user-message", msg_entry("m1", None, "user", "hello")),
        (
            "assistant-message",
            msg_entry("m2", None, "assistant", "hi"),
        ),
        ("thinking-level", thinking_level_entry("t1", None, "high")),
        (
            "model-change",
            model_change_entry("mc1", None, "openai", "gpt-4"),
        ),
        (
            "custom-entry",
            custom_entry("cu1", None, "state", json!({ "a": 1 })),
        ),
        (
            "custom-message-text",
            custom_message_entry(
                "cm1",
                None,
                "note",
                json!("plain"),
                true,
                Some(json!({ "meta": 1 })),
            ),
        ),
        (
            "custom-message-blocks",
            custom_message_entry(
                "cm2",
                None,
                "blocks",
                json!([{ "type": "text", "text": "b" }]),
                false,
                None,
            ),
        ),
        (
            "branch-summary",
            branch_summary_entry("b1", None, "summary text", "m1"),
        ),
        (
            "branch-summary-empty",
            branch_summary_entry("b2", None, "", "m1"),
        ),
        ("compaction", compaction_entry("c1", None, "summary", "m1")),
        ("compaction-with-system", {
            let mut compaction = match compaction_entry("c2", None, "summary", "m1") {
                SessionEntry::Compaction(compaction) => compaction,
                other => unreachable!("{other:?}"),
            };
            compaction.system_message = Some(system_message_fixture());
            SessionEntry::Compaction(compaction)
        }),
        (
            "session-info",
            SessionEntry::SessionInfo(super::SessionInfoEntry {
                id: "si1".to_string(),
                parent_id: None,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
                name: Some("name".to_string()),
            }),
        ),
        (
            "label",
            SessionEntry::Label(LabelEntry {
                id: "l1".to_string(),
                parent_id: None,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
                target_id: "m1".to_string(),
                label: Some("checkpoint".to_string()),
            }),
        ),
        (
            "label-clear",
            SessionEntry::Label(LabelEntry {
                id: "l2".to_string(),
                parent_id: None,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
                target_id: "m1".to_string(),
                label: None,
            }),
        ),
        ("system-null-content", loose_entry(system_null)),
        (
            "user-null-content",
            loose_entry(json!({
                "type": "message", "id": "n2", "parentId": null, "timestamp": "2025-01-01T00:00:00Z",
                "message": { "role": "user", "content": null, "timestamp": 6 }
            })),
        ),
        (
            "assistant-null-content",
            loose_entry(json!({
                "type": "message", "id": "n3", "parentId": null, "timestamp": "2025-01-01T00:00:00Z",
                "message": {
                    "role": "assistant", "content": null,
                    "api": "anthropic-messages", "provider": "anthropic", "model": "claude-test",
                    "usage": {
                        "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                    },
                    "stopReason": "stop", "timestamp": 7
                }
            })),
        ),
        (
            "toolresult-null-content",
            loose_entry(json!({
                "type": "message", "id": "n4", "parentId": null, "timestamp": "2025-01-01T00:00:00Z",
                "message": {
                    "role": "toolResult", "toolCallId": "call-1", "toolName": "nested-model",
                    "content": null, "isError": false, "timestamp": 8
                }
            })),
        ),
        (
            "custom-message-null-content",
            SessionEntry::CustomMessage(CustomMessageEntry {
                id: "cm3".to_string(),
                parent_id: None,
                timestamp: "2025-01-01T00:00:00Z".to_string(),
                custom_type: "note".to_string(),
                content: None,
                display: true,
                details: None,
            }),
        ),
    ];

    let oracle_cases = oracle()["entryToContext"]
        .as_array()
        .expect("entryToContext array");
    assert_eq!(
        oracle_cases.len(),
        battery.len(),
        "battery size matches oracle"
    );
    for (case_index, case) in oracle_cases.iter().enumerate() {
        let name = case["name"].as_str().expect("name");
        let entry = &battery[case_index].1;
        assert_eq!(
            canon("<root>", &session_entry_to_context_messages(entry)),
            oracle_canon_value(&case["canon"]),
            "entryToContext for {name}"
        );
    }
}

#[test]
fn latest_compaction_and_parse_entries_match_oracle() {
    let none = vec![msg_entry("m1", None, "user", "hi")];
    let last = vec![
        msg_entry("m1", None, "user", "hi"),
        compaction_entry("c1", Some("m1"), "s", "m1"),
    ];
    let middle = vec![
        compaction_entry("c1", None, "s", "m1"),
        msg_entry("m2", Some("c1"), "user", "hi"),
    ];
    for (index, input) in [&none, &last, &middle].into_iter().enumerate() {
        assert_eq!(
            canon("<root>", &get_latest_compaction_entry(input)),
            oracle_canon_value(&oracle()["latestCompaction"][index]["canon"]),
            "latestCompaction case {index}"
        );
    }

    let mixed = r##"{"type":"session","id":"abc","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
not json

{"type":"message","id":"m1","parentId":null,"timestamp":"2025-01-01T00:00:01Z","message":{"role":"user","content":"hi","timestamp":1}}
{"type":"unknown-kind","payload":true}"##;
    let blank_lines = "\n  \n\t\n";
    for (index, content) in [mixed, "", blank_lines].into_iter().enumerate() {
        assert_eq!(
            canon("<root>", &parse_session_entries(content)),
            oracle_canon_value(&oracle()["parseEntries"][index]["canon"]),
            "parseEntries case {index}"
        );
    }
}

#[test]
fn migrate_session_entries_matches_oracle() {
    test_id_seam::reset();
    let mut v1 = vec![
        FileEntry::Session(SessionHeader {
            version: None,
            id: Some("sess-1".to_string()),
            timestamp: Some("2025-01-01T00:00:00Z".to_string()),
            cwd: Some("/tmp".to_string()),
            parent_session: None,
        }),
        FileEntry::Unparsed(json!({
            "type": "message", "timestamp": "2025-01-01T00:00:01Z",
            "message": { "role": "user", "content": "hi", "timestamp": 1 }
        })),
        FileEntry::Entry(SessionEntry::Compaction(CompactionEntry {
            id: String::new(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:02Z".to_string(),
            summary: "s".to_string(),
            first_kept_entry_id: None,
            tokens_before: 10,
            details: None,
            usage: None,
            from_hook: None,
            system_message: None,
            first_kept_entry_index: Some(1),
        })),
    ];
    migrate_session_entries(&mut v1);
    assert_eq!(
        canon("<root>", &v1),
        oracle_canon_value(&oracle()["migrate"][0]["canon"]),
        "v1 migration"
    );

    let mut v2 = vec![
        FileEntry::Session(SessionHeader {
            version: Some(2),
            id: Some("sess-2".to_string()),
            timestamp: Some("2025-01-01T00:00:00Z".to_string()),
            cwd: Some("/tmp".to_string()),
            parent_session: None,
        }),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "hookmsgid", "parentId": null, "timestamp": "2025-01-01T00:00:01Z",
                "message": { "role": "hookMessage", "content": "from a hook", "timestamp": 1 }
            }))
            .expect("fixture entry"),
        ),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "usermsgid", "parentId": "hookmsgid", "timestamp": "2025-01-01T00:00:02Z",
                "message": { "role": "user", "content": "hi", "timestamp": 2 }
            }))
            .expect("fixture entry"),
        ),
    ];
    migrate_session_entries(&mut v2);
    assert_eq!(
        canon("<root>", &v2),
        oracle_canon_value(&oracle()["migrate"][1]["canon"]),
        "v2 migration"
    );

    let mut current = vec![
        FileEntry::Session(SessionHeader {
            version: Some(3),
            id: Some("sess-3".to_string()),
            timestamp: Some("2025-01-01T00:00:00Z".to_string()),
            cwd: Some("/tmp".to_string()),
            parent_session: None,
        }),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "usermsgid", "parentId": null, "timestamp": "2025-01-01T00:00:01Z",
                "message": { "role": "user", "content": "hi", "timestamp": 1 }
            }))
            .expect("fixture entry"),
        ),
    ];
    migrate_session_entries(&mut current);
    assert_eq!(
        canon("<root>", &current),
        oracle_canon_value(&oracle()["migrate"][2]["canon"]),
        "current entries unchanged"
    );
    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// Oracle: loadEntriesFromFile + newline repair + open() discovery
// ---------------------------------------------------------------------------

const VALID_SESSION_LINE: &str =
    r#"{"type":"session","id":"abc","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}"#;
const MESSAGE_LINE: &str = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2025-01-01T00:00:01Z","message":{"role":"user","content":"hi","timestamp":1}}"#;

#[test]
fn load_entries_from_file_matches_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("load");
    let root = dir.path().to_string_lossy().into_owned();

    let mut caps = serde_json::Map::new();

    caps.insert(
        "nonexistent-count".into(),
        json!(load_entries_from_file(&temp_path(dir.path(), "nonexistent.jsonl")).len()),
    );

    let empty_file = temp_path(dir.path(), "empty.jsonl");
    std::fs::write(&empty_file, "").unwrap();
    caps.insert(
        "empty-count".into(),
        json!(load_entries_from_file(&empty_file).len()),
    );
    assert_file_bytes_match(&root, &read_file(&empty_file), "empty");

    let no_header = temp_path(dir.path(), "no-header.jsonl");
    std::fs::write(&no_header, "{\"type\":\"message\",\"id\":\"m1\"}\n").unwrap();
    caps.insert(
        "no-header-count".into(),
        json!(load_entries_from_file(&no_header).len()),
    );
    assert_file_bytes_match(&root, &read_file(&no_header), "no-header");

    let malformed = temp_path(dir.path(), "malformed.jsonl");
    std::fs::write(&malformed, "not json\n").unwrap();
    caps.insert(
        "malformed-count".into(),
        json!(load_entries_from_file(&malformed).len()),
    );
    assert_file_bytes_match(&root, &read_file(&malformed), "malformed");

    let valid = temp_path(dir.path(), "valid.jsonl");
    std::fs::write(&valid, format!("{VALID_SESSION_LINE}\n{MESSAGE_LINE}\n")).unwrap();
    let valid_entries = load_entries_from_file(&valid);
    caps.insert(
        "valid".into(),
        json!({
            "count": valid_entries.len(),
            "types": valid_entries.iter().map(FileEntry::type_name).collect::<Vec<_>>(),
        }),
    );

    let mixed = temp_path(dir.path(), "mixed.jsonl");
    std::fs::write(
        &mixed,
        format!("{VALID_SESSION_LINE}\nnot valid json\n{MESSAGE_LINE}\n"),
    )
    .unwrap();
    caps.insert(
        "mixed-count".into(),
        json!(load_entries_from_file(&mixed).len()),
    );

    let unterminated = format!("{VALID_SESSION_LINE}\n{MESSAGE_LINE}");
    let unterm_file = temp_path(dir.path(), "unterminated.jsonl");
    std::fs::write(&unterm_file, &unterminated).unwrap();
    caps.insert(
        "unterminated-count".into(),
        json!(load_entries_from_file(&unterm_file).len()),
    );
    assert_file_bytes_match(&root, &unterminated, "unterminated-input");
    assert_file_bytes_match(&root, &read_file(&unterm_file), "unterminated-bytes");

    let malformed_tail = format!("{VALID_SESSION_LINE}\n{{\"type\":\"message\"");
    let tail_file = temp_path(dir.path(), "malformed-tail.jsonl");
    std::fs::write(&tail_file, &malformed_tail).unwrap();
    caps.insert(
        "malformed-tail-count".into(),
        json!(load_entries_from_file(&tail_file).len()),
    );
    assert_file_bytes_match(&root, &malformed_tail, "malformed-tail-input");
    assert_file_bytes_match(&root, &read_file(&tail_file), "malformed-tail-bytes");

    let invalid_unterm = temp_path(dir.path(), "invalid.jsonl");
    std::fs::write(&invalid_unterm, "{\"type\":\"message\",\"id\":\"m1\"}").unwrap();
    let invalid_content = read_file(&invalid_unterm);
    caps.insert(
        "invalid-unterminated-count".into(),
        json!(load_entries_from_file(&invalid_unterm).len()),
    );
    caps.insert(
        "invalid-unterminated-unchanged".into(),
        json!(read_file(&invalid_unterm) == invalid_content),
    );

    // cwd discovery through open()
    let stored_cwd = temp_path(dir.path(), "stored-project");
    let header_file = temp_path(dir.path(), "header.jsonl");
    let write_header = |prefix: &str, session_id: &str| {
        std::fs::write(
            &header_file,
            format!(
                "{prefix}{}\n",
                serde_json::to_string(&json!({
                    "type": "session", "version": 3, "id": session_id,
                    "timestamp": "2025-01-01T00:00:00Z", "cwd": stored_cwd,
                }))
                .unwrap()
            ),
        )
        .unwrap();
    };

    write_header("\n  \n", "leading-blank");
    let opened = SessionManager::open(&header_file, Some(&root), None).expect("open");
    caps.insert(
        "leading-blank".into(),
        json!({ "id": opened.get_session_id(), "cwd": opened.get_cwd() }),
    );

    write_header("not json\n{broken json\n", "leading-malformed");
    let opened = SessionManager::open(&header_file, Some(&root), None).expect("open");
    caps.insert(
        "leading-malformed".into(),
        json!({ "id": opened.get_session_id(), "cwd": opened.get_cwd() }),
    );

    write_header("", &"a".repeat(8192));
    let opened = SessionManager::open(&header_file, Some(&root), None).expect("open");
    caps.insert(
        "multi-buffer-header".into(),
        json!({ "id": opened.get_session_id(), "cwd": opened.get_cwd() }),
    );

    assert_eq!(
        canon(&root, &Value::Object(caps)),
        oracle_canon(&["canon", "load-entries"]),
        "load-entries caps"
    );
    test_id_seam::disable();
}

#[test]
fn open_beyond_scan_limit_matches_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("scan");
    let root = dir.path().to_string_lossy().into_owned();
    let scan_limit = 1024 * 1024;
    let stored_cwd = temp_path(dir.path(), "stored-project");
    let override_cwd = temp_path(dir.path(), "override-project");

    let mut caps = serde_json::Map::new();
    for (name, id, prefix) in [
        ("large-header", "a".repeat(scan_limit + 1), String::new()),
        (
            "large-prefix",
            "large-prefix".to_string(),
            format!("{}\n", "x".repeat(scan_limit + 1)),
        ),
    ] {
        let file = temp_path(dir.path(), &format!("{name}.jsonl"));
        std::fs::write(
            &file,
            format!(
                "{prefix}{}\n",
                serde_json::to_string(&json!({
                    "type": "session", "version": 3, "id": id,
                    "timestamp": "2025-01-01T00:00:00Z", "cwd": stored_cwd,
                }))
                .unwrap()
            ),
        )
        .unwrap();
        let mut per_file = serde_json::Map::new();
        for (tag, cwd_override) in [("default", None), ("override", Some(override_cwd.as_str()))] {
            let manager = SessionManager::open(&file, Some(&root), cwd_override).expect("open");
            per_file.insert(
                tag.to_string(),
                json!({ "id": manager.get_session_id(), "cwd": manager.get_cwd() }),
            );
        }
        caps.insert(name.to_string(), Value::Object(per_file));
    }

    assert_eq!(
        canon(&root, &Value::Object(caps)),
        oracle_canon(&["canon", "scan-limit"]),
        "scan-limit caps"
    );
    test_id_seam::disable();
}

#[test]
fn find_most_recent_session_matches_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("recent");
    let root = dir.path().to_string_lossy().into_owned();
    let t1: std::time::SystemTime =
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_000);
    let t2: std::time::SystemTime =
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_050_000);
    let pin = |path: &str, time: std::time::SystemTime| {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open for times");
        file.set_modified(time).expect("set mtime");
    };
    let write_jsonl = |name: &str, content: &str| {
        let path = temp_path(dir.path(), name);
        std::fs::write(&path, content).unwrap();
        path
    };
    let header_line = |id: &str, cwd: &str| {
        serde_json::to_string(&json!({
            "type": "session", "id": id, "timestamp": "2025-01-01T00:00:00Z", "cwd": cwd,
        }))
        .unwrap()
    };

    let mut caps = serde_json::Map::new();
    std::fs::create_dir(dir.path().join("empty-dir")).unwrap();
    caps.insert(
        "empty-dir".into(),
        find_most_recent_session(&temp_path(dir.path(), "empty-dir"), None).into(),
    );
    caps.insert(
        "nonexistent".into(),
        find_most_recent_session(&temp_path(dir.path(), "nonexistent"), None).into(),
    );

    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    std::fs::write(dir.path().join("file.json"), "{}").unwrap();
    caps.insert(
        "non-jsonl".into(),
        find_most_recent_session(&root, None).into(),
    );

    std::fs::write(dir.path().join("invalid.jsonl"), "{\"type\":\"message\"}\n").unwrap();
    caps.insert(
        "invalid-header".into(),
        find_most_recent_session(&root, None).into(),
    );

    let _single = write_jsonl(
        "session.jsonl",
        &format!("{}\n", header_line("abc", "/tmp")),
    );
    caps.insert(
        "single".into(),
        find_most_recent_session(&root, None).into(),
    );

    let older = write_jsonl("older.jsonl", &format!("{}\n", header_line("old", "/tmp")));
    let newer = write_jsonl("newer.jsonl", &format!("{}\n", header_line("new", "/tmp")));
    pin(&older, t1);
    pin(&newer, t2);
    caps.insert(
        "most-recent".into(),
        find_most_recent_session(&root, None).into(),
    );

    let invalid2 = write_jsonl("invalid2.jsonl", "{\"type\":\"not-session\"}\n");
    let valid2 = write_jsonl("valid2.jsonl", &format!("{}\n", header_line("abc", "/tmp")));
    pin(&invalid2, t1);
    pin(&valid2, t2);
    caps.insert(
        "skips-invalid".into(),
        find_most_recent_session(&root, None).into(),
    );

    let oversized = write_jsonl("oversized.jsonl", &"x".repeat(1024 * 1024 + 1));
    let valid3 = write_jsonl("valid3.jsonl", &format!("{}\n", header_line("abc", "/tmp")));
    pin(&oversized, t2);
    pin(&valid3, t1);
    caps.insert(
        "skips-oversized".into(),
        find_most_recent_session(&root, None).into(),
    );

    let project_a = temp_path(dir.path(), "project-a");
    let project_b = temp_path(dir.path(), "project-b");
    std::fs::create_dir(&project_a).unwrap();
    std::fs::create_dir(&project_b).unwrap();
    let file_a = write_jsonl("a.jsonl", &format!("{}\n", header_line("a", &project_a)));
    let file_b = write_jsonl("b.jsonl", &format!("{}\n", header_line("b", &project_b)));
    pin(&file_a, t1);
    pin(&file_b, t2);
    caps.insert(
        "cwd-a".into(),
        find_most_recent_session(&root, Some(&project_a)).into(),
    );
    caps.insert(
        "cwd-b".into(),
        find_most_recent_session(&root, Some(&project_b)).into(),
    );
    caps.insert(
        "cwd-none".into(),
        find_most_recent_session(&root, None).into(),
    );

    assert_eq!(
        canon(&root, &Value::Object(caps)),
        oracle_canon(&["canon", "most-recent"]),
        "most-recent caps"
    );
    test_id_seam::disable();
}

#[test]
fn default_session_dir_matches_oracle() {
    let agent_dir = std::env::temp_dir().join("pi-sm-oracle-agent-home");
    let _ = std::fs::remove_dir_all(agent_dir.join("sessions"));
    // environment-anchored: the fixed cwd is a capture-machine anchor. The
    // capture pinned upstream-on-win32 with the drive-absolute
    // `C:\oracle-fixed-cwd`; on POSIX that input is relative and upstream
    // node resolves it against the process cwd, so the port's raw name would
    // embed the runner's cwd. Mirror the capture with the root-anchored
    // `/oracle-fixed-cwd` (`C:` is the capture drive anchor) — the encoding
    // then strips the leading separator exactly like the captured `C:\...`
    // form keeps none.
    let (fixed_cwd, expected_path) = if cfg!(windows) {
        (
            "C:\\oracle-fixed-cwd".to_string(),
            oracle()["defaultDir"]["path"]
                .as_str()
                .expect("path")
                .to_string(),
        )
    } else {
        // Same stated rule on the oracle side: host separators become POSIX
        // and the captured drive-anchored dir name maps to the POSIX fixed
        // cwd's encoding.
        (
            "/oracle-fixed-cwd".to_string(),
            oracle()["defaultDir"]["path"]
                .as_str()
                .expect("path")
                .replace('\\', "/")
                .replace("--C--oracle-fixed-cwd--", "--oracle-fixed-cwd--"),
        )
    };
    let created = get_default_session_dir_with(&fixed_cwd, &agent_dir.to_string_lossy());
    assert_eq!(
        created.replace(
            std::env::temp_dir()
                .to_string_lossy()
                .trim_end_matches(std::path::is_separator),
            "<tmp>"
        ),
        expected_path,
        "default session dir path"
    );
    assert!(
        created.starts_with(&agent_dir.to_string_lossy().to_string()),
        "nested under agent dir"
    );
    assert!(std::path::Path::new(&created).exists(), "created on demand");
    let _ = std::fs::remove_dir_all(&agent_dir);
}

// ---------------------------------------------------------------------------
// Oracle: SessionManager file-writing grid
// ---------------------------------------------------------------------------

fn manager_user(text: &str, ts: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: ts,
    })
}

fn manager_assistant(text: &str, ts: i64) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "test".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: usage_fixture(1, 1, 0, 0, 2),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: ts,
    })
}

fn pin_mtime(path: &str, millis: u64) {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open for times");
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis))
        .expect("set mtime");
}

fn id_label_pair(id: &str, label: Option<&str>) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".into(), json!(id));
    if let Some(label) = label {
        object.insert("label".into(), json!(label));
    }
    Value::Object(object)
}

#[test]
fn session_manager_persisted_flows_match_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("mgr");
    let root = dir.path().to_string_lossy().into_owned();
    macro_rules! expect_cap {
        ($key:literal, $value:expr) => {
            assert_canon_matches(&root, &$value, &["canon", concat!("manager.", $key)]);
        };
    }

    // deferred flush: file appears only with the first assistant message
    let flush_dir = temp_path(dir.path(), "flush");
    std::fs::create_dir(&flush_dir).unwrap();
    let project_dir = temp_path(dir.path(), "proj");
    let mut s1 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    s1.append_message(manager_user("first question", 0))
        .unwrap();
    s1.append_message(manager_assistant("first answer", 0))
        .unwrap();
    s1.append_message(manager_user("second question", 0))
        .unwrap();
    s1.append_message(manager_assistant("second answer", 0))
        .unwrap();
    let flush_file = s1.get_session_file().expect("session file").to_string();
    let flush_base = std::path::Path::new(&flush_file)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    // upstream regex: /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_/
    let flush_base_masked = Regexp::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_")
        .unwrap()
        .replace(&flush_base, "<stamp>_")
        .into_owned();
    expect_cap!("flush-file-base", flush_base_masked);
    assert_file_bytes_match(&root, &read_file(&flush_file), "flush");

    // append after flush appends a single line
    s1.append_thinking_level_change("high").unwrap();
    assert_file_bytes_match(&root, &read_file(&flush_file), "append-after-flush");

    // createBranchedSession without assistant: deferred, then single header
    let mut s2 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    let first_id = s2
        .append_message(manager_user("first question", 0))
        .unwrap();
    s2.append_message(manager_assistant("first answer", 0))
        .unwrap();
    s2.append_message(manager_user("second question", 0))
        .unwrap();
    s2.append_message(manager_assistant("second answer", 0))
        .unwrap();
    let branch_file = s2
        .create_branched_session(&first_id)
        .expect("branch")
        .unwrap();
    expect_cap!(
        "branch-no-assistant-exists",
        json!(std::path::Path::new(&branch_file).exists())
    );
    s2.append_custom_entry("preset-state", Some(json!({ "name": "plan" })))
        .unwrap();
    s2.append_message(manager_assistant("new answer", 0))
        .unwrap();
    let branch_content = read_file(&branch_file);
    let branch_lines: Vec<&str> = branch_content.trim().split('\n').collect();
    let branch_headers = branch_lines
        .iter()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["type"] == "session")
        .count();
    expect_cap!("branch-no-assistant-headers", json!(branch_headers));
    let entry_ids: Vec<String> = branch_lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["type"] != "session")
        .filter_map(|record| record["id"].as_str().map(str::to_string))
        .collect();
    let unique = entry_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    expect_cap!(
        "branch-no-assistant-unique-ids",
        json!(unique == entry_ids.len())
    );
    assert_file_bytes_match(&root, &branch_content, "branch-no-assistant");

    // createBranchedSession with assistant: immediate write
    let mut s3 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    s3.append_message(manager_user("first question", 0))
        .unwrap();
    let a2 = s3
        .append_message(manager_assistant("first answer", 0))
        .unwrap();
    s3.append_message(manager_user("second question", 0))
        .unwrap();
    s3.append_message(manager_assistant("second answer", 0))
        .unwrap();
    let branch_file2 = s3.create_branched_session(&a2).expect("branch").unwrap();
    expect_cap!(
        "branch-assistant-exists",
        json!(std::path::Path::new(&branch_file2).exists())
    );
    assert_file_bytes_match(&root, &read_file(&branch_file2), "branch-assistant");

    // labels preserved through createBranchedSession (persisted)
    let mut s4 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    let m1 = s4.append_message(manager_user("hello", 0)).unwrap();
    let m2 = s4.append_message(manager_assistant("hi", 0)).unwrap();
    s4.append_label_change(&m1, Some("important")).unwrap();
    s4.append_label_change(&m2, Some("also-important")).unwrap();
    s4.append_message(manager_user("followup", 0)).unwrap();
    let m3_id = s4.get_entries().last().unwrap().id().unwrap().to_string();
    let branch_file3 = s4.create_branched_session(&m2).expect("branch").unwrap();
    assert_file_bytes_match(&root, &read_file(&branch_file3), "labels-branch");
    let label_caps = vec![
        id_label_pair(&m1, s4.get_label(&m1)),
        id_label_pair(&m2, s4.get_label(&m2)),
        id_label_pair(&m3_id, s4.get_label(&m3_id)),
    ];
    expect_cap!("labels-branch-labels", label_caps);
    assert_canon_matches(&root, &s4.get_tree(), &["canon", "manager.labels-tree"]);

    // label rewiring through a removed label entry
    let mut s5 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    let r1 = s5.append_message(manager_user("hello", 0)).unwrap();
    s5.append_label_change(&r1, Some("checkpoint")).unwrap();
    let model_change_id = s5.append_model_change("anthropic", "claude-test").unwrap();
    let r2 = s5.append_message(manager_user("followup", 0)).unwrap();
    s5.append_message(manager_assistant("done", 0)).unwrap();
    let branch_file5 = s5.create_branched_session(&r2).expect("branch").unwrap();
    let rewire_parent = s5
        .get_entry(&model_change_id)
        .unwrap()
        .parent_id()
        .map(str::to_string);
    expect_cap!("label-rewire-parent", rewire_parent);
    // the branched path (user, label, model change, user) has no assistant, so
    // the write is deferred to the first appended assistant message
    expect_cap!(
        "label-rewire-deferred",
        json!(std::path::Path::new(&branch_file5).exists())
    );
    s5.append_message(manager_assistant("post-fork", 0))
        .unwrap();
    assert_file_bytes_match(&root, &read_file(&branch_file5), "label-rewire");

    // compaction firstKeptEntryId remap when kept entry sits behind a label
    let mut s6 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    let c1 = s6.append_message(manager_user("one", 0)).unwrap();
    s6.append_message(manager_assistant("two", 0)).unwrap();
    s6.append_label_change(&c1, Some("kept-mark")).unwrap();
    let kept = match s6
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::Label(_)))
    {
        Some(SessionEntry::Label(label)) => label.target_id.clone(),
        other => unreachable!("{other:?}"),
    };
    let compaction_id = s6
        .append_compaction("summary", Some(&kept), 100, None, None, None)
        .unwrap();
    s6.append_message(manager_user("three", 0)).unwrap();
    s6.append_message(manager_assistant("four", 0)).unwrap();
    let branch_file6 = s6
        .create_branched_session(&compaction_id)
        .expect("branch")
        .unwrap();
    let parsed6: Vec<Value> = read_file(&branch_file6)
        .trim()
        .split('\n')
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let compaction_kept = parsed6
        .iter()
        .find(|record| record["type"] == "compaction")
        .and_then(|record| record["firstKeptEntryId"].as_str())
        .unwrap()
        == kept;
    expect_cap!("compaction-remap-kept", json!(compaction_kept));
    assert_file_bytes_match(&root, &read_file(&branch_file6), "compaction-remap");

    // in-memory branch semantics
    let mut s7 = SessionManager::in_memory(&process_cwd_string(), None, None).expect("in memory");
    let i1 = s7.append_message(manager_user("1", 0)).unwrap();
    let i2 = s7.append_message(manager_assistant("2", 0)).unwrap();
    s7.append_message(manager_user("3", 0)).unwrap();
    s7.branch(&i2).unwrap();
    let i4 = s7.append_message(manager_user("4", 0)).unwrap();
    let result7 = s7.create_branched_session(&i2).expect("branch");
    expect_cap!("in-memory-branch-result", json!(result7.is_none()));
    let branch_entries: Vec<String> = s7
        .get_entries()
        .iter()
        .map(|entry| entry.id().unwrap().to_string())
        .collect();
    expect_cap!("in-memory-branch-entries", branch_entries);
    expect_cap!(
        "in-memory-branch-ids",
        vec![i1.clone(), i2.clone(), i4.clone()]
    );

    // forkFrom
    let source_path = temp_path(dir.path(), "source.jsonl");
    std::fs::write(
        &source_path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&json!({
                "type": "session", "version": 3, "id": "source-session-id",
                "timestamp": "2025-01-01T00:00:00Z", "cwd": root,
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "type": "message", "id": "entry-1", "parentId": null,
                "timestamp": "2025-01-01T00:00:01Z",
                "message": { "role": "user", "content": "carried over", "timestamp": 1 },
            }))
            .unwrap(),
        ),
    )
    .unwrap();
    let forked = SessionManager::fork_from(
        &source_path,
        &temp_path(dir.path(), "target-cwd"),
        Some(&temp_path(dir.path(), "forks")),
        Some(&NewSessionOptions {
            id: Some("forked-session-id".to_string()),
            parent_session: None,
        }),
    )
    .expect("fork");
    expect_cap!("fork-header-id", forked.get_header().unwrap().id.unwrap());
    expect_cap!(
        "fork-parent",
        forked.get_header().unwrap().parent_session.unwrap()
    );
    assert_file_bytes_match(
        &root,
        &read_file(forked.get_session_file().unwrap()),
        "fork",
    );
    expect_cap!("fork-cwd", forked.get_cwd().to_string());

    // setSessionFile corruption handling
    let corrupt_dir = temp_path(dir.path(), "corrupt");
    std::fs::create_dir(&corrupt_dir).unwrap();
    let empty_file = dir
        .path()
        .join("corrupt")
        .join("empty.jsonl")
        .to_string_lossy()
        .into_owned();
    std::fs::write(&empty_file, "").unwrap();
    let sm_empty = SessionManager::open(&empty_file, Some(&corrupt_dir), None).expect("open");
    assert_file_bytes_match(&root, &read_file(&empty_file), "corrupt-empty-rewritten");
    let reopen_same = SessionManager::open(&empty_file, Some(&corrupt_dir), None)
        .expect("reopen")
        .get_session_id()
        == sm_empty.get_session_id();
    expect_cap!("corrupt-empty-reopen-same-id", json!(reopen_same));
    expect_cap!(
        "corrupt-empty-file-preserved",
        json!(sm_empty.get_session_file() == Some(empty_file.as_str()))
    );

    let no_header_file = dir
        .path()
        .join("corrupt")
        .join("no-header.jsonl")
        .to_string_lossy()
        .into_owned();
    let original_content = "{\"type\":\"message\",\"id\":\"abc\",\"parentId\":\"orphaned\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":\"test\"}}\n";
    std::fs::write(&no_header_file, original_content).unwrap();
    let open_error = SessionManager::open(&no_header_file, Some(&corrupt_dir), None)
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        open_error.map(|text| canon(&root, &text)),
        Some(oracle_error_canon("open-no-header")),
        "open no-header error"
    );
    expect_cap!(
        "corrupt-no-header-unchanged",
        json!(read_file(&no_header_file) == original_content)
    );

    let non_session_file = dir
        .path()
        .join("corrupt")
        .join("not-a-session.log")
        .to_string_lossy()
        .into_owned();
    let log_content = "{\"type\":\"event\",\"data\":\"not a session\"}\n";
    std::fs::write(&non_session_file, log_content).unwrap();
    let log_error = SessionManager::open(&non_session_file, Some(&corrupt_dir), None)
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        log_error.map(|text| canon(&root, &text)),
        Some(oracle_error_canon("open-not-session")),
        "open non-session error"
    );
    expect_cap!(
        "corrupt-log-unchanged",
        json!(read_file(&non_session_file) == log_content)
    );

    // persisted session listing
    let flat_dir = temp_path(dir.path(), "flat");
    std::fs::create_dir(&flat_dir).unwrap();
    let project_a = temp_path(dir.path(), "project-a");
    let project_b = temp_path(dir.path(), "project-b");
    std::fs::create_dir(&project_a).unwrap();
    std::fs::create_dir(&project_b).unwrap();
    let mk_persisted = |cwd: &str, label: &str, t: i64| -> String {
        let mut session = SessionManager::create(cwd, Some(&flat_dir), None).expect("create");
        session
            .append_message(AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text(label.to_string()),
                timestamp: 1000,
            }))
            .unwrap();
        session
            .append_message(manager_assistant(&format!("reply to {label}"), t))
            .unwrap();
        session
            .get_session_file()
            .expect("persisted file")
            .to_string()
    };
    let session_a = mk_persisted(&project_a, "from A", 1_700_000_000_000);
    let session_b = mk_persisted(&project_b, "from B", 1_700_000_050_000);
    pin_mtime(&session_a, 1_700_000_000_000);
    pin_mtime(&session_b, 1_700_000_050_000);

    let list_a = SessionManager::list(&project_a, Some(&flat_dir), None);
    let list_all_flat = SessionManager::list_all(Some(&flat_dir), None);
    expect_cap!("list-a", list_a);
    expect_cap!("list-all", list_all_flat);
    let continued =
        SessionManager::continue_recent(&project_a, Some(&flat_dir)).expect("continue recent");
    expect_cap!(
        "continue-recent",
        continued.get_session_file().unwrap().to_string()
    );

    let progress_events = std::cell::RefCell::new(Vec::new());
    let _listed = SessionManager::list(
        &project_a,
        Some(&flat_dir),
        Some(&mut |loaded, total, _partial| {
            progress_events.borrow_mut().push((loaded, total));
        }),
    );
    expect_cap!("list-progress", progress_events.into_inner());

    let id_of = |file: &str| -> String {
        serde_json::from_str::<Value>(read_file(file).split('\n').next().unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let mut find_by_id = serde_json::Map::new();
    find_by_id.insert(
        "a".into(),
        SessionManager::find_by_id(&project_a, &id_of(&session_a), Some(&flat_dir))
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    if let Some(foreign) =
        SessionManager::find_by_id(&project_a, &id_of(&session_b), Some(&flat_dir))
    {
        find_by_id.insert("foreign".into(), Value::String(foreign));
    }
    find_by_id.insert(
        "b".into(),
        SessionManager::find_by_id(&project_b, &id_of(&session_b), Some(&flat_dir))
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    expect_cap!("find-by-id", Value::Object(find_by_id));

    // usage round-trip through a file-backed reload
    let mut s8 = SessionManager::create(&project_dir, Some(&flush_dir), None).expect("create");
    let root_id = s8.append_message(manager_user("question", 0)).unwrap();
    s8.append_message(manager_assistant("answer", 0)).unwrap();
    s8.append_message(
        serde_json::from_value(json!({
            "role": "toolResult", "toolCallId": "call-1", "toolName": "nested-model",
            "content": [{ "type": "text", "text": "result" }], "isError": false,
            "usage": {
                "input": 10, "output": 20, "cacheRead": 30, "cacheWrite": 40, "totalTokens": 100,
                "cost": { "input": 0.1, "output": 0.2, "cacheRead": 0.3, "cacheWrite": 0.4, "total": 1 },
            },
            "timestamp": 1234,
        }))
        .unwrap(),
    )
    .unwrap();
    s8.append_compaction(
        "summary",
        Some(&root_id),
        100,
        None,
        Some(false),
        Some(big_usage()),
    )
    .unwrap();
    s8.branch_with_summary(
        Some(root_id.as_str()),
        "branch summary",
        None,
        Some(false),
        Some(big_usage()),
    )
    .unwrap();
    let reopened = SessionManager::open(s8.get_session_file().unwrap(), Some(&flush_dir), None)
        .expect("reopen");
    assert_canon_matches(
        &root,
        &reopened.get_entries(),
        &["canon", "manager.usage-roundtrip"],
    );

    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// Oracle: in-memory behavior grid
// ---------------------------------------------------------------------------

#[test]
fn session_manager_in_memory_flows_match_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("mem");
    let in_memory_cwd = "/project";
    macro_rules! expect_cap {
        ($key:literal, $value:expr) => {
            assert_canon_matches("<root>", &$value, &["canon", concat!("in-memory.", $key)]);
        };
    }
    macro_rules! in_memory {
        ($options:expr, $entries:expr) => {
            SessionManager::in_memory(&in_memory_cwd, ($options).as_ref(), $entries)
                .expect("in memory")
        };
    }

    // append + tree traversal
    let mut s1 = in_memory!(None, None);
    let id1 = s1.append_message(user_msg("first")).unwrap();
    let id2 = s1.append_message(assistant_msg("second")).unwrap();
    let id3 = s1.append_message(user_msg("third")).unwrap();
    assert_canon_matches(
        "<root>",
        &s1.get_entries(),
        &["canon", "in-memory.append-entries"],
    );
    expect_cap!(
        "append-parents",
        vec![
            s1.get_entry(&id1).unwrap().parent_id(),
            s1.get_entry(&id2).unwrap().parent_id(),
            s1.get_entry(&id3).unwrap().parent_id(),
        ]
    );
    expect_cap!(
        "leaf-advances",
        vec![
            s1.get_leaf_id() == Some(id3.as_str()),
            s1.get_leaf_entry().unwrap().id() == Some(id3.as_str())
        ]
    );

    let mut s2 = in_memory!(None, None);
    let _t1 = s2.append_message(user_msg("hello")).unwrap();
    let _th1 = s2.append_thinking_level_change("high").unwrap();
    s2.append_message(assistant_msg("response")).unwrap();
    let thinking_entry = s2
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::ThinkingLevelChange(_)))
        .unwrap()
        .clone();
    expect_cap!(
        "thinking-parents",
        vec![thinking_entry.parent_id(), s2.get_entries()[2].parent_id()]
    );

    let mut s3 = in_memory!(None, None);
    s3.append_message(user_msg("hello")).unwrap();
    let mo1 = s3.append_model_change("openai", "gpt-4").unwrap();
    s3.append_message(assistant_msg("response")).unwrap();
    let model_entry = s3
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::ModelChange(_)))
        .unwrap()
        .clone();
    let SessionEntry::ModelChange(model) = &model_entry else {
        unreachable!()
    };
    expect_cap!(
        "model-parents",
        json!([
            model.parent_id.clone(),
            model.provider,
            model.model_id,
            s3.get_entries()[2].parent_id(),
        ])
    );
    let _ = mo1;

    let mut s4 = in_memory!(None, None);
    let c1 = s4.append_message(user_msg("1")).unwrap();
    let c2 = s4.append_message(assistant_msg("2")).unwrap();
    let compaction_id = s4
        .append_compaction(
            "summary",
            Some(&c1),
            1000,
            None,
            Some(false),
            Some(big_usage()),
        )
        .unwrap();
    s4.append_message(user_msg("3")).unwrap();
    let compaction_entry4 = s4
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::Compaction(_)))
        .unwrap()
        .clone();
    let SessionEntry::Compaction(compaction) = &compaction_entry4 else {
        unreachable!()
    };
    expect_cap!(
        "compaction-parents",
        json!([
            compaction.parent_id.clone(),
            compaction.summary,
            compaction.first_kept_entry_id.clone(),
            compaction.tokens_before,
            compaction.usage,
            s4.get_entries()[3].parent_id(),
        ])
    );
    let _ = (c2, compaction_id);

    let mut s5 = in_memory!(None, None);
    let cu1 = s5.append_message(user_msg("hello")).unwrap();
    let _custom_id = s5
        .append_custom_entry("my_data", Some(json!({ "key": "value" })))
        .unwrap();
    s5.append_message(assistant_msg("response")).unwrap();
    let custom_entry5 = s5
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::Custom(_)))
        .unwrap()
        .clone();
    let SessionEntry::Custom(custom) = &custom_entry5 else {
        unreachable!()
    };
    expect_cap!(
        "custom-parents",
        json!([
            custom.parent_id.clone(),
            custom.custom_type,
            custom.data.clone(),
            s5.get_entries()[2].parent_id(),
        ])
    );
    let _ = cu1;

    // getBranch grid
    let mut s6 = in_memory!(None, None);
    expect_cap!("branch-empty", json!(s6.get_branch(None).len()));
    let _b1 = s6.append_message(user_msg("hello")).unwrap();
    expect_cap!(
        "branch-single",
        vec![s6.get_branch(None)[0].id().unwrap().to_string()]
    );
    let b2 = s6.append_message(assistant_msg("2")).unwrap();
    let b3 = s6.append_thinking_level_change("high").unwrap();
    let _b4 = s6.append_message(user_msg("3")).unwrap();
    expect_cap!(
        "branch-full",
        s6.get_branch(None)
            .iter()
            .map(|e: &SessionEntry| e.id().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    expect_cap!(
        "branch-from-mid",
        s6.get_branch(Some(&b2))
            .iter()
            .map(|e: &SessionEntry| e.id().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    let _ = b3;

    // getTree grid
    let mut s7 = in_memory!(None, None);
    expect_cap!("tree-empty", json!(s7.get_tree().len()));
    let tr1 = s7.append_message(user_msg("1")).unwrap();
    let tr2 = s7.append_message(assistant_msg("2")).unwrap();
    let tr3 = s7.append_message(user_msg("3")).unwrap();
    let tree7 = s7.get_tree();
    expect_cap!(
        "tree-linear",
        json!({
            "roots": tree7.len(),
            "rootId": tree7[0].entry.id(),
            "childId": tree7[0].children[0].entry.id(),
            "grandchildId": tree7[0].children[0].children[0].entry.id(),
            "leafChildren": tree7[0].children[0].children[0].children.len(),
        })
    );
    s7.branch(&tr2).unwrap();
    let tr4 = s7.append_message(user_msg("4-branch")).unwrap();
    let tree7b = s7.get_tree();
    let mut child_ids: Vec<String> = tree7b[0].children[0]
        .children
        .iter()
        .map(|c| c.entry.id().unwrap().to_string())
        .collect();
    child_ids.sort();
    expect_cap!(
        "tree-branch",
        json!({ "roots": tree7b.len(), "childCount": tree7b[0].children[0].children.len(), "childIds": child_ids })
    );
    let _ = (tr1, tr3, tr4);

    let mut s8 = in_memory!(None, None);
    s8.append_message(user_msg("root")).unwrap();
    let r2 = s8.append_message(assistant_msg("response")).unwrap();
    s8.branch(&r2).unwrap();
    let _ra = s8.append_message(user_msg("branch-A")).unwrap();
    s8.branch(&r2).unwrap();
    let _rb = s8.append_message(user_msg("branch-B")).unwrap();
    s8.branch(&r2).unwrap();
    let _rc = s8.append_message(user_msg("branch-C")).unwrap();
    let tree8 = s8.get_tree();
    let mut branch_ids: Vec<String> = tree8[0].children[0]
        .children
        .iter()
        .map(|c| c.entry.id().unwrap().to_string())
        .collect();
    branch_ids.sort();
    expect_cap!(
        "tree-multi-branch",
        json!({ "count": tree8[0].children[0].children.len(), "ids": branch_ids })
    );

    let mut s9 = in_memory!(None, None);
    s9.append_message(user_msg("1")).unwrap();
    let d2 = s9.append_message(assistant_msg("2")).unwrap();
    let d3 = s9.append_message(user_msg("3")).unwrap();
    s9.append_message(assistant_msg("4")).unwrap();
    s9.branch(&d2).unwrap();
    let d5 = s9.append_message(user_msg("5")).unwrap();
    s9.append_message(assistant_msg("6")).unwrap();
    s9.branch(&d5).unwrap();
    s9.append_message(user_msg("7")).unwrap();
    let tree9 = s9.get_tree();
    let node2 = &tree9[0].children[0];
    let node5 = node2
        .children
        .iter()
        .find(|c| c.entry.id() == Some(d5.as_str()))
        .unwrap();
    let node3 = node2
        .children
        .iter()
        .find(|c| c.entry.id() == Some(d3.as_str()))
        .unwrap();
    expect_cap!(
        "tree-deep",
        json!({
            "node2Children": node2.children.len(),
            "node5Children": node5.children.len(),
            "node3Children": node3.children.len(),
        })
    );

    // branch / branchWithSummary
    let mut s10 = in_memory!(None, None);
    let z1 = s10.append_message(user_msg("1")).unwrap();
    s10.append_message(assistant_msg("2")).unwrap();
    s10.append_message(user_msg("3")).unwrap();
    s10.branch(&z1).unwrap();
    expect_cap!("branch-leaf", json!(s10.get_leaf_id() == Some(z1.as_str())));
    let z4 = s10.append_message(user_msg("branched")).unwrap();
    let branched_parent = s10
        .get_entries()
        .iter()
        .find(|e| e.id() == Some(z4.as_str()))
        .unwrap()
        .parent_id()
        == Some(z1.as_str());
    expect_cap!("branch-child", json!(branched_parent));

    let mut s11 = in_memory!(None, None);
    let y1 = s11.append_message(user_msg("1")).unwrap();
    s11.append_message(assistant_msg("2")).unwrap();
    let _y3 = s11.append_message(user_msg("3")).unwrap();
    let summary_id = s11
        .branch_with_summary(
            Some(y1.as_str()),
            "Summary of abandoned work",
            None,
            Some(false),
            Some(big_usage()),
        )
        .unwrap();
    let summary_entry = s11
        .get_entries()
        .iter()
        .find(|e| matches!(e, SessionEntry::BranchSummary(_)))
        .unwrap()
        .clone();
    let SessionEntry::BranchSummary(summary) = &summary_entry else {
        unreachable!()
    };
    expect_cap!(
        "branch-summary",
        json!({
            "leaf": s11.get_leaf_id() == Some(summary_id.as_str()),
            "parentId": summary.parent_id.clone(),
            "fromId": summary.from_id,
            "usage": summary.usage,
        })
    );

    // labels
    let mut s12 = in_memory!(None, None);
    let lm = s12.append_message(user_msg("hello")).unwrap();
    expect_cap!("label-initial", json!(s12.get_label(&lm).is_none()));
    let label_id = s12.append_label_change(&lm, Some("checkpoint")).unwrap();
    expect_cap!("label-set", s12.get_label(&lm));
    let label_entry = s12
        .get_entries()
        .iter()
        .find(|e| e.id() == Some(label_id.as_str()))
        .unwrap()
        .clone();
    let SessionEntry::Label(label) = &label_entry else {
        unreachable!()
    };
    expect_cap!(
        "label-entry",
        json!({ "targetId": label.target_id, "label": label.label })
    );
    s12.append_label_change(&lm, None).unwrap();
    expect_cap!("label-cleared", json!(s12.get_label(&lm).is_none()));

    let mut s13 = in_memory!(None, None);
    let lm13 = s13.append_message(user_msg("hello")).unwrap();
    s13.append_label_change(&lm13, Some("first")).unwrap();
    s13.append_label_change(&lm13, Some("second")).unwrap();
    let last_label_id = s13.append_label_change(&lm13, Some("third")).unwrap();
    let last_label_entry = s13
        .get_entries()
        .iter()
        .find(|e| e.id() == Some(last_label_id.as_str()))
        .unwrap()
        .clone();
    let msg_node13 = s13
        .get_tree()
        .iter()
        .find(|n| n.entry.id() == Some(lm13.as_str()))
        .unwrap()
        .clone();
    expect_cap!(
        "label-last-wins",
        json!({
            "label": s13.get_label(&lm13),
            "tsMatches": msg_node13.label_timestamp.as_deref() == Some(last_label_entry.timestamp()),
        })
    );

    let mut s14 = in_memory!(None, None);
    let lm14a = s14.append_message(user_msg("hello")).unwrap();
    let lm14b = s14.append_message(assistant_msg("hi")).unwrap();
    let lb14a = s14.append_label_change(&lm14a, Some("start")).unwrap();
    let lb14b = s14.append_label_change(&lm14b, Some("response")).unwrap();
    let entries14 = s14.get_entries();
    let lab14a = entries14
        .iter()
        .find(|e| e.id() == Some(lb14a.as_str()))
        .unwrap();
    let lab14b = entries14
        .iter()
        .find(|e| e.id() == Some(lb14b.as_str()))
        .unwrap();
    let tree14 = s14.get_tree();
    let node14a = tree14
        .iter()
        .find(|n| n.entry.id() == Some(lm14a.as_str()))
        .unwrap();
    let node14b = node14a
        .children
        .iter()
        .find(|n| n.entry.id() == Some(lm14b.as_str()))
        .unwrap();
    expect_cap!(
        "label-tree",
        json!({
            "labelA": node14a.label,
            "tsA": node14a.label_timestamp.as_deref() == Some(lab14a.timestamp()),
            "labelB": node14b.label,
            "tsB": node14b.label_timestamp.as_deref() == Some(lab14b.timestamp()),
        })
    );

    let mut s15 = in_memory!(None, None);
    let lm15a = s15.append_message(user_msg("hello")).unwrap();
    let lm15b = s15.append_message(assistant_msg("hi")).unwrap();
    let lb15a = s15.append_label_change(&lm15a, Some("important")).unwrap();
    let lb15b = s15
        .append_label_change(&lm15b, Some("also-important"))
        .unwrap();
    let entries15 = s15.get_entries();
    let lab15a = entries15
        .iter()
        .find(|e| e.id() == Some(lb15a.as_str()))
        .unwrap();
    let lab15b = entries15
        .iter()
        .find(|e| e.id() == Some(lb15b.as_str()))
        .unwrap();
    s15.create_branched_session(&lm15b).unwrap();
    let tree15 = s15.get_tree();
    let node15a = tree15
        .iter()
        .find(|n| n.entry.id() == Some(lm15a.as_str()))
        .unwrap();
    let node15b = node15a
        .children
        .iter()
        .find(|n| n.entry.id() == Some(lm15b.as_str()))
        .unwrap();
    expect_cap!(
        "label-branch-inmemory",
        json!({
            "labelA": s15.get_label(&lm15a),
            "labelB": s15.get_label(&lm15b),
            "labelEntries": s15.get_entries().iter().filter(|e| matches!(e, SessionEntry::Label(_))).count(),
            "tsA": node15a.label_timestamp.as_deref() == Some(lab15a.timestamp()),
            "tsB": node15b.label_timestamp.as_deref() == Some(lab15b.timestamp()),
        })
    );

    let mut s16 = in_memory!(None, None);
    let lx = s16.append_message(user_msg("hello")).unwrap();
    s16.append_label_change(&lx, Some("checkpoint")).unwrap();
    let ctx16 = s16.build_session_context();
    expect_cap!(
        "label-not-in-context",
        json!({
            "count": ctx16.messages.len(),
            "role": ctx16.messages.first().map(AgentMessage::role),
        })
    );

    // session_info / getSessionName
    let mut s17 = in_memory!(None, None);
    expect_cap!("session-name-none", json!(s17.get_session_name().is_none()));
    s17.append_session_info("  my\nname  ").unwrap();
    expect_cap!("session-name", s17.get_session_name());
    s17.append_session_info("   ").unwrap();
    expect_cap!(
        "session-name-cleared",
        json!(s17.get_session_name().is_none())
    );

    // inMemory preloaded entries
    let _build_stored = |build: &dyn Fn(&mut SessionManager)| {
        let mut source = in_memory!(None, None);
        build(&mut source);
        source
    };
    let mut entries_a_manager = in_memory!(None, None);
    entries_a_manager.append_message(user_msg("hello")).unwrap();
    entries_a_manager
        .append_model_change("anthropic", "claude-opus-4-5")
        .unwrap();
    entries_a_manager.append_message(user_msg("again")).unwrap();
    let entries_a = entries_a_manager.get_entries();
    let restored_a = in_memory!(None, Some(entries_to_file_entries(entries_a.clone())));
    expect_cap!(
        "preload-verbatim",
        json!(canon("<root>", &restored_a.get_entries()) == canon("<root>", &entries_a))
    );
    let mut entries_b_manager = in_memory!(None, None);
    entries_b_manager.append_message(user_msg("hello")).unwrap();
    entries_b_manager.append_message(user_msg("again")).unwrap();
    let entries_b = entries_b_manager.get_entries();
    let last_b = entries_b.last().unwrap().id().unwrap().to_string();
    let mut restored_b = in_memory!(None, Some(entries_to_file_entries(entries_b.clone())));
    let appended_b = restored_b.append_message(user_msg("continued")).unwrap();
    expect_cap!(
        "preload-leaf",
        json!({
            "leaf": restored_b.get_leaf_id() == Some(appended_b.as_str()),
            "parent": restored_b.get_entry(&appended_b).unwrap().parent_id() == Some(last_b.as_str()),
        })
    );

    let mut entries_c_manager = in_memory!(None, None);
    for i in 0..50 {
        entries_c_manager
            .append_message(user_msg(&format!("message {i}")))
            .unwrap();
    }
    let entries_c = entries_c_manager.get_entries();
    let mut restored_c = in_memory!(None, Some(entries_to_file_entries(entries_c.clone())));
    let appended_c = restored_c.append_message(user_msg("continued")).unwrap();
    expect_cap!(
        "preload-no-collision",
        json!(!entries_c
            .iter()
            .any(|entry| entry.id() == Some(appended_c.as_str())))
    );

    let mut entries_d_manager = in_memory!(None, None);
    let first_d = entries_d_manager.append_message(user_msg("hello")).unwrap();
    entries_d_manager
        .append_message(user_msg("abandoned"))
        .unwrap();
    entries_d_manager.branch(&first_d).unwrap();
    entries_d_manager.append_message(user_msg("kept")).unwrap();
    let entries_d = entries_d_manager.get_entries();
    let restored_d = in_memory!(None, Some(entries_to_file_entries(entries_d)));
    let roots_d = restored_d.get_tree();
    expect_cap!(
        "preload-tree",
        json!({ "roots": roots_d.len(), "children": roots_d[0].children.len() })
    );

    let mut entries_e_manager = in_memory!(None, None);
    let labelled_e = entries_e_manager.append_message(user_msg("hello")).unwrap();
    entries_e_manager
        .append_label_change(&labelled_e, Some("checkpoint"))
        .unwrap();
    let entries_e = entries_e_manager.get_entries();
    let restored_e = in_memory!(None, Some(entries_to_file_entries(entries_e)));
    expect_cap!("preload-labels", restored_e.get_label(&labelled_e));

    let mut entries_f_manager = in_memory!(None, None);
    entries_f_manager
        .append_message(user_msg("dropped"))
        .unwrap();
    let kept_f = entries_f_manager.append_message(user_msg("kept")).unwrap();
    entries_f_manager
        .append_compaction("summary so far", Some(&kept_f), 1000, None, None, None)
        .unwrap();
    let entries_f = entries_f_manager.get_entries();
    let restored_f = in_memory!(None, Some(entries_to_file_entries(entries_f)));
    let context_f = restored_f.build_context_entries();
    expect_cap!(
        "preload-compaction",
        json!(context_f
            .iter()
            .any(|entry| entry.id() == Some(kept_f.as_str())))
    );

    let mut entries_g_manager = in_memory!(None, None);
    entries_g_manager.append_message(user_msg("hello")).unwrap();
    let entries_g = entries_g_manager.get_entries();
    let restored_g = in_memory!(
        Some(NewSessionOptions {
            id: Some("restored-session".to_string()),
            parent_session: None,
        }),
        Some(entries_to_file_entries(entries_g))
    );
    expect_cap!(
        "preload-header-from-options",
        json!({
            "id": restored_g.get_session_id(),
            "headerId": restored_g.get_header().unwrap().id,
            "cwd": restored_g.get_header().unwrap().cwd,
        })
    );

    let mut entries_i_manager = in_memory!(None, None);
    entries_i_manager.append_message(user_msg("hello")).unwrap();
    let entries_i = entries_i_manager.get_entries();
    let mut restored_i = in_memory!(None, Some(entries_to_file_entries(entries_i)));
    restored_i.append_message(user_msg("continued")).unwrap();
    expect_cap!(
        "preload-off-fs",
        json!({
            "file": restored_i.get_session_file().is_none(),
            "persisted": !restored_i.is_persisted(),
        })
    );

    let restored_j = in_memory!(
        Some(NewSessionOptions {
            id: Some("empty-session".to_string()),
            parent_session: None,
        }),
        Some(vec![])
    );
    expect_cap!(
        "preload-empty",
        json!({
            "id": restored_j.get_session_id(),
            "entries": restored_j.get_entries().len(),
            "leaf": restored_j.get_leaf_id().is_none(),
        })
    );

    let mut body_manager = in_memory!(None, None);
    body_manager.append_message(user_msg("hello")).unwrap();
    let body = body_manager.get_entries();
    let mut entries_k = vec![FileEntry::Session(SessionHeader {
        version: Some(3),
        id: Some("stored-session".to_string()),
        timestamp: Some("2026-01-01T00:00:00Z".to_string()),
        cwd: Some("/stored".to_string()),
        parent_session: None,
    })];
    entries_k.extend(body.into_iter().map(FileEntry::Entry));
    let restored_k = in_memory!(
        Some(NewSessionOptions {
            id: Some("ignored".to_string()),
            parent_session: None,
        }),
        Some(entries_k)
    );
    expect_cap!(
        "preload-header-identity",
        json!({
            "id": restored_k.get_session_id(),
            "cwd": restored_k.get_header().unwrap().cwd,
        })
    );

    let entries_l = vec![
        FileEntry::Session(SessionHeader {
            version: Some(2),
            id: Some("v2-session".to_string()),
            timestamp: Some("2026-01-01T00:00:00Z".to_string()),
            cwd: Some("/project".to_string()),
            parent_session: None,
        }),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "hookmsgid", "parentId": null,
                "timestamp": "2026-01-01T00:00:01Z",
                "message": { "role": "hookMessage", "content": "from a hook", "timestamp": 1 },
            }))
            .expect("fixture entry"),
        ),
    ];
    let restored_l = in_memory!(None, Some(entries_l));
    expect_cap!(
        "preload-migrated",
        json!({
            "version": restored_l.get_header().unwrap().version,
            "role": entry_message_role(&restored_l.get_entries()[0]),
            "id": restored_l.get_entries()[0].id(),
        })
    );

    let entries_m = vec![FileEntry::Unparsed(json!({
        "type": "message", "id": "hookmsgid", "parentId": null,
        "timestamp": "2026-01-01T00:00:01Z",
        "message": { "role": "hookMessage", "content": "from a hook", "timestamp": 1 },
    }))];
    let restored_m = in_memory!(None, Some(entries_m));
    expect_cap!(
        "preload-headerless-not-migrated",
        entry_message_role(&restored_m.get_entries()[0])
    );

    // custom id grid
    let mut s20 = in_memory!(None, None);
    s20.new_session(
        Some(NewSessionOptions {
            id: Some("my-custom-id".to_string()),
            parent_session: None,
        })
        .as_ref(),
    )
    .unwrap();
    expect_cap!("custom-id", s20.get_session_id().to_string());
    let s21 = in_memory!(
        Some(NewSessionOptions {
            id: Some("memory-session-id".to_string()),
            parent_session: None,
        }),
        None
    );
    expect_cap!(
        "custom-id-memory",
        json!({
            "id": s21.get_session_id(),
            "header": s21.get_header().unwrap().id,
            "fileUndefined": s21.get_session_file().is_none(),
        })
    );
    let mut s22 = in_memory!(None, None);
    s22.new_session(
        Some(NewSessionOptions {
            id: Some("abc-123_def.456".to_string()),
            parent_session: None,
        })
        .as_ref(),
    )
    .unwrap();
    expect_cap!("custom-id-punctuation", s22.get_session_id().to_string());
    let invalid_ids = [
        "", "-abc", "abc-", "_abc", "abc_", ".abc", "abc.", "abc/def", "abc\\def", "abc def",
    ];
    let invalid_count = invalid_ids
        .iter()
        .filter(|id| {
            let mut session = in_memory!(None, None);
            session
                .new_session(
                    Some(NewSessionOptions {
                        id: Some((*id).to_string()),
                        parent_session: None,
                    })
                    .as_ref(),
                )
                .is_err()
        })
        .count();
    expect_cap!("custom-id-invalid-count", json!(invalid_count));
    let invalid_error = SessionManager::in_memory(
        in_memory_cwd,
        Some(NewSessionOptions {
            id: Some(String::new()),
            parent_session: None,
        })
        .as_ref(),
        None,
    )
    .err()
    .map(|e| e.to_string())
    .unwrap_or_default();
    assert_eq!(invalid_error, oracle_error("invalid-session-id"));
    let mut s25 = in_memory!(None, None);
    s25.new_session(
        Some(NewSessionOptions {
            id: Some("header-test-id".to_string()),
            parent_session: None,
        })
        .as_ref(),
    )
    .unwrap();
    expect_cap!("custom-id-header", s25.get_header().unwrap().id.unwrap());

    let created_dir = temp_path(dir.path(), "created");
    std::fs::create_dir(&created_dir).unwrap();
    let s27 = SessionManager::create(
        &created_dir,
        Some(&created_dir),
        Some(&NewSessionOptions {
            id: Some("created-session-id".to_string()),
            parent_session: None,
        }),
    )
    .expect("create");
    let created_file_base = std::path::Path::new(s27.get_session_file().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let created_base_masked = Regexp::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_")
        .unwrap()
        .replace(&created_file_base, "<stamp>_")
        .into_owned();
    expect_cap!(
        "created-custom-id",
        json!({
            "id": s27.get_session_id(),
            "header": s27.get_header().unwrap().id,
            "fileBase": created_base_masked,
            "exists": std::path::Path::new(s27.get_session_file().unwrap()).exists(),
        })
    );

    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// Oracle: error messages
// ---------------------------------------------------------------------------

#[test]
fn session_manager_errors_match_oracle() {
    test_id_seam::reset();
    let dir = scenario_dir("err");
    let root = dir.path().to_string_lossy().into_owned();

    let assert_error = assert_valid_session_id("-bad-")
        .err()
        .map(|e: SessionManagerError| e.to_string());
    assert_eq!(
        assert_error.as_deref(),
        Some(oracle_error("assert-session-id"))
    );
    assert!(assert_valid_session_id("valid-id.123").is_ok());

    let mut s = SessionManager::in_memory(&process_cwd_string(), None, None).expect("in memory");
    s.append_message(user_msg("hello")).unwrap();
    let branch_error = s.branch("nonexistent").err().map(|e| e.to_string());
    assert_eq!(
        branch_error.as_deref(),
        Some(oracle_error("branch-not-found"))
    );
    let summary_error = s
        .branch_with_summary(Some("nonexistent"), "summary", None, None, None)
        .err();
    assert_eq!(
        summary_error.map(|e| e.to_string()).as_deref(),
        Some(oracle_error("branch-summary-not-found"))
    );
    let label_error = s.append_label_change("non-existent", Some("label")).err();
    assert_eq!(
        label_error.map(|e| e.to_string()).as_deref(),
        Some(oracle_error("label-not-found"))
    );
    let branched_error = s.create_branched_session("nonexistent").err();
    assert_eq!(
        branched_error.map(|e| e.to_string()).as_deref(),
        Some(oracle_error("branched-session-not-found"))
    );

    let empty_source = temp_path(dir.path(), "empty-source.jsonl");
    std::fs::write(&empty_source, "").unwrap();
    let fork_empty = SessionManager::fork_from(&empty_source, &root, Some(&root), None)
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        fork_empty.map(|text| canon(&root, &text)),
        Some(oracle_error_canon("fork-empty")),
    );

    let headerless_source = temp_path(dir.path(), "headerless.jsonl");
    std::fs::write(
        &headerless_source,
        "{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2025-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"x\",\"timestamp\":1}}\n",
    )
    .unwrap();
    let fork_no_header = SessionManager::fork_from(&headerless_source, &root, Some(&root), None)
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        fork_no_header.map(|text| canon(&root, &text)),
        Some(oracle_error_canon("fork-no-header")),
    );
    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// Loose-entry helpers
// ---------------------------------------------------------------------------

fn loose_entry(value: Value) -> SessionEntry {
    match super::file_entry_from_value(value) {
        FileEntry::Entry(entry) => entry,
        FileEntry::Unparsed(value) => SessionEntry::Unparsed(value),
        FileEntry::Session(_) => unreachable!(),
    }
}

fn entries_to_file_entries(entries: Vec<SessionEntry>) -> Vec<FileEntry> {
    entries.into_iter().map(FileEntry::Entry).collect()
}

fn entry_kind(entry: &SessionEntry) -> Option<&'static str> {
    match entry {
        SessionEntry::Message(_) => Some("message"),
        SessionEntry::ThinkingLevelChange(_) => Some("thinking_level_change"),
        SessionEntry::ModelChange(_) => Some("model_change"),
        SessionEntry::Compaction(_) => Some("compaction"),
        SessionEntry::BranchSummary(_) => Some("branch_summary"),
        SessionEntry::Custom(_) => Some("custom"),
        SessionEntry::CustomMessage(_) => Some("custom_message"),
        SessionEntry::Usage(_) => Some("usage"),
        SessionEntry::ContextEdit(_) => Some("context_edit"),
        SessionEntry::Label(_) => Some("label"),
        SessionEntry::SessionInfo(_) => Some("session_info"),
        SessionEntry::Unparsed(_) => None,
    }
}

fn entry_message_role(entry: &SessionEntry) -> String {
    match entry {
        SessionEntry::Message(message) => message.message.role().to_string(),
        SessionEntry::Unparsed(value) => value
            .get("message")
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/build-context.test.ts
// ---------------------------------------------------------------------------

#[test]
fn build_context_trivial_cases() {
    // empty entries returns empty context
    let ctx = build_session_context(&[], LeafRef::Unspecified);
    assert!(ctx.messages.is_empty());
    assert_eq!(ctx.thinking_level, "off");
    assert!(ctx.model.is_none());

    // single user message
    let single = [msg_entry("m1", None, "user", "hello")];
    let ctx = build_session_context(&single, LeafRef::Unspecified);
    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(ctx.messages[0].role(), "user");

    // simple conversation
    let simple = [
        msg_entry("m1", None, "user", "hello"),
        msg_entry("m2", Some("m1"), "assistant", "hi there"),
        msg_entry("m3", Some("m2"), "user", "how are you"),
        msg_entry("m4", Some("m3"), "assistant", "great"),
    ];
    let ctx = build_session_context(&simple, LeafRef::Unspecified);
    assert_eq!(ctx.messages.len(), 4);
    let roles: Vec<&str> = ctx.messages.iter().map(AgentMessage::role).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);

    // tracks thinking level changes
    let with_thinking = [
        msg_entry("m1", None, "user", "hello"),
        thinking_level_entry("t1", Some("m1"), "high"),
        msg_entry("m2", Some("t1"), "assistant", "thinking hard"),
    ];
    let ctx = build_session_context(&with_thinking, LeafRef::Unspecified);
    assert_eq!(ctx.thinking_level, "high");
    assert_eq!(ctx.messages.len(), 2);

    // tracks model from assistant message / model change overwritten
    let from_assistant = [
        msg_entry("m1", None, "user", "hello"),
        msg_entry("m2", Some("m1"), "assistant", "hi"),
    ];
    let ctx = build_session_context(&from_assistant, LeafRef::Unspecified);
    assert_eq!(
        ctx.model,
        Some(super::SessionContextModel {
            provider: "anthropic".to_string(),
            model_id: "claude-test".to_string()
        })
    );
    let with_change = [
        msg_entry("m1", None, "user", "hello"),
        model_change_entry("mc1", Some("m1"), "openai", "gpt-4"),
        msg_entry("m2", Some("mc1"), "assistant", "hi"),
    ];
    let ctx = build_session_context(&with_change, LeafRef::Unspecified);
    assert_eq!(
        ctx.model,
        Some(super::SessionContextModel {
            provider: "anthropic".to_string(),
            model_id: "claude-test".to_string()
        })
    );
}

#[test]
fn build_context_edge_cases() {
    // uses last entry when leafId not found
    let base = [
        msg_entry("m1", None, "user", "hello"),
        msg_entry("m2", Some("m1"), "assistant", "hi"),
    ];
    let ctx = build_session_context(&base, LeafRef::Id("nonexistent"));
    assert_eq!(ctx.messages.len(), 2);

    // handles orphaned entries gracefully
    let orphaned = [
        msg_entry("m1", None, "user", "hello"),
        msg_entry("m2", Some("missing"), "assistant", "orphan"),
    ];
    let ctx = build_session_context(&orphaned, LeafRef::Id("m2"));
    assert_eq!(ctx.messages.len(), 1);
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/custom-session-id.test.ts
// ---------------------------------------------------------------------------

fn uuid_v7_regexp() -> &'static Regexp {
    static RE: OnceLock<Regexp> = OnceLock::new();
    RE.get_or_init(|| {
        Regexp::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
            .expect("uuid v7 regex")
    })
}

#[test]
fn new_session_with_custom_ids() {
    let cwd = process_cwd_string();
    // uses the provided id instead of generating one
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session
        .new_session(Some(&NewSessionOptions {
            id: Some("my-custom-id".to_string()),
            parent_session: None,
        }))
        .unwrap();
    assert_eq!(session.get_session_id(), "my-custom-id");

    // uses the provided id when creating an in-memory session
    let session = SessionManager::in_memory(
        &cwd,
        Some(NewSessionOptions {
            id: Some("memory-session-id".to_string()),
            parent_session: None,
        })
        .as_ref(),
        None,
    )
    .unwrap();
    assert_eq!(session.get_session_id(), "memory-session-id");
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some("memory-session-id")
    );
    assert!(session.get_session_file().is_none());

    // allows alphanumeric session ids with interior punctuation
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session
        .new_session(Some(&NewSessionOptions {
            id: Some("abc-123_def.456".to_string()),
            parent_session: None,
        }))
        .unwrap();
    assert_eq!(session.get_session_id(), "abc-123_def.456");

    // rejects invalid custom session ids
    for id in [
        "", "-abc", "abc-", "_abc", "abc_", ".abc", "abc.", "abc/def", "abc\\def", "abc def",
    ] {
        let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
        let error = session
            .new_session(Some(&NewSessionOptions {
                id: Some(id.to_string()),
                parent_session: None,
            }))
            .expect_err("invalid id rejected");
        assert!(
            error
                .to_string()
                .starts_with("Session id must be non-empty, contain only alphanumeric characters"),
            "error for {id:?}"
        );
    }

    // includes the custom id in the session header
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session
        .new_session(Some(&NewSessionOptions {
            id: Some("header-test-id".to_string()),
            parent_session: None,
        }))
        .unwrap();
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some("header-test-id")
    );
}

#[test]
fn new_session_generates_uuid_v7_ids() {
    test_id_seam::disable();
    let cwd = process_cwd_string();
    // generates a UUIDv7 id when no id is provided
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session.new_session(None).unwrap();
    assert!(
        uuid_v7_regexp().is_match(session.get_session_id()),
        "{}",
        session.get_session_id()
    );

    // generates a UUIDv7 id when options is provided without id
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session
        .new_session(Some(&NewSessionOptions {
            id: None,
            parent_session: Some("parent.jsonl".to_string()),
        }))
        .unwrap();
    assert!(uuid_v7_regexp().is_match(session.get_session_id()));

    // generates a UUIDv7 id when constructed without an explicit id
    let session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert!(uuid_v7_regexp().is_match(session.get_session_id()));
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some(session.get_session_id())
    );
}

#[test]
fn persisted_sessions_use_provided_and_generated_ids() {
    test_id_seam::disable();
    let dir = scenario_dir("custom-persisted");
    let temp = dir.path().to_string_lossy().into_owned();
    let session = SessionManager::create(
        &temp,
        Some(&temp),
        Some(NewSessionOptions {
            id: Some("created-session-id".to_string()),
            parent_session: None,
        })
        .as_ref(),
    )
    .unwrap();
    assert_eq!(session.get_session_id(), "created-session-id");
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some("created-session-id")
    );
    let session_file = session.get_session_file().unwrap();
    assert!(session_file.contains("created-session-id"));
    let base = std::path::Path::new(session_file)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        Regexp::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_created-session-id\.jsonl$")
            .unwrap()
            .is_match(&base),
        "{base}"
    );
    assert!(
        !std::path::Path::new(session_file).exists(),
        "deferred until first assistant"
    );
}

#[test]
fn branched_and_forked_sessions_generate_uuid_v7_ids() {
    test_id_seam::disable();
    let cwd = process_cwd_string();
    // generates a UUIDv7 id when creating a branched session
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let first_id = session
        .append_message(AgentMessage::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                text: "hello".to_string(),
                text_signature: None,
            })]),
            timestamp: crate::ai::now_ms(),
        }))
        .unwrap();
    session.create_branched_session(&first_id).unwrap();
    assert!(uuid_v7_regexp().is_match(session.get_session_id()));
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some(session.get_session_id())
    );

    // forked sessions generate a UUIDv7 id and carry parentSession
    let dir = scenario_dir("fork-ids");
    let temp = dir.path().to_string_lossy().into_owned();
    let source_path = temp_path(dir.path(), "source.jsonl");
    let assistant_entry = json!({
        "type": "message", "id": "entry-1", "parentId": null,
        "timestamp": "2025-01-01T00:00:00Z",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": "hello" }],
            "api": "openai-responses",
            "provider": "openai",
            "model": "gpt-5.4",
            "usage": {
                "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            },
            "stopReason": "stop",
            "timestamp": 0,
        },
    });
    std::fs::write(
        &source_path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&json!({
                "type": "session", "version": 3, "id": "legacy-session-id",
                "timestamp": "2025-01-01T00:00:00Z", "cwd": temp,
            }))
            .unwrap(),
            serde_json::to_string(&assistant_entry).unwrap(),
        ),
    )
    .unwrap();
    let forked = SessionManager::fork_from(&source_path, &temp, Some(&temp), None).unwrap();
    let header = forked.get_header().unwrap();
    assert!(uuid_v7_regexp().is_match(header.id.as_deref().unwrap()));
    assert_eq!(header.parent_session.as_deref(), Some(source_path.as_str()));

    // uses the provided id when forking from another session file
    let source_path2 = temp_path(dir.path(), "source2.jsonl");
    std::fs::write(
        &source_path2,
        format!(
            "{}\n",
            serde_json::to_string(&json!({
                "type": "session", "version": 3, "id": "source-session-id",
                "timestamp": "2025-01-01T00:00:00Z", "cwd": temp,
            }))
            .unwrap(),
        ),
    )
    .unwrap();
    let forked = SessionManager::fork_from(
        &source_path2,
        &temp,
        Some(&temp),
        Some(NewSessionOptions {
            id: Some("forked-session-id".to_string()),
            parent_session: None,
        })
        .as_ref(),
    )
    .unwrap();
    let header = forked.get_header().unwrap();
    assert_eq!(header.id.as_deref(), Some("forked-session-id"));
    assert_eq!(
        header.parent_session.as_deref(),
        Some(source_path2.as_str())
    );
    let session_file = forked.get_session_file().unwrap();
    assert!(session_file.contains("forked-session-id"));
    let base = std::path::Path::new(session_file)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        Regexp::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_forked-session-id\.jsonl$")
            .unwrap()
            .is_match(&base),
        "{base}"
    );
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/file-operations.test.ts (remaining cases)
// ---------------------------------------------------------------------------

#[test]
fn find_most_recent_session_upstream_cases() {
    let dir = scenario_dir("recent-upstream");
    let root = dir.path().to_string_lossy().into_owned();
    let header = r#"{"type":"session","id":"abc","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}"#;

    // returns null for empty directory / non-existent directory
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    assert_eq!(
        find_most_recent_session(&temp_path(dir.path(), "empty"), None),
        None
    );
    assert_eq!(
        find_most_recent_session(&temp_path(dir.path(), "nonexistent"), None),
        None
    );

    // ignores non-jsonl files
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    std::fs::write(dir.path().join("file.json"), "{}").unwrap();
    assert_eq!(find_most_recent_session(&root, None), None);

    // ignores jsonl files without valid session header
    std::fs::write(dir.path().join("invalid.jsonl"), "{\"type\":\"message\"}\n").unwrap();
    assert_eq!(find_most_recent_session(&root, None), None);

    // returns single valid session file
    let single = temp_path(dir.path(), "session.jsonl");
    std::fs::write(&single, format!("{header}\n")).unwrap();
    assert_eq!(
        find_most_recent_session(&root, None).as_deref(),
        Some(single.as_str())
    );

    // returns most recently modified session (mtime-pinned)
    let older = temp_path(dir.path(), "older.jsonl");
    let newer = temp_path(dir.path(), "newer.jsonl");
    std::fs::write(&older, format!("{header}\n")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&newer, format!("{header}\n")).unwrap();
    assert_eq!(
        find_most_recent_session(&root, None).as_deref(),
        Some(newer.as_str())
    );

    // skips invalid files and returns valid one
    let invalid = temp_path(dir.path(), "invalid2.jsonl");
    let valid = temp_path(dir.path(), "valid.jsonl");
    std::fs::write(&invalid, "{\"type\":\"not-session\"}\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&valid, format!("{header}\n")).unwrap();
    assert_eq!(
        find_most_recent_session(&root, None).as_deref(),
        Some(valid.as_str())
    );

    // skips oversized corrupt files and returns a valid session
    let oversized = temp_path(dir.path(), "oversized.jsonl");
    let valid3 = temp_path(dir.path(), "valid3.jsonl");
    std::fs::write(&oversized, "x".repeat(1024 * 1024 + 1)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&valid3, format!("{header}\n")).unwrap();
    assert_eq!(
        find_most_recent_session(&root, None).as_deref(),
        Some(valid3.as_str())
    );

    // filters most recent session by cwd
    let project_a = temp_path(dir.path(), "project-a");
    let project_b = temp_path(dir.path(), "project-b");
    std::fs::create_dir(&project_a).unwrap();
    std::fs::create_dir(&project_b).unwrap();
    let file_a = temp_path(dir.path(), "a.jsonl");
    let file_b = temp_path(dir.path(), "b.jsonl");
    std::fs::write(
        &file_a,
        format!("{}\n", json!({ "type": "session", "id": "a", "timestamp": "2025-01-01T00:00:00Z", "cwd": project_a })),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        &file_b,
        format!("{}\n", json!({ "type": "session", "id": "b", "timestamp": "2025-01-01T00:00:00Z", "cwd": project_b })),
    )
    .unwrap();
    assert_eq!(
        find_most_recent_session(&root, Some(&project_a)).as_deref(),
        Some(file_a.as_str())
    );
    assert_eq!(
        find_most_recent_session(&root, Some(&project_b)).as_deref(),
        Some(file_b.as_str())
    );
}

#[test]
fn custom_flat_session_directory_scopes_by_cwd() {
    test_id_seam::disable();
    let dir = scenario_dir("flat");
    let temp = dir.path().to_string_lossy().into_owned();
    let project_a = temp_path(dir.path(), "project-a");
    let project_b = temp_path(dir.path(), "project-b");
    std::fs::create_dir(&project_a).unwrap();
    std::fs::create_dir(&project_b).unwrap();

    let create_persisted = |cwd: &str, label: &str| -> String {
        let mut session = SessionManager::create(cwd, Some(&temp), None).unwrap();
        session
            .append_message(AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text(label.to_string()),
                timestamp: crate::ai::now_ms(),
            }))
            .unwrap();
        session
            .append_message(assistant_msg(&format!("reply to {label}")))
            .unwrap();
        session
            .get_session_file()
            .expect("persisted file")
            .to_string()
    };
    let session_a = create_persisted(&project_a, "from A");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let session_b = create_persisted(&project_b, "from B");

    let current_a = SessionManager::list(&project_a, Some(&temp), None);
    let paths: Vec<String> = current_a.iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, vec![session_a.clone()]);

    let all = SessionManager::list_all(Some(&temp), None);
    let mut all_paths: Vec<String> = all.iter().map(|s| s.path.clone()).collect();
    all_paths.sort();
    let mut expected = vec![session_a.clone(), session_b.clone()];
    expected.sort();
    assert_eq!(all_paths, expected);

    let continued = SessionManager::continue_recent(&project_a, Some(&temp)).unwrap();
    assert_eq!(continued.get_session_file(), Some(session_a.as_str()));
}

#[test]
fn set_session_file_with_corrupted_files() {
    test_id_seam::reset();
    let dir = scenario_dir("corrupt");
    let temp = dir.path().to_string_lossy().into_owned();

    // truncates and rewrites empty file with valid header
    let empty_file = temp_path(dir.path(), "empty.jsonl");
    std::fs::write(&empty_file, "").unwrap();
    let sm = SessionManager::open(&empty_file, Some(&temp), None).unwrap();
    assert!(!sm.get_session_id().is_empty());
    let header = sm.get_header().unwrap();
    assert_eq!(header.id, Some(sm.get_session_id().to_string()));
    let content = read_file(&empty_file);
    let lines: Vec<&str> = content
        .trim()
        .split('\n')
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(lines.len(), 1);
    let parsed: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed["type"], "session");
    assert_eq!(parsed["id"].as_str(), Some(sm.get_session_id()));

    // throws and preserves non-empty file without valid header
    let no_header_file = temp_path(dir.path(), "no-header.jsonl");
    let original_content = "{\"type\":\"message\",\"id\":\"abc\",\"parentId\":\"orphaned\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":\"test\"}}\n";
    std::fs::write(&no_header_file, original_content).unwrap();
    let error = SessionManager::open(&no_header_file, Some(&temp), None).expect_err("rejected");
    assert_eq!(
        error.to_string(),
        format!("Session file is not a valid pi session: {no_header_file}")
    );
    assert_eq!(read_file(&no_header_file), original_content);

    // throws and preserves non-session JSONL files
    let non_session_file = temp_path(dir.path(), "not-a-session.log");
    std::fs::write(
        &non_session_file,
        "{\"type\":\"event\",\"data\":\"not a session\"}\n",
    )
    .unwrap();
    let original_log = read_file(&non_session_file);
    let error = SessionManager::open(&non_session_file, Some(&temp), None).expect_err("rejected");
    assert_eq!(
        error.to_string(),
        format!("Session file is not a valid pi session: {non_session_file}")
    );
    assert_eq!(read_file(&non_session_file), original_log);

    // preserves explicit session file path when recovering from corrupted file
    let explicit_path = temp_path(dir.path(), "my-session.jsonl");
    std::fs::write(&explicit_path, "").unwrap();
    let sm = SessionManager::open(&explicit_path, Some(&temp), None).unwrap();
    assert_eq!(sm.get_session_file(), Some(explicit_path.as_str()));

    // subsequent loads of initialized empty file work correctly
    let empty_file = temp_path(dir.path(), "empty2.jsonl");
    std::fs::write(&empty_file, "").unwrap();
    let sm1 = SessionManager::open(&empty_file, Some(&temp), None).unwrap();
    let session_id = sm1.get_session_id().to_string();
    let sm2 = SessionManager::open(&empty_file, Some(&temp), None).unwrap();
    assert_eq!(sm2.get_session_id(), session_id);
    assert_eq!(sm2.get_header().unwrap().id, Some(session_id));
    test_id_seam::disable();
}

#[test]
fn opens_session_files_with_huge_null_padded_regions() {
    // upstream: "opens session files larger than Node's max string length" —
    // the premise is a JS-runtime limit; the port scales the fixture down
    // while keeping the reader property (sparse NUL regions parse as
    // malformed lines and are skipped).
    let dir = scenario_dir("large");
    let temp = dir.path().to_string_lossy().into_owned();
    let file = temp_path(dir.path(), "large.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"session\",\"version\":3,\"id\":\"abc\",\"timestamp\":\"2025-01-01T00:00:00Z\",\"cwd\":\"/tmp\"}\n",
    )
    .unwrap();
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        let newline = b"\n";
        let stride = 1024 * 1024usize;
        let mut offset = stride;
        while offset <= 8 * stride {
            handle.seek(SeekFrom::Start(offset as u64)).unwrap();
            handle.write_all(newline).unwrap();
            offset += stride;
        }
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2025-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":1}}\n")
        .unwrap();

    let manager = SessionManager::open(&file, Some(&temp), None).unwrap();
    assert_eq!(manager.get_session_id(), "abc");
    assert_eq!(manager.get_entries().len(), 1);
    let ctx = manager.build_session_context();
    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(
        serde_json::to_value(&ctx.messages[0]).unwrap(),
        json!({ "role": "user", "content": "hi", "timestamp": 1 })
    );
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/labels.test.ts (remaining cases)
// ---------------------------------------------------------------------------

#[test]
fn labels_set_get_and_clear() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let msg_id = session.append_message(user_msg("hello")).unwrap();
    assert_eq!(session.get_label(&msg_id), None);

    let label_id = session
        .append_label_change(&msg_id, Some("checkpoint"))
        .unwrap();
    assert_eq!(session.get_label(&msg_id), Some("checkpoint"));

    let entries = session.get_entries();
    let label_entry = entries
        .iter()
        .find_map(|e| match e {
            SessionEntry::Label(label) => Some(label),
            _ => None,
        })
        .unwrap();
    assert_eq!(label_entry.id, label_id);
    assert_eq!(label_entry.target_id, msg_id);
    assert_eq!(label_entry.label.as_deref(), Some("checkpoint"));
    test_id_seam::disable();
}

#[test]
fn labels_not_in_context_and_unknown_target_rejected() {
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let msg_id = session.append_message(user_msg("hello")).unwrap();
    session
        .append_label_change(&msg_id, Some("checkpoint"))
        .unwrap();
    let ctx = session.build_session_context();
    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(ctx.messages[0].role(), "user");

    let error = session
        .append_label_change("non-existent", Some("label"))
        .expect_err("rejected");
    assert_eq!(error.to_string(), "Entry non-existent not found");
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/load-entries.test.ts (inMemory preloaded)
// ---------------------------------------------------------------------------

fn stored_entries(build: &dyn Fn(&mut SessionManager)) -> Vec<SessionEntry> {
    let mut source = SessionManager::in_memory("/project", None, None).unwrap();
    build(&mut source);
    source.get_entries()
}

fn preloaded_user(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })]),
        timestamp: crate::ai::now_ms(),
    })
}

#[test]
fn in_memory_preloaded_entries_adopt_verbatim() {
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
        source
            .append_model_change("anthropic", "claude-opus-4-5")
            .unwrap();
        source.append_message(preloaded_user("again")).unwrap();
    });
    let session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    let restored = session.get_entries();
    assert_eq!(restored.len(), entries.len());
    for (restored, original) in restored.iter().zip(entries.iter()) {
        assert_eq!(restored.id(), original.id());
        assert_eq!(restored.parent_id(), original.parent_id());
        assert_eq!(entry_kind(restored), entry_kind(original));
    }
}

#[test]
fn in_memory_preloaded_entries_keep_leaf_and_continue() {
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
        source.append_message(preloaded_user("again")).unwrap();
    });
    let last_id = entries.last().unwrap().id().unwrap().to_string();
    let mut session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    let appended = session.append_message(preloaded_user("continued")).unwrap();
    assert_eq!(session.get_leaf_id(), Some(appended.as_str()));
    assert_eq!(
        session
            .get_entry(&appended)
            .unwrap()
            .parent_id()
            .map(str::to_string),
        Some(last_id)
    );
}

#[test]
fn in_memory_preloaded_entries_never_mint_colliding_ids() {
    let entries = stored_entries(&|source| {
        for i in 0..50 {
            source
                .append_message(preloaded_user(&format!("message {i}")))
                .unwrap();
        }
    });
    let mut session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    let appended = session.append_message(preloaded_user("continued")).unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.id() == Some(appended.as_str())));
}

#[test]
fn in_memory_preloaded_entries_rebuild_branch_structure() {
    let entries = stored_entries(&|source| {
        let first = source.append_message(preloaded_user("hello")).unwrap();
        source.append_message(preloaded_user("abandoned")).unwrap();
        source.branch(&first).unwrap();
        source.append_message(preloaded_user("kept")).unwrap();
    });
    let session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    let roots = session.get_tree();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].children.len(), 2);
}

#[test]
fn in_memory_preloaded_entries_rebuild_labels() {
    let entries = stored_entries(&|source| {
        let labelled = source.append_message(preloaded_user("hello")).unwrap();
        source
            .append_label_change(&labelled, Some("checkpoint"))
            .unwrap();
    });
    let labelled_id = entries[0].id().unwrap().to_string();
    let session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    assert_eq!(session.get_label(&labelled_id), Some("checkpoint"));
}

#[test]
fn in_memory_preloaded_entries_resolve_compaction() {
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("dropped")).unwrap();
        let kept = source.append_message(preloaded_user("kept")).unwrap();
        source
            .append_compaction("summary so far", Some(&kept), 1000, None, None, None)
            .unwrap();
    });
    let kept_id = entries[1].id().unwrap().to_string();
    let session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    let context = session.build_context_entries();
    assert!(context
        .iter()
        .any(|entry: &SessionEntry| entry.id() == Some(kept_id.as_str())));
}

#[test]
fn in_memory_preloaded_entries_create_header_from_options() {
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
    });
    let session = SessionManager::in_memory(
        "/project",
        Some(NewSessionOptions {
            id: Some("restored-session".to_string()),
            parent_session: None,
        })
        .as_ref(),
        Some(entries_to_file_entries(entries)),
    )
    .unwrap();
    assert_eq!(session.get_session_id(), "restored-session");
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some("restored-session")
    );
    // the manager cwd is resolvePath("/project"), drive-prefixed on Windows hosts
    assert_eq!(
        session.get_header().unwrap().cwd.as_deref(),
        Some(super::resolve_path_auto_base("/project").unwrap().as_str())
    );
}

#[test]
fn in_memory_preloaded_entries_generate_session_ids() {
    test_id_seam::disable();
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
    });
    let session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    assert!(uuid_v7_regexp().is_match(session.get_session_id()));
    assert_eq!(
        session.get_header().unwrap().id.as_deref(),
        Some(session.get_session_id())
    );
}

#[test]
fn in_memory_preloaded_entries_stay_off_the_filesystem() {
    let entries = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
    });
    let mut session = SessionManager::in_memory(
        "/project",
        None,
        Some(entries_to_file_entries(entries.clone())),
    )
    .unwrap();
    session.append_message(preloaded_user("continued")).unwrap();
    assert!(session.get_session_file().is_none());
    assert!(!session.is_persisted());
}

#[test]
fn in_memory_preloaded_empty_entries_start_empty_session() {
    let session = SessionManager::in_memory(
        "/project",
        Some(NewSessionOptions {
            id: Some("empty-session".to_string()),
            parent_session: None,
        })
        .as_ref(),
        Some(vec![]),
    )
    .unwrap();
    assert_eq!(session.get_session_id(), "empty-session");
    assert!(session.get_entries().is_empty());
    assert_eq!(session.get_leaf_id(), None);
}

#[test]
fn in_memory_preloaded_entries_take_identity_from_header() {
    let body = stored_entries(&|source| {
        source.append_message(preloaded_user("hello")).unwrap();
    });
    let mut entries = vec![FileEntry::Session(SessionHeader {
        version: Some(3),
        id: Some("stored-session".to_string()),
        timestamp: Some("2026-01-01T00:00:00Z".to_string()),
        cwd: Some("/stored".to_string()),
        parent_session: None,
    })];
    entries.extend(body.into_iter().map(FileEntry::Entry));
    let session = SessionManager::in_memory(
        "/project",
        Some(NewSessionOptions {
            id: Some("ignored".to_string()),
            parent_session: None,
        })
        .as_ref(),
        Some(entries),
    )
    .unwrap();
    assert_eq!(session.get_session_id(), "stored-session");
    assert_eq!(
        session.get_header().unwrap().cwd.as_deref(),
        Some("/stored")
    );
}

#[test]
fn in_memory_preloaded_entries_migrate_older_headers() {
    let entries = vec![
        FileEntry::Session(SessionHeader {
            version: Some(2),
            id: Some("v2-session".to_string()),
            timestamp: Some("2026-01-01T00:00:00Z".to_string()),
            cwd: Some("/project".to_string()),
            parent_session: None,
        }),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "hookmsgid", "parentId": null,
                "timestamp": "2026-01-01T00:00:01Z",
                "message": { "role": "hookMessage", "content": "from a hook", "timestamp": 1 },
            }))
            .unwrap(),
        ),
    ];
    let session = SessionManager::in_memory("/project", None, Some(entries.clone())).unwrap();
    assert_eq!(session.get_header().unwrap().version, Some(3));
    assert_eq!(entry_message_role(&session.get_entries()[0]), "custom");
    assert_eq!(session.get_entries()[0].id(), Some("hookmsgid"));
}

#[test]
fn in_memory_preloaded_headerless_entries_skip_migration() {
    let entries = vec![FileEntry::Entry(
        serde_json::from_value(json!({
            "type": "message", "id": "hookmsgid", "parentId": null,
            "timestamp": "2026-01-01T00:00:01Z",
            "message": { "role": "hookMessage", "content": "from a hook", "timestamp": 1 },
        }))
        .unwrap(),
    )];
    let session = SessionManager::in_memory("/project", None, Some(entries.clone())).unwrap();
    assert_eq!(entry_message_role(&session.get_entries()[0]), "hookMessage");
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/migration.test.ts
// ---------------------------------------------------------------------------

#[test]
fn migrate_adds_ids_to_v1_entries() {
    test_id_seam::reset();
    let mut entries = vec![
        FileEntry::Session(SessionHeader {
            version: None,
            id: Some("sess-1".to_string()),
            timestamp: Some("2025-01-01T00:00:00Z".to_string()),
            cwd: Some("/tmp".to_string()),
            parent_session: None,
        }),
        FileEntry::Unparsed(json!({
            "type": "message", "timestamp": "2025-01-01T00:00:01Z",
            "message": { "role": "user", "content": "hi", "timestamp": 1 },
        })),
        FileEntry::Entry(SessionEntry::Compaction(CompactionEntry {
            id: String::new(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:02Z".to_string(),
            summary: String::new(),
            first_kept_entry_id: None,
            tokens_before: 0,
            details: None,
            usage: None,
            from_hook: None,
            system_message: None,
            first_kept_entry_index: None,
        })),
    ];
    // the upstream fixture's second entry carries a full assistant message;
    // v1 entries have no id yet, so the loose capture holds them
    entries[2] = FileEntry::Unparsed(json!({
        "type": "message", "timestamp": "2025-01-01T00:00:02Z",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": "hello" }],
            "api": "test", "provider": "test", "model": "test",
            "usage": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0 },
            "stopReason": "stop", "timestamp": 2,
        },
    }));

    migrate_session_entries(&mut entries);

    // header should have version set (v3 is current after hookMessage->custom)
    let FileEntry::Session(header) = &entries[0] else {
        unreachable!()
    };
    assert_eq!(header.version, Some(3));

    let id1 = match &entries[1] {
        FileEntry::Unparsed(value) => value["id"].as_str().expect("generated id").to_string(),
        other => unreachable!("{other:?}"),
    };
    let parent2 = match &entries[2] {
        FileEntry::Unparsed(value) => value["parentId"].as_str().map(str::to_string),
        other => unreachable!("{other:?}"),
    };
    assert_eq!(id1.len(), 8);
    assert_eq!(parent1_of(&entries[1]), None);
    assert_eq!(parent2.as_deref(), Some(id1.as_str()));
    test_id_seam::disable();
}

fn parent1_of(entry: &FileEntry) -> Option<String> {
    match entry {
        FileEntry::Unparsed(value) => value["parentId"].as_str().map(str::to_string),
        FileEntry::Entry(typed) => typed.parent_id().map(str::to_string),
        FileEntry::Session(_) => None,
    }
}

#[test]
fn migrate_is_idempotent_for_current_entries() {
    let mut entries = vec![
        FileEntry::Session(SessionHeader {
            version: Some(2),
            id: Some("sess-1".to_string()),
            timestamp: Some("2025-01-01T00:00:00Z".to_string()),
            cwd: Some("/tmp".to_string()),
            parent_session: None,
        }),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "hookmsgid", "parentId": null,
                "timestamp": "2025-01-01T00:00:01Z",
                "message": { "role": "user", "content": "hi", "timestamp": 1 },
            }))
            .unwrap(),
        ),
        FileEntry::Entry(
            serde_json::from_value(json!({
                "type": "message", "id": "usermsgid", "parentId": "hookmsgid",
                "timestamp": "2025-01-01T00:00:02Z",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "hello" }],
                    "api": "test", "provider": "test", "model": "test",
                    "usage": { "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2, "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 } },
                    "stopReason": "stop", "timestamp": 2,
                },
            }))
            .unwrap(),
        ),
    ];

    migrate_session_entries(&mut entries);

    let id1 = match &entries[1] {
        FileEntry::Entry(SessionEntry::Message(message)) => message.id.clone(),
        other => unreachable!("{other:?}"),
    };
    let (id2, parent2) = match &entries[2] {
        FileEntry::Entry(SessionEntry::Message(message)) => {
            (message.id.clone(), message.parent_id.clone())
        }
        other => unreachable!("{other:?}"),
    };
    assert_eq!(id1, "hookmsgid");
    assert_eq!(id2, "usermsgid");
    assert_eq!(parent2.as_deref(), Some("hookmsgid"));
    // v2 → v3 still renames hookMessage roles while keeping ids stable
    let header_version = match &entries[0] {
        FileEntry::Session(header) => header.version,
        _ => unreachable!(),
    };
    assert_eq!(header_version, Some(3));
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/save-entry.test.ts
// ---------------------------------------------------------------------------

#[test]
fn saves_custom_entries_and_includes_them_in_tree_traversal() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();

    let msg_id = session.append_message(user_msg("hello")).unwrap();
    let custom_id = session
        .append_custom_entry("my_data", Some(json!({ "foo": "bar" })))
        .unwrap();
    let msg2_id = session.append_message(assistant_msg("hi")).unwrap();

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);

    let custom_entry = entries
        .iter()
        .find_map(|e| match e {
            SessionEntry::Custom(custom) => Some(custom),
            _ => None,
        })
        .unwrap();
    assert_eq!(custom_entry.custom_type, "my_data");
    assert_eq!(custom_entry.data, Some(json!({ "foo": "bar" })));
    assert_eq!(custom_entry.id, custom_id);
    assert_eq!(custom_entry.parent_id.as_deref(), Some(msg_id.as_str()));

    let path = session.get_branch(None);
    assert_eq!(path.len(), 3);
    assert_eq!(path[0].id(), Some(msg_id.as_str()));
    assert_eq!(path[1].id(), Some(custom_id.as_str()));
    assert_eq!(path[2].id(), Some(msg2_id.as_str()));

    let ctx = session.build_session_context();
    assert_eq!(ctx.messages.len(), 2);
    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// upstream test/session-manager/tree-traversal.test.ts
// ---------------------------------------------------------------------------

#[test]
fn append_message_creates_parent_chain() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("first")).unwrap();
    let id2 = session.append_message(assistant_msg("second")).unwrap();
    let id3 = session.append_message(user_msg("third")).unwrap();

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id(), Some(id1.as_str()));
    assert_eq!(entries[0].parent_id(), None);
    assert_eq!(entry_kind(&entries[0]), Some("message"));
    assert_eq!(entries[1].id(), Some(id2.as_str()));
    assert_eq!(entries[1].parent_id(), Some(id1.as_str()));
    assert_eq!(entries[2].id(), Some(id3.as_str()));
    assert_eq!(entries[2].parent_id(), Some(id2.as_str()));
    test_id_seam::disable();
}

#[test]
fn append_thinking_level_change_integrates_into_tree() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let msg_id = session.append_message(user_msg("hello")).unwrap();
    let thinking_id = session.append_thinking_level_change("high").unwrap();
    session.append_message(assistant_msg("response")).unwrap();

    let entries = session.get_entries();
    assert_eq!(entries.len(), 3);
    let thinking = entries.iter().find_map(|e| match e {
        SessionEntry::ThinkingLevelChange(change) => Some(change),
        _ => None,
    });
    let thinking = thinking.expect("thinking entry");
    assert_eq!(thinking.id, thinking_id);
    assert_eq!(thinking.parent_id.as_deref(), Some(msg_id.as_str()));
    assert_eq!(entries[2].parent_id(), Some(thinking_id.as_str()));
    test_id_seam::disable();
}

#[test]
fn append_model_change_integrates_into_tree() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let msg_id = session.append_message(user_msg("hello")).unwrap();
    let model_id = session.append_model_change("openai", "gpt-4").unwrap();
    session.append_message(assistant_msg("response")).unwrap();

    let entries = session.get_entries();
    let model = entries.iter().find_map(|e| match e {
        SessionEntry::ModelChange(change) => Some(change),
        _ => None,
    });
    let model = model.expect("model entry");
    assert_eq!(model.id, model_id);
    assert_eq!(model.parent_id.as_deref(), Some(msg_id.as_str()));
    assert_eq!(model.provider, "openai");
    assert_eq!(model.model_id, "gpt-4");
    assert_eq!(entries[2].parent_id(), Some(model_id.as_str()));
    test_id_seam::disable();
}

#[test]
fn append_compaction_integrates_into_tree() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    let usage = big_usage();
    let compaction_id = session
        .append_compaction("summary", Some(&id1), 1000, None, Some(false), Some(usage))
        .unwrap();
    session.append_message(user_msg("3")).unwrap();

    let entries = session.get_entries();
    let compaction = entries.iter().find_map(|e| match e {
        SessionEntry::Compaction(compaction) => Some(compaction),
        _ => None,
    });
    let compaction = compaction.expect("compaction entry");
    assert_eq!(compaction.id, compaction_id);
    assert_eq!(compaction.parent_id.as_deref(), Some(id2.as_str()));
    assert_eq!(compaction.summary, "summary");
    assert_eq!(
        compaction.first_kept_entry_id.as_deref(),
        Some(id1.as_str())
    );
    assert_eq!(compaction.tokens_before, 1000);
    assert_eq!(compaction.usage, Some(usage));
    assert_eq!(entries[3].parent_id(), Some(compaction_id.as_str()));
    test_id_seam::disable();
}

#[test]
fn append_custom_entry_integrates_into_tree() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let msg_id = session.append_message(user_msg("hello")).unwrap();
    let custom_id = session
        .append_custom_entry("my_data", Some(json!({ "key": "value" })))
        .unwrap();
    session.append_message(assistant_msg("response")).unwrap();

    let entries = session.get_entries();
    let custom = entries.iter().find_map(|e| match e {
        SessionEntry::Custom(custom) => Some(custom),
        _ => None,
    });
    let custom = custom.expect("custom entry");
    assert_eq!(custom.id, custom_id);
    assert_eq!(custom.parent_id.as_deref(), Some(msg_id.as_str()));
    assert_eq!(custom.custom_type, "my_data");
    assert_eq!(custom.data, Some(json!({ "key": "value" })));
    assert_eq!(entries[2].parent_id(), Some(custom_id.as_str()));
    test_id_seam::disable();
}

#[test]
fn leaf_pointer_advances_after_each_append() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert_eq!(session.get_leaf_id(), None);
    let id1 = session.append_message(user_msg("1")).unwrap();
    assert_eq!(session.get_leaf_id(), Some(id1.as_str()));
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    assert_eq!(session.get_leaf_id(), Some(id2.as_str()));
    let id3 = session.append_thinking_level_change("high").unwrap();
    assert_eq!(session.get_leaf_id(), Some(id3.as_str()));
    test_id_seam::disable();
}

#[test]
fn get_branch_paths() {
    test_id_seam::reset();
    let cwd = process_cwd_string();

    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert!(session.get_branch(None).is_empty());

    let id = session.append_message(user_msg("hello")).unwrap();
    let branch = session.get_branch(None);
    assert_eq!(branch.len(), 1);
    assert_eq!(branch[0].id(), Some(id.as_str()));

    let id1 = id;
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_thinking_level_change("high").unwrap();
    let id4 = session.append_message(user_msg("3")).unwrap();
    let path = session.get_branch(None);
    assert_eq!(path.len(), 4);
    let ids: Vec<&str> = path.iter().filter_map(SessionEntry::id).collect();
    assert_eq!(
        ids,
        [id1.as_str(), id2.as_str(), id3.as_str(), id4.as_str()]
    );

    let from_mid = session.get_branch(Some(&id2));
    assert_eq!(from_mid.len(), 2);
    let ids: Vec<&str> = from_mid.iter().filter_map(SessionEntry::id).collect();
    assert_eq!(ids, [id1.as_str(), id2.as_str()]);
    test_id_seam::disable();
}

#[test]
fn get_tree_shapes() {
    test_id_seam::reset();
    let cwd = process_cwd_string();

    // returns empty array for empty session
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert!(session.get_tree().is_empty());

    // returns single root for linear session
    let id1 = session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_message(user_msg("3")).unwrap();
    let tree = session.get_tree();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].entry.id(), Some(id1.as_str()));
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].entry.id(), Some(id2.as_str()));
    assert_eq!(tree[0].children[0].children.len(), 1);
    assert_eq!(
        tree[0].children[0].children[0].entry.id(),
        Some(id3.as_str())
    );
    assert!(tree[0].children[0].children[0].children.is_empty());

    // returns tree with branches after branch
    session.branch(&id2).unwrap();
    let id4 = session.append_message(user_msg("4-branch")).unwrap();
    let tree = session.get_tree();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].children.len(), 1);
    let node2 = &tree[0].children[0];
    assert_eq!(node2.entry.id(), Some(id2.as_str()));
    assert_eq!(node2.children.len(), 2);
    let mut child_ids: Vec<&str> = node2.children.iter().filter_map(|c| c.entry.id()).collect();
    child_ids.sort();
    let mut expected = vec![id3.as_str(), id4.as_str()];
    expected.sort();
    assert_eq!(child_ids, expected);

    // handles multiple branches at same point
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session.append_message(user_msg("root")).unwrap();
    let id2 = session.append_message(assistant_msg("response")).unwrap();
    session.branch(&id2).unwrap();
    let id_a = session.append_message(user_msg("branch-A")).unwrap();
    session.branch(&id2).unwrap();
    let id_b = session.append_message(user_msg("branch-B")).unwrap();
    session.branch(&id2).unwrap();
    let id_c = session.append_message(user_msg("branch-C")).unwrap();
    let tree = session.get_tree();
    let node2 = &tree[0].children[0];
    assert_eq!(node2.entry.id(), Some(id2.as_str()));
    assert_eq!(node2.children.len(), 3);
    let mut branch_ids: Vec<&str> = node2.children.iter().filter_map(|c| c.entry.id()).collect();
    branch_ids.sort();
    let mut expected = vec![id_a.as_str(), id_b.as_str(), id_c.as_str()];
    expected.sort();
    assert_eq!(branch_ids, expected);

    // handles deep branching
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_message(user_msg("3")).unwrap();
    session.append_message(assistant_msg("4")).unwrap();
    session.branch(&id2).unwrap();
    let id5 = session.append_message(user_msg("5")).unwrap();
    session.append_message(assistant_msg("6")).unwrap();
    session.branch(&id5).unwrap();
    session.append_message(user_msg("7")).unwrap();
    let tree = session.get_tree();
    let node2 = &tree[0].children[0];
    assert_eq!(node2.children.len(), 2);
    let node5 = node2
        .children
        .iter()
        .find(|c| c.entry.id() == Some(id5.as_str()))
        .unwrap();
    assert_eq!(node5.children.len(), 2);
    let node3 = node2
        .children
        .iter()
        .find(|c| c.entry.id() == Some(id3.as_str()))
        .unwrap();
    assert_eq!(node3.children.len(), 1);
    test_id_seam::disable();
}

#[test]
fn branch_moves_leaf_pointer() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("1")).unwrap();
    session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_message(user_msg("3")).unwrap();
    assert_eq!(session.get_leaf_id(), Some(id3.as_str()));
    session.branch(&id1).unwrap();
    assert_eq!(session.get_leaf_id(), Some(id1.as_str()));

    // throws for non-existent entry
    let error = session.branch("nonexistent").expect_err("rejected");
    assert_eq!(error.to_string(), "Entry nonexistent not found");

    // new appends become children of branch point
    let id3b = session.append_message(user_msg("branched")).unwrap();
    let branched = session
        .get_entries()
        .into_iter()
        .find(|e| e.id() == Some(id3b.as_str()))
        .unwrap();
    assert_eq!(branched.parent_id(), Some(id1.as_str()));
    test_id_seam::disable();
}

#[test]
fn branch_with_summary_inserts_summary_entry() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("1")).unwrap();
    session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_message(user_msg("3")).unwrap();
    let usage = big_usage();
    let summary_id = session
        .branch_with_summary(
            Some(id1.as_str()),
            "Summary of abandoned work",
            None,
            Some(false),
            Some(usage),
        )
        .unwrap();

    assert_eq!(session.get_leaf_id(), Some(summary_id.as_str()));

    let entries = session.get_entries();
    let summary = entries
        .iter()
        .find_map(|e| match e {
            SessionEntry::BranchSummary(summary) => Some(summary),
            _ => None,
        })
        .unwrap();
    assert_eq!(summary.parent_id.as_deref(), Some(id1.as_str()));
    assert_eq!(summary.from_id, id3);
    assert_eq!(summary.summary, "Summary of abandoned work");
    assert_eq!(summary.usage, Some(usage));

    // throws for non-existent entry
    let error = session
        .branch_with_summary(Some("nonexistent"), "summary", None, None, None)
        .expect_err("rejected");
    assert_eq!(error.to_string(), "Entry nonexistent not found");
    test_id_seam::disable();
}

#[test]
fn leaf_entry_accessors() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert!(session.get_leaf_entry().is_none());

    session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    assert_eq!(session.get_leaf_entry().unwrap().id(), Some(id2.as_str()));
    test_id_seam::disable();
}

#[test]
fn get_entry_by_id() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    assert!(session.get_entry("nonexistent").is_none());

    let id1 = session.append_message(user_msg("first")).unwrap();
    let id2 = session.append_message(assistant_msg("second")).unwrap();

    let entry1 = session.get_entry(&id1).unwrap();
    assert_eq!(entry_kind(entry1), Some("message"));
    let SessionEntry::Message(message) = entry1 else {
        unreachable!()
    };
    let AgentMessage::User(user) = &message.message else {
        unreachable!()
    };
    assert_eq!(user.content, StringOrBlocks::Text("first".to_string()));

    let entry2 = session.get_entry(&id2).unwrap();
    let SessionEntry::Message(message) = entry2 else {
        unreachable!()
    };
    let AgentMessage::Assistant(assistant) = &message.message else {
        unreachable!()
    };
    match &assistant.content[0] {
        AssistantBlock::Text(TextContent { text, .. }) => assert_eq!(text, "second"),
        other => unreachable!("{other:?}"),
    }
    test_id_seam::disable();
}

#[test]
fn build_session_context_returns_current_branch_only() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session.append_message(user_msg("msg1")).unwrap();
    let id2 = session.append_message(assistant_msg("msg2")).unwrap();
    session.append_message(user_msg("msg3")).unwrap();
    session.branch(&id2).unwrap();
    session
        .append_message(assistant_msg("msg4-branch"))
        .unwrap();

    let ctx = session.build_session_context();
    assert_eq!(ctx.messages.len(), 3);
    assert_eq!(ctx.messages[0].role(), "user");
    assert_eq!(ctx.messages[1].role(), "assistant");
    assert_eq!(ctx.messages[2].role(), "assistant");
    test_id_seam::disable();
}

// ---------------------------------------------------------------------------
// upstream tree-traversal.test.ts createBranchedSession block
// ---------------------------------------------------------------------------

#[test]
fn create_branched_session_throws_for_nonexistent_entry() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    session.append_message(user_msg("hello")).unwrap();
    let error = session
        .create_branched_session("nonexistent")
        .expect_err("rejected");
    assert_eq!(error.to_string(), "Entry nonexistent not found");
    test_id_seam::disable();
}

#[test]
fn create_branched_session_in_memory_keeps_path_to_leaf() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    let id3 = session.append_message(user_msg("3")).unwrap();
    session.append_message(assistant_msg("4")).unwrap();
    session.branch(&id3).unwrap();
    session.append_message(user_msg("5")).unwrap();

    let result = session.create_branched_session(&id2).unwrap();
    assert!(result.is_none());
    let entries = session.get_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id(), Some(id1.as_str()));
    assert_eq!(entries[1].id(), Some(id2.as_str()));
    test_id_seam::disable();
}

#[test]
fn create_branched_session_extracts_path_from_branched_tree() {
    test_id_seam::reset();
    let cwd = process_cwd_string();
    let mut session = SessionManager::in_memory(&cwd, None, None).unwrap();
    let id1 = session.append_message(user_msg("1")).unwrap();
    let id2 = session.append_message(assistant_msg("2")).unwrap();
    session.append_message(user_msg("3")).unwrap();
    session.branch(&id2).unwrap();
    let id4 = session.append_message(user_msg("4")).unwrap();
    let id5 = session.append_message(assistant_msg("5")).unwrap();

    session.create_branched_session(&id5).unwrap();
    let entries = session.get_entries();
    assert_eq!(entries.len(), 4);
    let ids: Vec<&str> = entries.iter().filter_map(SessionEntry::id).collect();
    assert_eq!(
        ids,
        [id1.as_str(), id2.as_str(), id4.as_str(), id5.as_str()]
    );
    test_id_seam::disable();
}

#[test]
fn create_branched_session_does_not_duplicate_entries_from_first_user_message() {
    test_id_seam::reset();
    let dir = scenario_dir("fork-dedup");
    let temp = dir.path().to_string_lossy().into_owned();
    let mut session = SessionManager::create(&temp, Some(&temp), None).unwrap();
    let id1 = session.append_message(user_msg("first question")).unwrap();
    session
        .append_message(assistant_msg("first answer"))
        .unwrap();
    session.append_message(user_msg("second question")).unwrap();
    session
        .append_message(assistant_msg("second answer"))
        .unwrap();

    let new_file = session.create_branched_session(&id1).unwrap().unwrap();
    // Delta (upstream `_hasConversation`): the branched path holds a user
    // message, so the file is written immediately (oracle
    // `manager.branch-no-assistant-exists` = true).
    assert!(
        std::path::Path::new(&new_file).exists(),
        "branched path has a conversation: written now"
    );

    session
        .append_custom_entry("preset-state", Some(json!({ "name": "plan" })))
        .unwrap();
    session.append_message(assistant_msg("new answer")).unwrap();

    assert!(std::path::Path::new(&new_file).exists());
    let content = read_file(&new_file);
    let records: Vec<Value> = content
        .trim()
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        records.iter().filter(|r| r["type"] == "session").count(),
        1,
        "exactly one header"
    );
    let entry_ids: Vec<String> = records
        .iter()
        .filter(|r| r["type"] != "session")
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        entry_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        entry_ids.len(),
        "no duplicate ids"
    );
    test_id_seam::disable();
}

#[test]
fn create_branched_session_preserves_usage_across_reload() {
    test_id_seam::reset();
    let dir = scenario_dir("usage-roundtrip");
    let temp = dir.path().to_string_lossy().into_owned();
    let mut session = SessionManager::create(&temp, Some(&temp), None).unwrap();
    let root_id = session.append_message(user_msg("question")).unwrap();
    session.append_message(assistant_msg("answer")).unwrap();
    let usage = big_usage();
    session
        .append_message(
            serde_json::from_value(json!({
                "role": "toolResult", "toolCallId": "call-1", "toolName": "nested-model",
                "content": [{ "type": "text", "text": "result" }], "isError": false,
                "usage": {
                    "input": 10, "output": 20, "cacheRead": 30, "cacheWrite": 40, "totalTokens": 100,
                    "cost": { "input": 0.1, "output": 0.2, "cacheRead": 0.3, "cacheWrite": 0.4, "total": 1 },
                },
                "timestamp": 0,
            }))
            .unwrap(),
        )
        .unwrap();
    session
        .append_compaction(
            "summary",
            Some(&root_id),
            100,
            None,
            Some(false),
            Some(usage),
        )
        .unwrap();
    session
        .branch_with_summary(
            Some(root_id.as_str()),
            "branch summary",
            None,
            Some(false),
            Some(usage),
        )
        .unwrap();

    let file = session.get_session_file().unwrap().to_string();
    let reopened = SessionManager::open(&file, Some(&temp), None).unwrap();
    let entries = reopened.get_entries();
    let compaction = entries.iter().find_map(|e| match e {
        SessionEntry::Compaction(c) => Some(c),
        _ => None,
    });
    let summary = entries.iter().find_map(|e| match e {
        SessionEntry::BranchSummary(s) => Some(s),
        _ => None,
    });
    let tool_result = entries.iter().find_map(|e| match e {
        SessionEntry::Message(m) => match &m.message {
            AgentMessage::ToolResult(result) => Some(result),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(compaction.expect("compaction").usage, Some(usage));
    assert_eq!(summary.expect("branch summary").usage, Some(usage));
    assert_eq!(tool_result.expect("tool result").usage, Some(usage));
    test_id_seam::disable();
}

#[test]
fn create_branched_session_writes_immediately_with_assistant() {
    test_id_seam::reset();
    let dir = scenario_dir("fork-with-assistant");
    let temp = dir.path().to_string_lossy().into_owned();
    let mut session = SessionManager::create(&temp, Some(&temp), None).unwrap();
    session.append_message(user_msg("first question")).unwrap();
    let id2 = session
        .append_message(assistant_msg("first answer"))
        .unwrap();
    session.append_message(user_msg("second question")).unwrap();
    session
        .append_message(assistant_msg("second answer"))
        .unwrap();

    let new_file = session.create_branched_session(&id2).unwrap().unwrap();
    assert!(
        std::path::Path::new(&new_file).exists(),
        "written immediately"
    );
    let records: Vec<Value> = read_file(&new_file)
        .trim()
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.iter().filter(|r| r["type"] == "session").count(), 1);
    test_id_seam::disable();
}

#[test]
fn json_order_legacy_compaction_migration_keeps_unknown_fields_in_place() {
    // Same raw-entry path and deterministic ID seam as the actual-source Node witness.
    test_id_seam::reset();
    let mut entries = vec![
        FileEntry::Unparsed(json!({"type":"message","z":1,"a":2})),
        FileEntry::Unparsed(json!({"type":"compaction","firstKeptEntryIndex":0,"z":1,"a":2})),
    ];
    super::migrate_v1_to_v2(&mut entries);
    test_id_seam::disable();
    assert_eq!(
        serde_json::to_string(&entries[1]).unwrap(),
        r#"{"type":"compaction","z":1,"a":2,"id":"00000002","parentId":"00000001","firstKeptEntryId":"00000001"}"#
    );
}
