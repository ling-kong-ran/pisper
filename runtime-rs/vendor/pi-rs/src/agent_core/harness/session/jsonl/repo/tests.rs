//! Ports of `packages/agent/test/harness/jsonl-session-repo.test.ts` (199
//! lines) and the core cases of `jsonl-v3-migration.test.ts` (2013 lines):
//! repo lifecycle, byte-format header publication, exclusive-open handles,
//! and the legacy v3 discovery/normalization/upgrade envelope.

use super::JsonlSessionRepo;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlSessionCreateOptions, JsonlSessionListOptions, JsonlSessionRepoOptions,
    JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::session::StorageBackedSession;
use crate::agent_core::harness::session::types::{ForkOptions, Session as _, Storage};
use crate::agent_core::harness::session::values::{
    entry_label, lane_config, lane_state, session_name, set_value,
};
use crate::agent_core::harness::types::{FileContent, FileError, FileSystem};
use futures::future::BoxFuture;
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

pub(crate) async fn resolved_cwd(file_system: &NodeExecutionEnv) -> String {
    file_system
        .absolute_path("/workspace", background_context())
        .await
        .unwrap_or_else(|_| "/workspace".to_string())
        .trim_end_matches(['/', '\\'])
        .to_string()
}

pub(crate) fn repo_in(dir: &tempfile::TempDir) -> Arc<JsonlSessionRepo> {
    Arc::new(JsonlSessionRepo::new(JsonlSessionRepoOptions {
        file_system: Arc::new(NodeExecutionEnv::new(
            dir.path().to_string_lossy().to_string(),
        )),
        sessions_root: "sessions".to_string(),
        now: Some(Arc::new(|| NOW)),
    }))
}

fn create_options(id: &str, cwd: &str) -> JsonlSessionCreateOptions {
    JsonlSessionCreateOptions {
        id: Some(id.to_string()),
        parent_session_id: None,
        cwd: cwd.to_string(),
    }
}

/// "persists metadata and filters discovery by cwd"
/// (`jsonl-session-repo.test.ts:32-68`).
#[tokio::test]
async fn persists_metadata_and_filters_by_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    // The oracle's "/workspace" resolves through the env's absolutePath; the
    // resolved form is the platform cwd on Windows.
    let cwd = resolved_cwd(&file_system).await;
    let session = repo
        .create(
            JsonlSessionCreateOptions {
                parent_session_id: Some("parent".to_string()),
                ..create_options("child", &cwd)
            },
            background_context(),
        )
        .await
        .unwrap();
    let metadata = session.metadata().clone();
    assert_eq!(metadata.id, "child");
    assert_eq!(metadata.created_at, NOW);
    assert_eq!(metadata.storage_version, JSONL_STORAGE_VERSION);
    assert_eq!(metadata.cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(metadata.parent_session_id.as_deref(), Some("parent"));
    let path = metadata.path.clone().unwrap();
    assert!(path.contains("sessions"), "{path}");
    assert!(path.ends_with("_child.jsonl"), "{path}");
    assert!(metadata.modified_at.unwrap().is_finite());
    session.close(background_context()).await.unwrap();

    let other = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/other".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert!(other.is_empty());
    let listed = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some(cwd.clone()),
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "child");
    // Byte-format header line.
    let lines = file_system
        .read_text_lines(&path, Some(&Default::default()), background_context())
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!({
            "v": 4,
            "kind": "header",
            "id": "child",
            "storageVersion": JSONL_STORAGE_VERSION,
            "createdAt": NOW,
            "cwd": cwd,
            "parentSessionId": "parent",
        })
    );
    repo.close(background_context()).await.unwrap();
}

/// "keeps an explicit Session mutation through commit until end"
/// (`jsonl-session-repo.test.ts:91-114`).
#[tokio::test]
async fn holds_explicit_mutation_until_end() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let session = repo
        .create(
            create_options("session", "/workspace"),
            background_context(),
        )
        .await
        .unwrap();
    let mutation = session.begin_mutation(background_context()).await.unwrap();
    // A queued mutate stays queued until end.
    let queued = {
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            crate::agent_core::harness::session::types::Session::mutate(
                &*session,
                |_mutator, _ctx| Box::pin(async { Ok(()) }),
                background_context(),
            )
            .await
        })
    };
    let result = mutation
        .commit(
            vec![set_value(&session_name(), serde_json::json!("explicit"))],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(result.seqs.len(), 1);
    let stored = crate::agent_core::harness::session::types::SessionMutationReader::get_value(
        mutation.as_ref(),
        &session_name(),
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(stored.unwrap().value, serde_json::json!("explicit"));
    mutation.end(background_context()).await.unwrap();
    queued.await.unwrap().unwrap();

    session.close(background_context()).await.unwrap();
    let reopened = repo
        .open(session.metadata(), background_context())
        .await
        .unwrap();
    assert_eq!(
        reopened.get_name(background_context()).await.unwrap(),
        Some("explicit".to_string())
    );
    reopened.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// `AtomicPublicationNodeExecutionEnv`
/// (`jsonl-session-repo.test.ts:11-29`): captures the staged content at the
/// publication rename.
#[derive(Clone)]
struct AtomicPublicationEnv {
    inner: Arc<NodeExecutionEnv>,
    publication: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
}

impl FileSystem for AtomicPublicationEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }

    fn absolute_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.absolute_path(path, context)
    }

    fn join_path<'a>(
        &'a self,
        parts: &[String],
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.join_path(parts, context)
    }

    fn read_text_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.read_text_file(path, context)
    }

    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn crate::agent_core::harness::types::TextLineReader>, FileError>>
    {
        self.inner.open_text_line_reader(path, context)
    }

    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::ReadTextLinesOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        self.inner.read_text_lines(path, options, context)
    }

    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        self.inner.read_binary_file(path, context)
    }

    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.write_file(path, content, context)
    }

    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.append_file(path, content, context)
    }

    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        let inner = Arc::clone(&self.inner);
        let publication = Arc::clone(&self.publication);
        let source = source_path.to_string();
        let destination = destination_path.to_string();
        Box::pin(async move {
            // Capture the staged content and destination existence before
            // publishing (upstream `AtomicPublicationNodeExecutionEnv`).
            let destination_exists = inner.exists(&destination, context.clone()).await;
            let staged = inner.read_text_file(&source, context.clone()).await;
            if let (Ok(destination_existed), Ok(staged_content)) = (&destination_exists, &staged) {
                *publication.lock().unwrap() = Some(serde_json::json!({
                    "sourcePath": source,
                    "destinationPath": destination,
                    "destinationExisted": destination_existed,
                    "stagedContent": staged_content,
                }));
            }
            inner.rename_file(&source, &destination, context).await
        })
    }

    fn file_info<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<crate::agent_core::harness::types::FileInfo, FileError>> {
        self.inner.file_info(path, context)
    }

    fn list_dir<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<crate::agent_core::harness::types::FileInfo>, FileError>> {
        self.inner.list_dir(path, context)
    }

    fn canonical_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.canonical_path(path, context)
    }

    fn exists<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        self.inner.exists(path, context)
    }

    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::CreateDirOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.create_dir(path, options, context)
    }

    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::RemoveOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.remove(path, options, context)
    }

    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_dir(prefix, context)
    }

    fn create_temp_file<'a>(
        &'a self,
        options: Option<&crate::agent_core::harness::types::TempFileOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_file(options, context)
    }

    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()> {
        self.inner.cleanup(context)
    }
}

/// "atomically publishes a branchless session header"
/// (`jsonl-session-repo.test.ts:70-89`).
#[tokio::test]
async fn atomically_publishes_branchless_session_header() {
    let dir = tempfile::tempdir().unwrap();
    let inner = Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ));
    let env = AtomicPublicationEnv {
        inner: Arc::clone(&inner),
        publication: Arc::new(std::sync::Mutex::new(None)),
    };
    let cwd = resolved_cwd(&inner).await;
    let repo = Arc::new(JsonlSessionRepo::new(JsonlSessionRepoOptions {
        file_system: Arc::new(AtomicPublicationEnv {
            inner: Arc::clone(&inner),
            publication: Arc::clone(&env.publication),
        }),
        sessions_root: "sessions".to_string(),
        now: Some(Arc::new(|| NOW)),
    }));
    let session = repo
        .create(create_options("session", &cwd), background_context())
        .await
        .unwrap();

    let publication = env
        .publication
        .lock()
        .unwrap()
        .clone()
        .expect("Expected atomic session publication");
    assert_eq!(
        publication["destinationPath"],
        serde_json::json!(session.metadata().path)
    );
    assert_eq!(publication["destinationExisted"], serde_json::json!(false));
    let staged_content = publication["stagedContent"].as_str().unwrap();
    let lines: Vec<&str> = staged_content.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed["kind"], "header");
    assert_eq!(parsed["id"], "session");
    assert_eq!(
        inner
            .read_text_file(
                session.metadata().path.as_deref().unwrap(),
                background_context()
            )
            .await
            .unwrap(),
        staged_content
    );
    assert!(!inner
        .exists(
            publication["sourcePath"].as_str().unwrap(),
            background_context()
        )
        .await
        .unwrap());

    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "rejects unsupported storage versions without repairing a torn tail"
/// (`jsonl-session-repo.test.ts:116-137`).
#[tokio::test]
async fn rejects_unsupported_storage_version_without_repair() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let session = repo
        .create(create_options("future", "/workspace"), background_context())
        .await
        .unwrap();
    let metadata = session.metadata().clone();
    let path = metadata.path.clone().unwrap();
    session.close(background_context()).await.unwrap();

    let content = file_system
        .read_text_file(&path, background_context())
        .await
        .unwrap();
    let mut lines: Vec<String> = content.trim_end().split('\n').map(str::to_string).collect();
    let unsupported_version = JSONL_STORAGE_VERSION + 1;
    let mut header: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    header["storageVersion"] = serde_json::json!(unsupported_version);
    lines[0] = header.to_string();
    let unsupported_content = format!("{}\n{{\"kind\":\"entry\"", lines.join("\n"));
    file_system
        .write_file(
            &path,
            FileContent::Text(unsupported_content.clone()),
            background_context(),
        )
        .await
        .unwrap();

    let error = repo
        .open(&metadata, background_context())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains(&format!(
            "unsupported storage version {unsupported_version}"
        )),
        "{error}"
    );
    assert_eq!(
        file_system
            .read_text_file(&path, background_context())
            .await
            .unwrap(),
        unsupported_content
    );
    repo.close(background_context()).await.unwrap();
}

/// "keeps fork destinations claimed until close and rejects deleting open
/// sessions" (`jsonl-session-repo.test.ts:139-156`).
#[tokio::test]
async fn keeps_fork_destinations_claimed_until_close() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let source = repo
        .create(create_options("source", "/workspace"), background_context())
        .await
        .unwrap();
    let fork = repo
        .fork(
            source.metadata(),
            &ForkOptions::Tree {
                id: Some("fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();

    let error = repo
        .open(fork.metadata(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already open"), "{error}");
    let error = repo
        .delete(fork.metadata(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("open"), "{error}");
    fork.close(background_context()).await.unwrap();

    let reopened = repo
        .open(fork.metadata(), background_context())
        .await
        .unwrap();
    reopened.close(background_context()).await.unwrap();
    repo.delete(fork.metadata(), background_context())
        .await
        .unwrap();
    let error = repo
        .open(fork.metadata(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");

    source.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "rejects concurrent creates for the same working-directory id"
/// (`jsonl-session-repo.test.ts:158-174`).
#[tokio::test]
async fn rejects_concurrent_creates_for_same_cwd_id() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let first = repo.create(
        create_options("session", "/workspace"),
        background_context(),
    );
    let second = repo.create(
        create_options("session", "/workspace"),
        background_context(),
    );
    let (first, second) = tokio::join!(first, second);
    let fulfilled = [first.is_ok(), second.is_ok()]
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(fulfilled, 1);
    assert_eq!(
        repo.list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context()
        )
        .await
        .unwrap()
        .len(),
        1
    );
    repo.close(background_context()).await.unwrap();
}

/// "allows the same id to be active in different working directories"
/// (`jsonl-session-repo.test.ts:176-199`).
#[tokio::test]
async fn allows_same_id_in_different_cwds() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let env = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd_a = env
        .absolute_path("/workspace-a", background_context())
        .await
        .unwrap()
        .trim_end_matches(['/', '\\'])
        .to_string();
    let cwd_b = env
        .absolute_path("/workspace-b", background_context())
        .await
        .unwrap()
        .trim_end_matches(['/', '\\'])
        .to_string();
    let first = repo
        .create(
            create_options("shared", "/workspace-a"),
            background_context(),
        )
        .await
        .unwrap();
    let second = repo
        .create(
            create_options("shared", "/workspace-b"),
            background_context(),
        )
        .await
        .unwrap();
    assert_ne!(first.metadata().path, second.metadata().path);
    let error = repo
        .create(
            create_options("shared", "/workspace-a"),
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    let listed = repo.list(None, background_context()).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|metadata| (metadata.cwd.clone().unwrap(), metadata.id.clone()))
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            (cwd_a.clone(), "shared".to_string()),
            (cwd_b, "shared".to_string())
        ])
    );

    first.close(background_context()).await.unwrap();
    second.close(background_context()).await.unwrap();
    let reopened_first = repo
        .open(first.metadata(), background_context())
        .await
        .unwrap();
    let reopened_second = repo
        .open(second.metadata(), background_context())
        .await
        .unwrap();
    reopened_first.close(background_context()).await.unwrap();
    reopened_second.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

// --- legacy v3 migration (jsonl-v3-migration.test.ts) ------------------------

/// The v3 header line shared by the migration fixtures.
pub(crate) fn v3_header(parent_session: Option<&str>, cwd: &str) -> String {
    let mut header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": "legacy",
        "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW),
        "cwd": cwd,
    });
    if let Some(parent_session) = parent_session {
        header["parentSession"] = serde_json::json!(parent_session);
    }
    header.to_string()
}

pub(crate) async fn write_legacy_v3_fixture(
    dir: &tempfile::TempDir,
    records: &[String],
    parent_session: Option<&str>,
    cwd: &str,
) -> (String, String) {
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    // The repo's `sessionDirectoryName` encoding of the cwd (`repo.ts:36-38`).
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
    let directory = file_system
        .join_path(
            &["sessions".to_string(), format!("--{encoded}--")],
            background_context(),
        )
        .await
        .unwrap();
    file_system
        .create_dir(&directory, None, background_context())
        .await
        .unwrap();
    let joined = file_system
        .join_path(
            &[directory, "legacy.jsonl".to_string()],
            background_context(),
        )
        .await
        .unwrap();
    let path = file_system
        .absolute_path(&joined, background_context())
        .await
        .unwrap();
    let mut lines = vec![v3_header(parent_session, cwd)];
    lines.extend(records.iter().cloned());
    let content = format!("{}\n", lines.join("\n"));
    file_system
        .write_file(
            &path,
            FileContent::Text(content.clone()),
            background_context(),
        )
        .await
        .unwrap();
    (path, content)
}

pub(crate) fn legacy_message(
    id: &str,
    parent_id: Option<&str>,
    text: &str,
    offset_ms: i64,
) -> String {
    serde_json::json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + offset_ms),
        "message": { "role": "user", "content": [{ "type": "text", "text": text }], "timestamp": NOW + offset_ms },
    })
    .to_string()
}

/// "discovers legacy v3 session files without rewriting them"
/// (`jsonl-v3-migration.test.ts:91-109`).
#[tokio::test]
async fn discovers_legacy_v3_without_rewriting() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    let (path, content) =
        write_legacy_v3_fixture(&dir, &[], Some("/old-session.jsonl"), &cwd).await;

    let listed = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    let metadata = &listed[0];
    assert_eq!(metadata.id, "legacy");
    assert_eq!(metadata.created_at, NOW);
    assert_eq!(metadata.storage_version, JSONL_STORAGE_VERSION);
    assert_eq!(metadata.cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(metadata.path.as_deref(), Some(path.as_str()));
    assert_eq!(
        metadata.legacy_parent_session_path.as_deref(),
        Some("/old-session.jsonl")
    );
    assert!(metadata.modified_at.unwrap().is_finite());
    assert_eq!(
        file_system
            .read_text_file(&path, background_context())
            .await
            .unwrap(),
        content
    );
    repo.close(background_context()).await.unwrap();
}

/// "opens an empty legacy session with a data-only main Branch"
/// (`jsonl-v3-migration.test.ts:429-440`).
#[tokio::test]
async fn opens_empty_legacy_session() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(&dir, &[], None, &cwd).await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let session: Arc<StorageBackedSession> =
        repo.open(&metadata, background_context()).await.unwrap();
    assert!(session
        .find_entries(None, background_context())
        .await
        .unwrap()
        .is_empty());
    let tip = session
        .get_branch_tip("main", background_context())
        .await
        .unwrap();
    assert_eq!(tip, None);
    assert!(session
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap()
        .is_none());
    assert!(session
        .get_value(&lane_config("main"), background_context())
        .await
        .unwrap()
        .is_none());
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "remaps a legacy message chain and exposes it through current APIs"
/// (`jsonl-v3-migration.test.ts:1882-1939`): reminted UUIDv7 ids, preserved
/// timestamps and payloads, tip, labels, and the branch walk.
#[tokio::test]
async fn remaps_legacy_message_chain() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(
        &dir,
        &[
            legacy_message("message-1", None, "first", 1_000),
            legacy_message("message-2", Some("message-1"), "second", 2_000),
        ],
        None,
        &cwd,
    )
    .await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let session: Arc<StorageBackedSession> =
        repo.open(&metadata, background_context()).await.unwrap();
    let entries = session
        .find_entries(
            Some(&crate::agent_core::harness::session::types::EntryQuery {
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Asc),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    let first = &entries[0];
    let second = &entries[1];
    assert_eq!(first.parent_id(), None);
    assert_eq!(first.seq(), 1);
    assert_eq!(first.timestamp(), NOW + 1_000);
    // Reminted UUIDv7 with the legacy timestamp.
    let hex = first.id().replace('-', "");
    let decoded = i64::from_str_radix(&hex[..12], 16).unwrap();
    assert_eq!(decoded, NOW + 1_000);
    assert_eq!(second.parent_id(), Some(first.id()));
    assert_eq!(second.seq(), 2);
    assert_eq!(second.timestamp(), NOW + 2_000);
    // Tip + branch walk + stats.
    let tip = session
        .get_branch_tip("main", background_context())
        .await
        .unwrap();
    assert_eq!(tip.as_deref(), Some(second.id()));
    let branch = session
        .branch("main", background_context())
        .await
        .unwrap()
        .unwrap();
    let branch_entries = branch
        .find_entries(
            Some(&crate::agent_core::harness::session::types::BranchScan {
                order: Some(
                    crate::agent_core::harness::session::types::BranchScanOrder::OldestFirst,
                ),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        branch_entries
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>(),
        vec![first.id(), second.id()]
    );
    assert_eq!(
        session
            .get_stats(background_context())
            .await
            .unwrap()
            .message_count,
        2
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "imports a label for its remapped entry without retaining a tree node"
/// (`jsonl-v3-migration.test.ts:1058-1092`).
#[tokio::test]
async fn imports_label_for_remapped_entry() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(
        &dir,
        &[
            legacy_message("message-1", None, "label me", 1_000),
            serde_json::json!({
                "type": "label", "id": "label-1", "parentId": "message-1",
                "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + 2_000),
                "targetId": "message-1", "label": "Important",
            })
            .to_string(),
        ],
        None,
        &cwd,
    )
    .await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let session: Arc<StorageBackedSession> =
        repo.open(&metadata, background_context()).await.unwrap();
    let entries = session
        .find_entries(None, background_context())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    let stored = session
        .get_value(&entry_label(entry.id()), background_context())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.value, serde_json::json!("Important"));
    let tip = session
        .get_branch_tip("main", background_context())
        .await
        .unwrap();
    assert_eq!(tip.as_deref(), Some(entry.id()));
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "converts to v4 and preserves the first caller transaction with one
/// usage adjustment" (`jsonl-v3-migration.test.ts:592-668`), at the storage
/// level.
#[tokio::test]
async fn upgrade_writes_import_adjustment_and_caller_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Arc::clone(&repo_in(&dir));
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(
        &dir,
        &[legacy_message("assistant", None, "imported answer", 1_000)],
        None,
        &cwd,
    )
    .await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let path = metadata.path.clone().unwrap();
    // Open at the storage level and capture the normalized view.
    let storage = crate::agent_core::harness::session::jsonl::storage::JsonlStorage::open(
        crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
            file_system: Arc::new(file_system.clone()),
            path: path.clone(),
            now: Some(Arc::new(|| NOW)),
        },
        background_context(),
    )
    .await
    .unwrap();
    let imported_entries = storage
        .scan_entries(&Default::default(), background_context())
        .await
        .unwrap();
    let stats_before = storage.get_stats(background_context()).await.unwrap();

    // The first caller write triggers conversion and must not expose the
    // internal adjustment sequence.
    let committed = storage
        .commit(
            vec![set_value(
                &session_name(),
                serde_json::json!("Converted session"),
            )],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(committed.seqs.len(), 1);
    assert_eq!(committed.first_seq, committed.seqs[0]);
    let usage_rows = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(usage_rows.len(), 1);
    let adjustment = &usage_rows[0];
    assert!(adjustment.adjustment);
    assert_eq!(
        adjustment.details,
        Some(serde_json::json!({ "source": "v3-import" }))
    );
    assert_eq!(adjustment.usage.total_tokens, 0);
    assert!(!committed.seqs.contains(&adjustment.seq));
    assert_eq!(committed.stats, stats_before);
    assert_eq!(
        storage.get_stats(background_context()).await.unwrap(),
        stats_before
    );

    // The converted file: v4 header, imported records, then the adjustment +
    // caller transaction as the final line.
    let content = file_system
        .read_text_file(&path, background_context())
        .await
        .unwrap();
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    let parsed_header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    // Upstream toMatchObject: header fields plus the upgrade nextSeq mark.
    assert_eq!(parsed_header["v"], serde_json::json!(4));
    assert_eq!(parsed_header["kind"], "header");
    assert_eq!(parsed_header["id"], "legacy");
    assert_eq!(
        parsed_header["storageVersion"],
        serde_json::json!(JSONL_STORAGE_VERSION)
    );
    assert_eq!(parsed_header["createdAt"], serde_json::json!(NOW));
    assert_eq!(parsed_header["cwd"], serde_json::json!(cwd));
    assert_eq!(parsed_header["nextSeq"], serde_json::json!(5));
    let transaction: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    let transaction = transaction.as_array().unwrap();
    assert_eq!(transaction.len(), 2);
    assert_eq!(transaction[0]["kind"], "usage");
    assert_eq!(transaction[0]["adjustment"], true);
    assert_eq!(
        transaction[0]["details"],
        serde_json::json!({ "source": "v3-import" })
    );
    assert_eq!(
        transaction[1],
        serde_json::json!({
            "kind": "value",
            "op": "set",
            "seq": committed.seqs[0],
            "namespace": "pi.session.name",
            "key": "",
            "value": "Converted session",
        })
    );
    storage.close(background_context()).await.unwrap();

    // Reopening through the ordinary v4 path recovers the complete state.
    let reopened = crate::agent_core::harness::session::jsonl::storage::JsonlStorage::open(
        crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
            file_system: Arc::new(file_system.clone()),
            path: path.clone(),
            now: Some(Arc::new(|| NOW)),
        },
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .scan_entries(&Default::default(), background_context())
            .await
            .unwrap(),
        imported_entries
    );
    let imported_entry = &imported_entries[0];
    assert_eq!(
        reopened
            .get_value(
                &crate::agent_core::harness::session::values::branch_tip("main"),
                background_context()
            )
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!(imported_entry.id())
    );
    assert!(reopened
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        reopened
            .get_value(&session_name(), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("Converted session")
    );
    assert_eq!(
        reopened
            .scan_usage(&Default::default(), background_context())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        reopened.get_stats(background_context()).await.unwrap(),
        stats_before
    );
    reopened.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "forks a closed source into a complete v4 destination without rewriting
/// it" (`jsonl-v3-migration.test.ts:276-304`), compact form.
#[tokio::test]
async fn forks_closed_legacy_source_into_v4() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    let (path, content) = write_legacy_v3_fixture(
        &dir,
        &[
            legacy_message("message-1", None, "fork me", 1_000),
            legacy_message("message-2", Some("message-1"), "forked", 4_000),
        ],
        None,
        &cwd,
    )
    .await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions {
                cwd: Some("/workspace".to_string()),
            }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let fork = repo
        .fork(
            &metadata,
            &ForkOptions::Tree {
                id: Some("closed-fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();

    // Source untouched.
    assert_eq!(
        file_system
            .read_text_file(&path, background_context())
            .await
            .unwrap(),
        content
    );
    assert_eq!(fork.metadata().id, "closed-fork");
    assert_eq!(fork.metadata().parent_session_id.as_deref(), Some("legacy"));
    let entries = fork
        .find_entries(
            Some(&crate::agent_core::harness::session::types::EntryQuery {
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Asc),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].parent_id(), None);
    assert_eq!(entries[1].parent_id(), Some(entries[0].id()));
    assert_ne!(entries[0].id(), "message-1");
    assert_ne!(entries[1].id(), "message-2");
    let tip = fork
        .get_branch_tip("main", background_context())
        .await
        .unwrap();
    assert_eq!(tip.as_deref(), Some(entries[1].id()));
    assert_eq!(
        fork.get_stats(background_context())
            .await
            .unwrap()
            .message_count,
        2
    );
    fork.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// Compile-time reference keeper for the trait imports used above.
#[allow(unused)]
fn _keepers(_e: Option<FileError>, _f: BoxFuture<'static, ()>, _c: Context) {}
