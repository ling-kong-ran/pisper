//! Port of `packages/agent/src/harness/session/jsonl/types.ts` (45 lines):
//! the format/version constants, the storage header (the first line of every
//! session file), and the storage/repo option and metadata shapes.

use serde::{Deserialize, Serialize};

use crate::agent_core::harness::types::FileSystem;

/// Upstream `JSONL_FORMAT_VERSION` (`types.ts:4`).
pub const JSONL_FORMAT_VERSION: i64 = 4;
/// Upstream `JSONL_STORAGE_VERSION` (`types.ts:5`).
pub const JSONL_STORAGE_VERSION: i64 = 1;

/// Upstream `JsonlStorageHeader` (`types.ts:7-18`): the first line of every
/// format-4 session file. `v` is always [`JSONL_FORMAT_VERSION`] on write;
/// `parentSessionId`/`legacyParentSessionPath`/`nextSeq` are omitted when
/// absent, like upstream `undefined`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlStorageHeader {
    /// Upstream `v: typeof JSONL_FORMAT_VERSION`.
    pub v: i64,
    /// Always `"header"`.
    pub kind: HeaderKind,
    pub id: String,
    pub storage_version: i64,
    pub created_at: i64,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    /// Sequence high-water mark written by snapshot rewrites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_seq: Option<i64>,
}

/// Upstream `kind: "header"` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderKind {
    Header,
}

impl JsonlStorageHeader {
    /// A fresh header with `v`/`kind` fixed (`types.ts:7-18`).
    pub fn new(
        id: impl Into<String>,
        storage_version: i64,
        created_at: i64,
        cwd: impl Into<String>,
    ) -> Self {
        JsonlStorageHeader {
            v: JSONL_FORMAT_VERSION,
            kind: HeaderKind::Header,
            id: id.into(),
            storage_version,
            created_at,
            cwd: cwd.into(),
            parent_session_id: None,
            legacy_parent_session_path: None,
            next_seq: None,
        }
    }
}

/// Upstream `JsonlStorageOptions` (`types.ts:20-24`).
pub struct JsonlStorageOptions {
    pub file_system: Arc<dyn FileSystem>,
    pub path: String,
    /// Upstream `now?: () => number`. Defaults to the wall clock.
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
}

/// Upstream `JsonlSessionMetadata` (`types.ts:26-31`) flattened into
/// [`crate::agent_core::harness::session::types::SessionMetadata`] (see the
/// session module docs); the extended fields live there as options.
/// Upstream `JsonlSessionCreateOptions` (`types.ts:33-35`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsonlSessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
    pub cwd: String,
}

/// Upstream `JsonlSessionListOptions` (`types.ts:37-39`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsonlSessionListOptions {
    pub cwd: Option<String>,
}

/// Upstream `JsonlSessionRepoOptions` (`types.ts:41-45`).
pub struct JsonlSessionRepoOptions {
    pub file_system: Arc<dyn FileSystem>,
    pub sessions_root: String,
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
}

use std::sync::Arc;
