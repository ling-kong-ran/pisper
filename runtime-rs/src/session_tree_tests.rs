#[cfg(test)]
mod tests {
    use pi_rust::agent_core::types::AgentMessage;
    use pi_rust::coding_agent::session_manager::{SessionEntry, SessionManager};

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pisper-prune-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn user(text: &str) -> AgentMessage {
        serde_json::from_value(serde_json::json!({
            "role": "user", "content": text, "timestamp": 1
        }))
        .unwrap()
    }

    fn fixture() -> (TestDir, SessionManager) {
        let dir = TestDir::new();
        let path = dir.path().to_string_lossy().to_string();
        let manager = SessionManager::create(&path, Some(&path), None).unwrap();
        (dir, manager)
    }

    #[test]
    fn prune_inactive_subtree_preserves_sibling_bytes_leaf_and_restart() {
        let (_dir, mut manager) = fixture();
        let root = manager.append_message(user("shared")).unwrap();
        let removed = manager.append_message(user("removed")).unwrap();
        let descendant = manager.append_message(user("descendant")).unwrap();
        manager.branch(&root).unwrap();
        let sibling = manager.append_message(user("surviving sibling")).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let before = std::fs::read_to_string(&path).unwrap();
        // 保留已有行的字节，包括外部写入器采用的空格，不能重序列化兄弟消息。
        let before = before.replace(
            "\"content\":\"surviving sibling\"",
            "\"content\" : \"surviving sibling\"",
        );
        std::fs::write(&path, &before).unwrap();
        manager.set_session_file(&path).unwrap();
        let sibling_line = before
            .lines()
            .find(|line| line.contains("surviving sibling"))
            .unwrap();
        let result = manager.delete_branch(&removed).unwrap();
        assert_eq!(
            result.deleted_entry_ids,
            vec![removed.clone(), descendant.clone()]
        );
        assert_eq!(result.leaf_id.as_deref(), Some(sibling.as_str()));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .any(|line| line == sibling_line));
        let reopened = SessionManager::open(&path, None, None).unwrap();
        assert!(reopened.get_entry(&removed).is_none());
        assert!(reopened.get_entry(&descendant).is_none());
        assert_eq!(reopened.get_leaf_id(), Some(sibling.as_str()));
        assert_eq!(reopened.get_branch(None).len(), 2);
    }

    #[test]
    fn prune_active_subtree_returns_to_parent_and_preserves_other_branch_after_restart() {
        let (_dir, mut manager) = fixture();
        let root = manager.append_message(user("shared")).unwrap();
        let sibling = manager.append_message(user("sibling")).unwrap();
        manager.branch(&root).unwrap();
        let removed = manager.append_message(user("removed")).unwrap();
        manager.append_message(user("child")).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        assert_eq!(
            manager.delete_branch(&removed).unwrap().leaf_id,
            Some(root.clone())
        );
        let mut reopened = SessionManager::open(&path, None, None).unwrap();
        assert_eq!(reopened.get_leaf_id(), Some(root.as_str()));
        assert!(reopened.get_entry(&sibling).is_some());
        let next = reopened
            .append_message(user("continue from shared"))
            .unwrap();
        assert_eq!(
            reopened.get_entry(&next).unwrap().parent_id(),
            Some(root.as_str())
        );
        reopened.delete_branch(&root).unwrap();
        let reopened = SessionManager::open(&path, None, None).unwrap();
        assert_eq!(reopened.get_leaf_id(), None);
        assert!(reopened.build_session_context().messages.is_empty());
        assert!(reopened
            .get_entries()
            .iter()
            .all(|entry| !matches!(entry, SessionEntry::Message(_))));
        assert_eq!(
            crate::session_api::project_tree(&reopened, false).unwrap()["nodeCount"],
            0
        );
    }

    #[test]
    fn prune_preserves_surviving_labels_and_clears_cross_branch_deleted_targets() {
        let (_dir, mut manager) = fixture();
        let root = manager.append_message(user("shared")).unwrap();
        manager
            .append_label_change(&root, Some("old label"))
            .unwrap();
        let removed = manager.append_message(user("removed")).unwrap();
        manager.append_label_change(&root, None).unwrap();
        manager.branch(&root).unwrap();
        let label = manager
            .append_label_change(&removed, Some("delete target"))
            .unwrap();
        let sibling = manager
            .append_message(user("survives metadata cleanup"))
            .unwrap();
        manager.delete_branch(&removed).unwrap();
        let reopened =
            SessionManager::open(manager.get_session_file().unwrap(), None, None).unwrap();
        assert_eq!(reopened.get_leaf_id(), Some(sibling.as_str()));
        assert_eq!(reopened.get_label(&removed), None);
        assert_eq!(reopened.get_label(&root), None);
        assert_eq!(
            reopened.get_entry(&sibling).unwrap().parent_id(),
            Some(label.as_str())
        );
        assert_eq!(reopened.get_branch(None).len(), 3);
        assert!(!reopened.get_entries().iter().any(
            |entry| matches!(entry, SessionEntry::Label(label) if label.target_id == removed)
        ));
    }

    #[test]
    fn prune_missing_or_metadata_target_leaves_memory_and_file_unchanged() {
        let (_dir, mut manager) = fixture();
        manager.append_message(user("keep")).unwrap();
        let metadata = manager.append_session_info("name").unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let before = std::fs::read(&path).unwrap();
        let entries = manager.get_entries();
        let leaf = manager.get_leaf_id().map(str::to_owned);
        for target in [
            "missing",
            metadata.as_str(),
            manager.get_session_id().to_string().as_str(),
        ] {
            assert!(manager.delete_branch(target).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(manager.get_entries(), entries);
            assert_eq!(manager.get_leaf_id(), leaf.as_deref());
        }
    }

    #[cfg(windows)]
    #[test]
    fn prune_failed_atomic_replace_preserves_original_file_memory_and_cleans_temporary() {
        use std::os::windows::fs::OpenOptionsExt;
        let (dir, mut manager) = fixture();
        let root = manager.append_message(user("keep")).unwrap();
        let removed = manager.append_message(user("remove")).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let before = std::fs::read(&path).unwrap();
        let entries = manager.get_entries();
        // 允许读取而拒绝删除共享，稳定地令最终 rename 失败，覆盖写完临时文件后的回滚。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(manager.delete_branch(&removed).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(manager.get_entries(), entries);
        assert_eq!(manager.get_leaf_id(), Some(removed.as_str()));
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pisper-prune-")));
        drop(lock);
        assert_eq!(manager.delete_branch(&removed).unwrap().leaf_id, Some(root));
    }

    #[test]
    fn prune_detects_external_journal_changes_without_overwriting_them() {
        let (_dir, mut manager) = fixture();
        let removed = manager.append_message(user("remove")).unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let mut external = SessionManager::open(&path, None, None).unwrap();
        external.append_message(user("external append")).unwrap();
        let before = std::fs::read(&path).unwrap();
        let entries = manager.get_entries();
        assert!(manager.delete_branch(&removed).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(manager.get_entries(), entries);
    }

    #[test]
    fn prune_includes_opaque_extension_descendants_and_preserves_other_branch_summaries() {
        use std::io::Write;
        let (_dir, mut manager) = fixture();
        let root = manager.append_message(user("shared")).unwrap();
        let removed = manager.append_message(user("removed")).unwrap();
        let summary = manager
            .branch_with_summary(Some(&root), "surviving branch summary", None, None, None)
            .unwrap();
        let label = manager
            .append_label_change(&removed, Some("cross reference"))
            .unwrap();
        manager.append_message(user("kept after metadata")).unwrap();
        let compaction = manager
            .append_compaction("surviving compaction", Some(&label), 25, None, None, None)
            .unwrap();
        let path = manager.get_session_file().unwrap().to_string();
        let summary_before = manager.get_entry(&summary).unwrap().clone();
        let compaction_before = manager.get_entry(&compaction).unwrap().clone();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for value in [
            serde_json::json!({"type":"future_extension","id":"opaque","parentId":removed,"timestamp":"2026-01-01T00:00:00Z","secret":"removed descendant"}),
            serde_json::json!({"type":"message","id":"after-opaque","parentId":"opaque","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"removed descendant","timestamp":1}}),
        ] {
            writeln!(file, "{value}").unwrap();
        }
        drop(file);
        manager.set_session_file(&path).unwrap();
        let result = manager.delete_branch(&removed).unwrap();
        assert_eq!(
            result.deleted_entry_ids,
            vec![removed, "opaque".into(), "after-opaque".into()]
        );
        let reopened = SessionManager::open(&path, None, None).unwrap();
        assert_eq!(reopened.get_leaf_id(), Some(root.as_str()));
        assert_eq!(reopened.get_entry(&summary), Some(&summary_before));
        assert_eq!(reopened.get_entry(&compaction), Some(&compaction_before));
        assert!(reopened.get_entry(&label).is_some());
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("removed descendant"));
        manager.branch(&compaction).unwrap();
        assert!(
            serde_json::to_string(&manager.build_session_context().messages)
                .unwrap()
                .contains("kept after metadata")
        );
    }

    #[test]
    fn prune_active_leaf_resolves_opaque_and_orphan_parents_before_persisting_position() {
        use std::io::Write;
        for parent in ["opaque", "missing"] {
            let (_dir, mut manager) = fixture();
            let root = manager.append_message(user("shared")).unwrap();
            let sibling = manager.append_message(user("surviving sibling")).unwrap();
            let path = manager.get_session_file().unwrap().to_string();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            for value in [
                serde_json::json!({"type":"future_extension","id":"opaque","parentId":root,"timestamp":"2026-01-01T00:00:00Z"}),
                serde_json::json!({"type":"message","id":"target","parentId":parent,"timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"removed","timestamp":1}}),
            ] {
                writeln!(file, "{value}").unwrap();
            }
            drop(file);
            manager.set_session_file(&path).unwrap();
            let expected = (parent == "opaque").then_some(root.as_str());
            let result = manager.delete_branch("target").unwrap();
            assert_eq!(result.leaf_id.as_deref(), expected);
            assert_eq!(manager.get_leaf_id(), expected);
            let reopened = SessionManager::open(&path, None, None).unwrap();
            assert_eq!(reopened.get_leaf_id(), expected);
            assert!(reopened.get_entry(&sibling).is_some());
        }
    }

    #[tokio::test]
    async fn prune_http_contract_rejects_busy_and_refreshes_hosted_context_and_read_cache() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;

        let dir = TestDir::new();
        let options = crate::runtime_host::EmbeddedOptions {
            agent_dir: dir.path().join("agent"),
            workspace: dir.path().join("workspace"),
            resource_dir: dir.path().join("resources"),
            token: "fixture-tree-delete-authentication-token".into(),
            store: false,
        };
        for name in [
            "agent/sessions",
            "workspace",
            "resources/shared/speech",
            "resources/shared/speech-resources",
        ] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        let shared = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../shared");
        for name in [
            "speech/speech-model-catalog.json",
            "speech/speech-resource-notices.json",
            "speech-resources/xasr-bpe.vocab",
        ] {
            std::fs::copy(
                shared.join(name),
                options.resource_dir.join("shared").join(name),
            )
            .unwrap();
        }
        let mut manager = SessionManager::create(
            &options.workspace.to_string_lossy(),
            Some(&options.agent_dir.join("sessions").to_string_lossy()),
            None,
        )
        .unwrap();
        let root = manager.append_message(user("shared HTTP context")).unwrap();
        let sibling = manager
            .append_message(user("sibling HTTP context"))
            .unwrap();
        manager.branch(&root).unwrap();
        let removed = manager
            .append_message(user("removed HTTP context"))
            .unwrap();
        let metadata = manager.append_session_info("tree fixture").unwrap();
        let id = manager.get_session_id().to_string();
        let path = manager.get_session_file().unwrap().to_string();
        let state = crate::boot(Some(&options)).await.unwrap();
        let hosted = crate::session_runtime::hosted(&state, &id).await.unwrap();
        hosted.session().state().error_message = Some("deleted branch error".into());
        let router = crate::session_router().with_state(state.clone());
        let request = |method: &str, suffix: &str| {
            Request::builder()
                .method(method)
                .uri(format!("/api/sessions/{id}/{suffix}"))
                .body(Body::empty())
                .unwrap()
        };
        // 先暖消息缓存，再删除，检验读缓存不会返回旧分支。
        let warm = router
            .clone()
            .oneshot(request("GET", "messages"))
            .await
            .unwrap();
        let warm_body = axum::body::to_bytes(warm.into_body(), usize::MAX)
            .await
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let run = hosted.run.clone().lock_owned().await;
        let busy = router
            .clone()
            .oneshot(request("DELETE", &format!("tree/branches/{removed}")))
            .await
            .unwrap();
        let busy_status = busy.status();
        let busy_body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(busy.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap_or_default();
        let busy_unchanged = std::fs::read(&path).unwrap() == before;
        drop(run);
        let missing = router
            .clone()
            .oneshot(request("DELETE", "tree/branches/missing"))
            .await
            .unwrap();
        let invalid = router
            .clone()
            .oneshot(request("DELETE", &format!("tree/branches/{metadata}")))
            .await
            .unwrap();
        let invalid_unchanged = std::fs::read(&path).unwrap() == before;
        let result = router
            .clone()
            .oneshot(request("DELETE", &format!("tree/branches/{removed}")))
            .await
            .unwrap();
        let status = result.status();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(result.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap_or_default();
        let current = router
            .clone()
            .oneshot(request("GET", "messages"))
            .await
            .unwrap();
        let current_body = axum::body::to_bytes(current.into_body(), usize::MAX)
            .await
            .unwrap();
        let hosted_context = serde_json::to_string(&hosted.session().messages()).unwrap();
        let hosted_error = hosted.session().state().error_message.clone();
        drop(router);
        drop(hosted);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::serve_runtime(state, listener, async {})
            .await
            .unwrap();
        let reopened = SessionManager::open(&path, None, None).unwrap();

        assert_eq!(busy_status, axum::http::StatusCode::CONFLICT);
        assert_eq!(busy_body["code"], "session_busy");
        assert!(busy_unchanged && invalid_unchanged);
        assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);
        assert_eq!(invalid.status(), axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["leafId"], root);
        assert!(body["deletedEntryIds"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(removed)));
        assert!(String::from_utf8_lossy(&warm_body).contains("removed HTTP context"));
        assert!(!String::from_utf8_lossy(&current_body).contains("removed HTTP context"));
        assert!(!hosted_context.contains("removed HTTP context"));
        assert_eq!(hosted_error, None);
        assert!(hosted_context.contains("shared HTTP context"));
        assert_eq!(reopened.get_leaf_id(), Some(root.as_str()));
        assert!(reopened.get_entry(&removed).is_none());
        assert!(reopened.get_entry(&sibling).is_some());
    }
}
