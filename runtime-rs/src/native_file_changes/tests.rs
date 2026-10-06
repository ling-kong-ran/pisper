use super::*;
use tokio::time::{sleep, Duration};

struct Fixture {
    root: PathBuf,
    cwd: PathBuf,
    data: PathBuf,
    service: Arc<FileChangesService>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pisper-native-file-changes-{}",
            uuid::Uuid::new_v4()
        ));
        let cwd = root.join("workspace");
        let data = root.join("data");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&data).unwrap();
        let service = FileChangesService::open(&data).unwrap();
        Self {
            root,
            cwd,
            data,
            service,
        }
    }
    async fn tracked(&self, id: &str) {
        self.service
            .mark_session_tracked(id, &self.cwd)
            .await
            .unwrap();
    }
    async fn write(&self, id: &str, path: &str, text: impl AsRef<[u8]>) {
        let ticket = self
            .service
            .before_tool(id, &self.cwd, "write", &json!({"path":path}))
            .await
            .unwrap()
            .unwrap();
        fs::write(self.cwd.join(path), text).unwrap();
        ticket.finish(true).await.unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
        if let Ok(root) = fs::canonicalize(&self.root) {
            assert_eq!(root.parent(), Some(temp.as_path()));
            assert!(root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-native-file-changes-"));
            let _ = fs::remove_dir_all(root);
        }
    }
}
#[tokio::test]
async fn first_baseline_counts_diff_approval_and_flattened_revert_survive_restart() {
    let f = Fixture::new();
    f.tracked("s").await;
    fs::write(f.cwd.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    f.write("s", "a.txt", "one\nTWO\nthree\nfour\n").await;
    let list = f.service.list("s", &f.cwd).await.unwrap();
    assert_eq!(
        list["summary"],
        json!({"files":1,"pending":1,"added":2,"removed":1})
    );
    assert_eq!(list["files"][0]["status"], "modified");
    let diff = f.service.diff("s", &f.cwd, "a.txt").await.unwrap();
    assert!(diff["diff"].as_str().unwrap().contains("--- a/a.txt\n"));
    assert!(diff["diff"].as_str().unwrap().contains("+TWO\n"));
    assert_eq!(
        f.service.approve("s", &f.cwd, None).await.unwrap()["summary"]["pending"],
        0
    );
    f.write("s", "a.txt", "second\n").await;
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["files"][0]["changeCount"],
        2
    );
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["summary"]["pending"],
        1
    );
    let revived = FileChangesService::open(&f.data).unwrap();
    let reverted = revived.revert("s", &f.cwd, Some("a.txt")).await.unwrap();
    assert_eq!(reverted["reverted"], 1);
    assert_eq!(reverted["summary"]["pending"], 0);
    assert_eq!(
        fs::read_to_string(f.cwd.join("a.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    assert_eq!(
        revived.summary("s", &f.cwd).await.unwrap()["changedFiles"],
        0
    );
    let index: Value = serde_json::from_slice(&fs::read(revived.index_path("s")).unwrap()).unwrap();
    assert_eq!(index["version"], 2);
    assert_eq!(index["coverage"], "complete");
    assert_eq!(
        revived.session_dir("s").file_name().unwrap(),
        store::key("s", 32).as_str()
    );
    assert_eq!(index["entries"][0]["key"], store::key("a.txt", 24));
}
#[tokio::test]
async fn new_file_line_count_and_missing_diff_exact_wire() {
    let f = Fixture::new();
    f.tracked("new").await;
    f.write("new", "new.md", "# hello\n").await;
    let list = f.service.list("new", &f.cwd).await.unwrap();
    assert_eq!(list["files"][0]["added"], 2);
    assert_eq!(list["files"][0]["status"], "created");
    let diff = f.service.diff("new", &f.cwd, "new.md").await.unwrap();
    assert!(diff["diff"]
        .as_str()
        .unwrap()
        .contains("new file mode 100644\n--- /dev/null\n"));
    assert_eq!(
        f.service
            .diff("new", &f.cwd, "untracked.txt")
            .await
            .unwrap(),
        json!({"diff":"","diffTruncated":false,"source":"snapshot","found":false})
    );
    assert_eq!(
        f.service.revert("new", &f.cwd, None).await.unwrap()["reverted"],
        1
    );
    assert!(!f.cwd.join("new.md").exists());
    assert_eq!(
        f.service.summary("new", &f.cwd).await.unwrap()["changedFiles"],
        0
    );
}
#[tokio::test]
async fn fresh_marker_is_known_history_and_legacy_indexes_are_not_zero() {
    let f = Fixture::new();
    assert_eq!(
        f.service.summary("history", &f.cwd).await.unwrap(),
        summary_unknown("unavailable", 0, false)
    );
    f.tracked("fresh").await;
    let revived = FileChangesService::open(&f.data).unwrap();
    assert_eq!(
        revived.summary("fresh", &f.cwd).await.unwrap(),
        json!({"status":"known","changedFiles":0,"pendingFiles":0,"added":0,"removed":0,"unknownFiles":0,"capped":false})
    );
    store::atomic(&revived.index_path("legacy"), b"{\"entries\":[]}").unwrap();
    revived
        .mark_session_tracked("legacy", &f.cwd)
        .await
        .unwrap();
    assert_eq!(
        revived.summary("legacy", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert!(
        serde_json::from_slice::<Value>(&fs::read(revived.index_path("legacy")).unwrap())
            .unwrap()
            .get("version")
            .is_none()
    );
}
#[tokio::test]
async fn deleted_and_manually_restored_files_use_net_summary() {
    let f = Fixture::new();
    f.tracked("s").await;
    fs::write(f.cwd.join("existing"), "before\n").unwrap();
    f.write("s", "existing", "after\nextra\n").await;
    f.write("s", "created", "new\n").await;
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["changedFiles"],
        2
    );
    f.service
        .approve("s", &f.cwd, Some("existing"))
        .await
        .unwrap();
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["pendingFiles"],
        1
    );
    fs::remove_file(f.cwd.join("existing")).unwrap();
    let summary = f.service.summary("s", &f.cwd).await.unwrap();
    assert_eq!(summary["changedFiles"], 2);
    assert_eq!(summary["removed"], 1);
    fs::write(f.cwd.join("existing"), "before\n").unwrap();
    fs::remove_file(f.cwd.join("created")).unwrap();
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["changedFiles"],
        0
    );
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["summary"]["files"],
        2
    );
}
#[tokio::test]
async fn binary_and_oversize_baselines_record_real_changes_without_fake_restore() {
    let f = Fixture::new();
    f.tracked("s").await;
    fs::write(f.cwd.join("binary"), [0, 1, 2]).unwrap();
    f.write("s", "binary", [0, 3]).await;
    fs::write(
        f.cwd.join("large"),
        vec![b'x'; store::MAX_SNAPSHOT_BYTES + 1],
    )
    .unwrap();
    f.write("s", "large", "after\n").await;
    let list = f.service.list("s", &f.cwd).await.unwrap();
    for file in list["files"].as_array().unwrap() {
        assert_eq!(file["status"], "modified");
        assert_eq!(file["snapshot"], false);
        assert_eq!(file["canRevert"], false);
        assert_eq!(file["pending"], true);
    }
    assert_eq!(
        f.service.revert("s", &f.cwd, None).await.unwrap()["reverted"],
        0
    );
    assert_eq!(fs::read(f.cwd.join("binary")).unwrap(), [0, 3]);
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap(),
        summary_unknown("partial", 2, false)
    );
    assert_eq!(
        f.service.diff("s", &f.cwd, "binary").await.unwrap(),
        json!({"diff":"","diffTruncated":false,"source":"snapshot","found":true})
    );
}
#[tokio::test]
async fn original_bom_and_newline_bytes_are_restored_summary_normalizes_both_sides() {
    let f = Fixture::new();
    f.tracked("s").await;
    let before = "\u{feff}one\r\ntwo\r";
    fs::write(f.cwd.join("bom"), before).unwrap();
    f.write("s", "bom", "one\ntwo\n").await;
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["changedFiles"],
        0
    );
    f.service.revert("s", &f.cwd, None).await.unwrap();
    assert_eq!(fs::read_to_string(f.cwd.join("bom")).unwrap(), before);
    let (preview, _) = diff::preview("space file.txt", "before", "after", false);
    assert!(preview.starts_with("diff --git \"a/space file.txt\" \"b/space file.txt\"\n"));
    assert!(preview.contains("\\ No newline at end of file"));
    assert_eq!(
        diff::preview("same", "same\n", "same\n", false),
        (String::new(), false)
    );
}
#[tokio::test]
async fn summary_reports_file_line_total_and_missing_baseline_limits() {
    let f = Fixture::new();
    f.tracked("large").await;
    f.write("large", "large", "x".repeat(600 * 1024)).await;
    assert_eq!(
        f.service.summary("large", &f.cwd).await.unwrap(),
        summary_unknown("partial", 1, false)
    );
    f.tracked("lines").await;
    f.write("lines", "lines", "line\n".repeat(2100)).await;
    assert_eq!(
        f.service.summary("lines", &f.cwd).await.unwrap()["unknownFiles"],
        1
    );
    f.tracked("budget").await;
    for index in 0..6 {
        f.write("budget", &format!("chunk{index}"), vec![b'x'; 400 * 1024])
            .await;
    }
    assert_eq!(
        f.service.summary("budget", &f.cwd).await.unwrap()["unknownFiles"],
        1
    );
    f.tracked("missing").await;
    fs::write(f.cwd.join("baseline"), "before\n").unwrap();
    f.write("missing", "baseline", "after\n").await;
    fs::remove_file(
        f.service
            .snapshot_path("missing", &store::key("baseline", 24))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.service.summary("missing", &f.cwd).await.unwrap()["unknownFiles"],
        1
    );
}
#[tokio::test]
async fn tool_coverage_and_delegated_tools_are_persisted_before_execution() {
    let f = Fixture::new();
    f.tracked("s").await;
    for name in ["read", "grep", "find", "ls"] {
        assert!(f
            .service
            .before_tool("s", &f.cwd, name, &json!({}))
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["status"],
        "known"
    );
    assert!(f
        .service
        .before_tool("s", &f.cwd, "bash", &json!({"command":"synthetic"}))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        FileChangesService::open(&f.data)
            .unwrap()
            .summary("s", &f.cwd)
            .await
            .unwrap(),
        summary_unknown("partial", 0, false)
    );
    f.tracked("delegated").await;
    f.service
        .before_tool(
            "delegated",
            &f.cwd,
            "call_tool",
            &json!({"name":"read","arguments":{"path":"a"}}),
        )
        .await
        .unwrap();
    assert_eq!(
        f.service.summary("delegated", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    f.tracked("nested").await;
    let ticket = f
        .service
        .before_tool(
            "nested",
            &f.cwd,
            "call_tool",
            &json!({"name":" write ","arguments":{"path":"nested"}}),
        )
        .await
        .unwrap()
        .unwrap();
    fs::write(f.cwd.join("nested"), "written\n").unwrap();
    ticket.finish(true).await.unwrap();
    assert_eq!(
        f.service.list("nested", &f.cwd).await.unwrap()["files"][0]["path"],
        "nested"
    );
    f.tracked("invalid").await;
    f.service
        .before_tool("invalid", &f.cwd, "write", &json!({}))
        .await
        .unwrap();
    assert_eq!(
        f.service.summary("invalid", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert_eq!(
        write_operation(
            "call_tool",
            &json!({"name":"edit","arguments":{"path":" x "}})
        ),
        Some("x".into())
    );
}
#[tokio::test]
async fn capture_outside_workspace_is_partial_and_api_paths_are_rejected() {
    let f = Fixture::new();
    f.tracked("s").await;
    let outside = f.root.join("outside");
    fs::write(&outside, "secret fixture").unwrap();
    assert!(f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":outside}))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["summary"]["files"],
        0
    );
    for path in ["../outside", "", "..foo"] {
        assert!(store::relative(&f.cwd, path, true).is_err());
    }
    assert!(f.service.diff("s", &f.cwd, "../outside").await.is_err());
    assert!(f
        .service
        .revert("s", &f.cwd, Some("../outside"))
        .await
        .is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "secret fixture");
}
#[tokio::test]
async fn changed_workspace_legacy_and_tampered_indexes_stay_partial() {
    let f = Fixture::new();
    f.tracked("s").await;
    let other = f.root.join("other");
    fs::create_dir_all(&other).unwrap();
    assert_eq!(
        f.service.summary("s", &other).await.unwrap()["status"],
        "partial"
    );
    let ticket = f
        .service
        .before_tool("s", &other, "write", &json!({"path":"same"}))
        .await
        .unwrap()
        .unwrap();
    fs::write(other.join("same"), "written\n").unwrap();
    ticket.finish(true).await.unwrap();
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert_eq!(
        FileChangesService::open(&f.data)
            .unwrap()
            .summary("s", &other)
            .await
            .unwrap()["status"],
        "partial"
    );
    f.tracked("tamper").await;
    let mut index: Value =
        serde_json::from_slice(&fs::read(f.service.index_path("tamper")).unwrap()).unwrap();
    index["entries"] = json!([{"path":"../outside","key":"000000000000000000000000","beforeExists":false,"snapshot":false}]);
    store::atomic(
        &f.service.index_path("tamper"),
        &serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.service.summary("tamper", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    let revived = FileChangesService::open(&f.data).unwrap();
    assert_eq!(
        revived.summary("tamper", &f.cwd).await.unwrap()["unknownFiles"],
        1
    );
    assert!(revived.list("tamper", &f.cwd).await.is_err());
}
#[tokio::test]
async fn file_cap_and_retention_separate_fifty_snapshots_from_five_hundred_markers() {
    let f = Fixture::new();
    f.tracked("cap").await;
    for index in 0..201 {
        f.write("cap", &format!("file{index}"), "x\n").await;
    }
    assert_eq!(
        f.service.list("cap", &f.cwd).await.unwrap()["summary"]["files"],
        200
    );
    let summary = f.service.summary("cap", &f.cwd).await.unwrap();
    assert_eq!(summary["capped"], true);
    assert_eq!(summary["changedFiles"], Value::Null);
    for index in 0..50 {
        let id = format!("snapshot{index}");
        f.tracked(&id).await;
        f.write(&id, "retained", "x\n").await;
    }
    for index in 0..501 {
        f.tracked(&format!("empty{index}")).await;
    }
    FileChangesService::open(&f.data).unwrap();
    let mut snapshots = 0;
    let mut markers = 0;
    for dir in fs::read_dir(&f.service.root).unwrap().flatten() {
        let index: Value =
            serde_json::from_slice(&fs::read(dir.path().join("index.json")).unwrap()).unwrap();
        if index["entries"].as_array().unwrap().is_empty() {
            markers += 1;
        } else {
            snapshots += 1;
        }
    }
    assert_eq!(snapshots, 50);
    assert_eq!(markers, 500);
}
#[tokio::test]
async fn parallel_writes_keep_first_baseline_and_drop_does_not_leak_session_lock() {
    let f = Fixture::new();
    f.tracked("s").await;
    fs::write(f.cwd.join("parallel"), "before\n").unwrap();
    let first = f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"parallel"}))
        .await
        .unwrap()
        .unwrap();
    let service = f.service.clone();
    let cwd = f.cwd.clone();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let second = tokio::spawn(async move {
        let ticket = service
            .before_tool("s", &cwd, "write", &json!({"path":"parallel"}))
            .await
            .unwrap()
            .unwrap();
        started_tx.send(()).unwrap();
        fs::write(cwd.join("parallel"), "second\n").unwrap();
        ticket.finish(true).await.unwrap();
    });
    sleep(Duration::from_millis(20)).await;
    assert!(started_rx.try_recv().is_err());
    fs::write(f.cwd.join("parallel"), "first\n").unwrap();
    first.finish(true).await.unwrap();
    second.await.unwrap();
    f.service.revert("s", &f.cwd, None).await.unwrap();
    assert_eq!(
        fs::read_to_string(f.cwd.join("parallel")).unwrap(),
        "before\n"
    );
    let ticket = f
        .service
        .before_tool("s", &f.cwd, "edit", &json!({"path":"parallel"}))
        .await
        .unwrap()
        .unwrap();
    drop(ticket);
    tokio::time::timeout(Duration::from_secs(2), f.service.wait_session("s"))
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn clear_waits_for_inflight_capture_and_blocks_queued_recreation() {
    let f = Fixture::new();
    f.tracked("s").await;
    let ticket = f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"result"}))
        .await
        .unwrap()
        .unwrap();
    let service = f.service.clone();
    let clear = tokio::spawn(async move {
        service.clear("s").await.unwrap();
    });
    sleep(Duration::from_millis(20)).await;
    assert!(!clear.is_finished());
    fs::write(f.cwd.join("result"), "actual\n").unwrap();
    ticket.finish(true).await.unwrap();
    clear.await.unwrap();
    assert!(!f.service.session_dir("s").exists());
    assert!(f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"result"}))
        .await
        .is_err());
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["status"],
        "unavailable"
    );
}
#[tokio::test]
async fn close_preserves_inflight_finished_index_and_rejects_new_capture() {
    let f = Fixture::new();
    f.tracked("s").await;
    let ticket = f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"result"}))
        .await
        .unwrap()
        .unwrap();
    let service = f.service.clone();
    let close = tokio::spawn(async move {
        service.close().await.unwrap();
    });
    sleep(Duration::from_millis(20)).await;
    assert!(!close.is_finished());
    fs::write(f.cwd.join("result"), "actual\n").unwrap();
    ticket.finish(true).await.unwrap();
    close.await.unwrap();
    assert_eq!(
        FileChangesService::open(&f.data)
            .unwrap()
            .list("s", &f.cwd)
            .await
            .unwrap()["files"][0]["changeCount"],
        1
    );
    assert!(f
        .service
        .before_tool("s", &f.cwd, "edit", &json!({"path":"result"}))
        .await
        .is_err());
}
#[tokio::test]
async fn failed_tool_keeps_original_snapshot_and_failed_partial_persistence_blocks_execution() {
    let f = Fixture::new();
    f.tracked("s").await;
    fs::write(f.cwd.join("failed"), "before\n").unwrap();
    let ticket = f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"failed"}))
        .await
        .unwrap()
        .unwrap();
    fs::write(f.cwd.join("failed"), "effect before failure\n").unwrap();
    ticket.finish(false).await.unwrap();
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["files"][0]["changeCount"],
        0
    );
    f.service.revert("s", &f.cwd, None).await.unwrap();
    assert_eq!(
        fs::read_to_string(f.cwd.join("failed")).unwrap(),
        "before\n"
    );
    f.tracked("storage-failure").await;
    fs::remove_dir_all(f.service.session_dir("storage-failure")).unwrap();
    fs::write(f.service.session_dir("storage-failure"), "blocking fixture").unwrap();
    assert!(f
        .service
        .before_tool("storage-failure", &f.cwd, "bash", &json!({}))
        .await
        .is_err());
}
#[test]
fn diff_truncation_counts_utf16_and_preserves_valid_wire_text() {
    let output = "😀".repeat(110_000);
    let (diff, truncated) = diff::preview("new.txt", "", &output, true);
    assert!(truncated);
    assert!(diff.encode_utf16().count() <= diff::MAX_DIFF_CHARS);
    assert!(
        serde_json::from_str::<Value>(&serde_json::to_string(&json!({"diff":diff})).unwrap())
            .is_ok()
    );
}
#[cfg(windows)]
#[tokio::test]
async fn windows_junction_boundary_does_not_capture_or_restore_outside_files() {
    let f = Fixture::new();
    f.tracked("junction").await;
    let outside = f.root.join("outside-dir");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("fixture.txt"), "outside fixture").unwrap();
    let link = f.cwd.join("link");
    // 普通 Windows 用户可建立目录 junction，不需要修改系统权限或真实配置。
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "隔离 junction 夹具建立失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(store::target(&f.cwd, "link/fixture.txt").is_err());
    assert!(store::target(&f.cwd, "link/new.txt").is_err());
    let ticket = f
        .service
        .before_tool(
            "junction",
            &f.cwd,
            "write",
            &json!({"path":"link/fixture.txt"}),
        )
        .await
        .unwrap()
        .unwrap();
    ticket.finish(false).await.unwrap();
    assert_eq!(
        f.service.summary("junction", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert_eq!(
        f.service.list("junction", &f.cwd).await.unwrap()["summary"]["files"],
        0
    );
    assert_eq!(
        fs::read_to_string(outside.join("fixture.txt")).unwrap(),
        "outside fixture"
    );
    let internal = f.cwd.join("internal");
    fs::create_dir_all(&internal).unwrap();
    fs::write(internal.join("file.txt"), "before\r\n").unwrap();
    let internal_link = f.cwd.join("internal-link");
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&internal_link)
        .arg(&internal)
        .output()
        .unwrap();
    assert!(output.status.success());
    f.tracked("internal-link").await;
    f.write("internal-link", "internal-link/file.txt", "after\n")
        .await;
    assert_eq!(
        f.service.summary("internal-link", &f.cwd).await.unwrap()["status"],
        "known"
    );
    f.service
        .revert("internal-link", &f.cwd, None)
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(internal.join("file.txt")).unwrap(),
        "before\r\n"
    );
    let snapshot = f.service.session_dir("snapshot-link");
    std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&snapshot)
        .arg(&outside)
        .output()
        .map(|output| assert!(output.status.success()))
        .unwrap();
    assert!(f
        .service
        .mark_session_tracked("snapshot-link", &f.cwd)
        .await
        .is_err());
    assert!(f.service.clear("snapshot-link").await.is_err());
    assert_eq!(
        fs::read_to_string(outside.join("fixture.txt")).unwrap(),
        "outside fixture"
    );
}
#[cfg(unix)]
#[tokio::test]
async fn outside_symlinks_never_snapshot_diff_or_revert_user_files() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.tracked("s").await;
    let outside = f.root.join("outside");
    fs::write(&outside, "outside fixture").unwrap();
    symlink(&outside, f.cwd.join("link")).unwrap();
    let ticket = f
        .service
        .before_tool("s", &f.cwd, "write", &json!({"path":"link"}))
        .await
        .unwrap()
        .unwrap();
    ticket.finish(false).await.unwrap();
    assert_eq!(
        f.service.summary("s", &f.cwd).await.unwrap()["status"],
        "partial"
    );
    assert_eq!(
        f.service.list("s", &f.cwd).await.unwrap()["summary"]["files"],
        0
    );
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside fixture");
    symlink(f.root.join("data"), f.cwd.join("outside-dir")).unwrap();
    assert!(store::target(&f.cwd, "outside-dir/new").is_err());
}
