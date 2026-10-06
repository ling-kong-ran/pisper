//! Port of `packages/agent/test/harness/jsonl-v3-stream.test.ts` (161
//! lines): the streaming legacy v3 normalization — selected compaction
//! tails, cached tail messages across branches, and torn-tail capture.

use super::LegacyV3Source;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::commit::{CommittedValueWrite, CommittedWrite};
use crate::agent_core::harness::types::{FileError, FileSystem, TextLine, TextLineReader};
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

fn iso(ms: i64) -> String {
    crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(ms)
}

/// `ObservedEnv` (`jsonl-v3-stream.test.ts:34-54`): counts reader opens and
/// line reads.
struct ObservedEnv {
    inner: Arc<NodeExecutionEnv>,
    line_reads: AtomicUsize,
    self_arc: std::sync::Weak<ObservedEnv>,
}

impl ObservedEnv {
    fn new(inner: Arc<NodeExecutionEnv>) -> Arc<Self> {
        Arc::new_cyclic(|weak| ObservedEnv {
            inner,
            line_reads: AtomicUsize::new(0),
            self_arc: weak.clone(),
        })
    }

    fn line_reads(&self) -> usize {
        self.line_reads.load(Ordering::SeqCst)
    }
}

struct CountingReader {
    inner: Arc<dyn TextLineReader>,
    env: Arc<ObservedEnv>,
}

impl TextLineReader for CountingReader {
    fn read_line<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, Result<Option<TextLine>, FileError>> {
        self.env.line_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_line(context)
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, ()> {
        self.inner.close(context)
    }
}

impl FileSystem for ObservedEnv {
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
    ) -> BoxFuture<'a, Result<Arc<dyn TextLineReader>, FileError>> {
        let env = self
            .self_arc
            .upgrade()
            .expect("ObservedEnv outlives its readers");
        let inner = Arc::clone(&self.inner);
        let path = path.to_string();
        Box::pin(async move {
            let opened = inner.open_text_line_reader(&path, context).await?;
            Ok(Arc::new(CountingReader { inner: opened, env }) as Arc<dyn TextLineReader>)
        })
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
        content: crate::agent_core::harness::types::FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.write_file(path, content, context)
    }

    fn append_file<'a>(
        &'a self,
        path: &str,
        content: crate::agent_core::harness::types::FileContent,
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
        self.inner
            .rename_file(source_path, destination_path, context)
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

fn legacy_header() -> String {
    serde_json::json!({
        "type": "session",
        "version": 3,
        "id": "legacy",
        "timestamp": iso(NOW),
        "cwd": "/workspace",
    })
    .to_string()
}

fn message(id: &str, parent_id: Option<&str>, content: &str) -> String {
    serde_json::json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": iso(NOW),
        "message": { "role": "user", "content": content, "timestamp": NOW },
    })
    .to_string()
}

fn record(value: serde_json::Value) -> String {
    value.to_string()
}

async fn write_fixture(env: &ObservedEnv, path: &str, records: Vec<String>, suffix: &str) {
    let mut lines = vec![legacy_header()];
    lines.extend(records);
    let content = format!("{}\n{}", lines.join("\n"), suffix);
    env.inner
        .write_file(
            path,
            crate::agent_core::harness::types::FileContent::Text(content),
            background_context(),
        )
        .await
        .unwrap();
}

fn collect(writes: Vec<CommittedWrite>) -> Vec<CommittedWrite> {
    writes
}

fn value_sets(writes: &[CommittedWrite]) -> Vec<&CommittedValueWrite> {
    writes
        .iter()
        .filter_map(|write| match write {
            CommittedWrite::Value(value) => Some(value),
            _ => None,
        })
        .collect()
}

/// "materializes a selected compaction with its branch-local tail"
/// (`jsonl-v3-stream.test.ts:76-105`).
#[tokio::test]
async fn materializes_selected_compaction_with_branch_local_tail() {
    let dir = tempfile::tempdir().unwrap();
    let env = ObservedEnv::new(Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    )));
    write_fixture(
        &env,
        "legacy-0.jsonl",
        vec![
            message("before", None, "before"),
            record(serde_json::json!({
                "type": "session_info", "id": "boundary", "parentId": "before",
                "timestamp": iso(NOW),
            })),
            message("kept", Some("boundary"), "Unicode é漢字"),
            message("other", Some("before"), "not on this branch"),
            record(serde_json::json!({
                "type": "compaction", "id": "selected", "parentId": "kept",
                "timestamp": iso(NOW),
                "summary": "selected", "firstKeptEntryId": "boundary",
                "tokensBefore": 20, "fromHook": true, "details": { "a": 2 },
            })),
        ],
        "",
    )
    .await;
    let source = LegacyV3Source::read(
        env.clone() as Arc<dyn FileSystem>,
        "legacy-0.jsonl",
        background_context(),
    )
    .await
    .unwrap();
    let selected_id = source.entry_structures().last().unwrap().id.clone();
    // With nothing selected, the writes are exactly the derived values.
    let unselected = source
        .writes(background_context(), Some(&|_id: &str| false))
        .await
        .unwrap();
    assert_eq!(value_sets(&collect(unselected)), value_sets(&source.values));
    env.line_reads.store(0, Ordering::SeqCst);
    let writes = source
        .writes(background_context(), Some(&|id: &str| id == selected_id))
        .await
        .unwrap();
    assert_eq!(collect(writes.clone()).len(), 2);
    let CommittedWrite::Entry { entry } = &collect(writes)[0] else {
        panic!("expected entry write");
    };
    let crate::agent_core::harness::session::types::Entry::Compaction {
        summary,
        details,
        from_hook,
        retained_tail,
        ..
    } = &**entry
    else {
        panic!("expected compaction entry");
    };
    assert_eq!(summary, "selected");
    assert!(*from_hook);
    assert_eq!(*details, Some(serde_json::json!({ "a": 2 })));
    assert_eq!(entry.seq(), 4);
    assert_eq!(
        serde_json::to_value(retained_tail).unwrap(),
        serde_json::json!([{ "role": "user", "content": "Unicode é漢字", "timestamp": NOW }])
    );
    // Header plus each captured physical record, read once.
    assert_eq!(env.line_reads(), 6);
}

/// "reuses an earlier cached message in selected compaction tails on
/// different branches" (`jsonl-v3-stream.test.ts:107-147`).
#[tokio::test]
async fn reuses_cached_tail_messages_across_branches() {
    let dir = tempfile::tempdir().unwrap();
    let env = ObservedEnv::new(Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    )));
    write_fixture(
        &env,
        "legacy-1.jsonl",
        vec![
            message("root", None, "root"),
            message("left", Some("root"), "left"),
            record(serde_json::json!({
                "type": "compaction", "id": "left-compaction", "parentId": "left",
                "timestamp": iso(NOW), "summary": "left summary",
                "firstKeptEntryId": "root", "tokensBefore": 10,
            })),
            message("right", Some("root"), "right"),
            record(serde_json::json!({
                "type": "compaction", "id": "right-compaction", "parentId": "right",
                "timestamp": iso(NOW), "summary": "right summary",
                "firstKeptEntryId": "root", "tokensBefore": 20,
            })),
        ],
        "",
    )
    .await;
    let source = LegacyV3Source::read(
        env.clone() as Arc<dyn FileSystem>,
        "legacy-1.jsonl",
        background_context(),
    )
    .await
    .unwrap();
    let structures = source.entry_structures();
    let left_id = structures[2].id.clone();
    let right_id = structures[4].id.clone();
    let selected: std::collections::HashSet<String> = [left_id, right_id].into();
    env.line_reads.store(0, Ordering::SeqCst);
    let writes = source
        .writes(
            background_context(),
            Some(&|id: &str| selected.contains(id)),
        )
        .await
        .unwrap();
    let writes = collect(writes);
    assert_eq!(writes.len(), 3);
    let tail_of = |index: usize, expected: &[&str]| {
        let CommittedWrite::Entry { entry } = &writes[index] else {
            panic!("expected entry write");
        };
        let crate::agent_core::harness::session::types::Entry::Compaction { retained_tail, .. } =
            &**entry
        else {
            panic!("expected compaction entry");
        };
        let contents: Vec<String> = retained_tail
            .iter()
            .map(|message| match message {
                crate::agent_core::types::AgentMessage::User(user) => match &user.content {
                    crate::ai::types::message::StringOrBlocks::Text(text) => text.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            })
            .collect();
        assert_eq!(contents, expected);
    };
    tail_of(0, &["root", "left"]);
    tail_of(1, &["root", "right"]);
    assert_eq!(env.line_reads(), 6);
    // Repeatable passes share only the captured ids and metadata.
    let repeat = source
        .writes(
            background_context(),
            Some(&|id: &str| selected.contains(id)),
        )
        .await
        .unwrap();
    assert_eq!(collect(repeat), writes);
}

/// "ignores a torn tail and emits only the captured complete records after
/// later appends" (`jsonl-v3-stream.test.ts:149-160`).
#[tokio::test]
async fn ignores_torn_tail_and_captures_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let env = ObservedEnv::new(Arc::new(NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    )));
    write_fixture(
        &env,
        "legacy-2.jsonl",
        vec![message("a", None, "a")],
        &message("torn", Some("a"), "torn"),
    )
    .await;
    let source = LegacyV3Source::read(
        env.clone() as Arc<dyn FileSystem>,
        "legacy-2.jsonl",
        background_context(),
    )
    .await
    .unwrap();
    env.inner
        .append_file(
            "legacy-2.jsonl",
            crate::agent_core::harness::types::FileContent::Text(format!(
                "\n{}\n",
                message("later", Some("torn"), "later")
            )),
            background_context(),
        )
        .await
        .unwrap();
    env.line_reads.store(0, Ordering::SeqCst);
    let writes = source.writes(background_context(), None).await.unwrap();
    assert_eq!(collect(writes).len(), 2);
    // Header plus the single captured complete record.
    assert_eq!(env.line_reads(), 2);
    assert_eq!(source.next_seq, 3);
}
