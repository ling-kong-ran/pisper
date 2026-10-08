//! Codec tests: the legacy v3 header grammar (review round 1, Important 2)
//! — `version === 3` enforcement, `parentSession` string-or-absent (null
//! rejected), and the covering end-to-end error path through
//! [`JsonlSessionRepo::open`] with a version-5 fixture file.

use super::{
    parse_jsonl_session_header, parse_legacy_v3_session_header, JsonlParsedSessionHeader,
    LegacyV3SessionHeader,
};
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::jsonl::repo::JsonlSessionRepo;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlSessionListOptions, JsonlSessionRepoOptions,
};
use crate::agent_core::harness::types::FileSystem;
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

fn iso(ms: i64) -> String {
    crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(ms)
}

fn v3_header_json(version: i64, parent_session: Option<&str>) -> serde_json::Value {
    let mut header = serde_json::json!({
        "type": "session",
        "version": version,
        "id": "legacy",
        "timestamp": iso(NOW),
        "cwd": "/workspace",
    });
    if let Some(parent_session) = parent_session {
        header["parentSession"] = serde_json::json!(parent_session);
    }
    header
}

/// `version === 3` parses as v3-legacy (`codec.ts:21-32`).
#[test]
fn parses_version_three_header() {
    let parsed = parse_legacy_v3_session_header(&v3_header_json(3, None)).unwrap();
    assert_eq!(parsed.id, "legacy");
    assert_eq!(parsed.cwd, "/workspace");
    assert_eq!(parsed.parent_session, None);
    let via_line = parse_jsonl_session_header(&v3_header_json(3, None).to_string()).unwrap();
    assert!(matches!(
        via_line,
        JsonlParsedSessionHeader::V3Legacy { .. }
    ));
}

/// A future `version` is NOT v3-legacy: `parseJsonlSessionHeader` falls
/// through to "Unsupported JSONL session header" (upstream codec.ts:60-62).
#[test]
fn rejects_future_version_header() {
    assert!(parse_legacy_v3_session_header(&v3_header_json(5, None)).is_none());
    let error = parse_jsonl_session_header(&v3_header_json(5, None).to_string()).unwrap_err();
    assert_eq!(error.to_string(), "Unsupported JSONL session header");
}

/// `parentSession: null` is rejected (upstream requires
/// `typeof value.parentSession === "string"` when present).
#[test]
fn rejects_null_parent_session() {
    let mut value = v3_header_json(3, None);
    value["parentSession"] = serde_json::Value::Null;
    assert!(parse_legacy_v3_session_header(&value).is_none());
    let error = parse_jsonl_session_header(&value.to_string()).unwrap_err();
    assert_eq!(error.to_string(), "Unsupported JSONL session header");
}

/// `parentSession` as a string parses.
#[test]
fn parses_string_parent_session() {
    let parsed = parse_legacy_v3_session_header(&v3_header_json(3, Some("/old.jsonl"))).unwrap();
    assert_eq!(parsed.parent_session.as_deref(), Some("/old.jsonl"));
}

/// An invalid timestamp rejects (`Number.isFinite(Date.parse(...))`).
#[test]
fn rejects_unparseable_timestamp() {
    let mut value = v3_header_json(3, None);
    value["timestamp"] = serde_json::json!("not-a-date");
    assert!(parse_legacy_v3_session_header(&value).is_none());
}

/// End-to-end: a version-5 fixture file is discovered with
/// `legacyParentSessionPath`-style metadata absent and `repo.open` rejects
/// with the upstream "Unsupported JSONL session header" path — the file is
/// never silently upgraded.
#[tokio::test]
async fn repo_open_rejects_version_five_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let env = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    let cwd = env
        .absolute_path("/workspace", background_context())
        .await
        .unwrap()
        .trim_end_matches(['/', '\\'])
        .to_string();
    let encoded: String = cwd
        .trim_start_matches(['/', '\\'])
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let directory = env
        .join_path(
            &["sessions".to_string(), format!("--{encoded}--")],
            background_context(),
        )
        .await
        .unwrap();
    env.create_dir(&directory, None, background_context())
        .await
        .unwrap();
    let path = env
        .join_path(
            &[directory, "legacy-v5.jsonl".to_string()],
            background_context(),
        )
        .await
        .unwrap();
    let header = v3_header_json(5, None);
    env.write_file(
        &path,
        crate::agent_core::harness::types::FileContent::Text(format!("{header}\n")),
        background_context(),
    )
    .await
    .unwrap();

    let repo = JsonlSessionRepo::new(JsonlSessionRepoOptions {
        file_system: Arc::clone(&env) as Arc<dyn FileSystem>,
        sessions_root: "sessions".to_string(),
        now: Some(Arc::new(|| NOW)),
    });
    let listed = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some(cwd.clone()),
            }),
            background_context(),
        )
        .await
        .unwrap();
    // readSessionMetadata drops unparsable headers (upstream repo.ts:250-251).
    assert!(listed.is_empty(), "version-5 file must not be discovered");
    // Opening by a hand-built metadata handle rejects with the codec error.
    let error = repo
        .open(
            &crate::agent_core::harness::session::types::SessionMetadata {
                id: "legacy".to_string(),
                created_at: NOW,
                storage_version:
                    crate::agent_core::harness::session::jsonl::types::JSONL_STORAGE_VERSION,
                cwd: Some(cwd),
                path: Some(path.clone()),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid JSONL storage: invalid header: Unsupported JSONL session header")
            || error
                .to_string()
                .contains("Unsupported JSONL session header"),
        "{error}"
    );
    // The file is untouched (no silent v4 rewrite).
    let content = env
        .read_text_file(&path, background_context())
        .await
        .unwrap();
    assert_eq!(content, format!("{header}\n"));
    let _ = LegacyV3SessionHeader {
        r#type: super::LegacyHeaderType::Session,
        version: 3,
        id: String::new(),
        timestamp: String::new(),
        cwd: String::new(),
        parent_session: None,
    };
}
