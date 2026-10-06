use super::*;
use serde_json::Map;
use std::sync::Mutex;
fn tree(base: &Path) -> Value {
    fn visit(base: &Path, dir: &Path, files: &mut Map<String, Value>) {
        let mut entries = fs::read_dir(dir)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_type().unwrap().is_dir() {
                visit(base, &entry.path(), files);
            } else {
                let key = entry
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(key, fs::read_to_string(entry.path()).unwrap().into());
            }
        }
    }
    let mut files = Map::new();
    visit(base, base, &mut files);
    Value::Object(files)
}
#[test]
fn filesystem_migrations_match_real_upstream_bytes_order_and_second_run() {
    let oracle: Value = serde_json::from_str(include_str!("migrations_oracle.json")).unwrap();
    let platform = if cfg!(windows) { "win32" } else { "posix" };
    let mut count = 0;
    for case in oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["platform"] == platform)
    {
        count += 1;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent = root.join("agent");
        let cwd = root.join("project");
        fs::create_dir_all(&agent).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        if let Some(dirs) = case["dirs"].as_array() {
            for dir in dirs {
                fs::create_dir_all(root.join(dir.as_str().unwrap())).unwrap();
            }
        }
        for (name, contents) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents.as_str().unwrap()).unwrap();
        }
        let messages = Mutex::new(Vec::<String>::new());
        let log = |s: &str| messages.lock().unwrap().push(s.into());
        let report = run_migrations(&cwd, &agent, &log).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            case["result"],
            "{} result",
            case["id"]
        );
        assert_eq!(tree(root), case["after"], "{} first bytes", case["id"]);
        let report = run_migrations(&cwd, &agent, &log).unwrap();
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            case["second"],
            "{} second result",
            case["id"]
        );
        assert_eq!(
            tree(root),
            case["afterSecond"],
            "{} second bytes",
            case["id"]
        );
        assert_eq!(
            serde_json::to_value(messages.into_inner().unwrap()).unwrap(),
            case["messages"],
            "{} stdout",
            case["id"]
        );
    }
    assert_eq!(count, 17);
}
#[test]
fn failed_oauth_archive_retains_collected_credentials_like_upstream() {
    let root = tempfile::tempdir().unwrap();
    let agent = root.path();
    let oauth = r#"{"fixture":{"access":"offline-test-only"}}"#;
    fs::write(agent.join("oauth.json"), oauth).unwrap();
    fs::create_dir(agent.join("oauth.json.migrated")).unwrap();
    assert_eq!(migrate_auth_to_auth_json(agent).unwrap(), vec!["fixture"]);
    assert_eq!(fs::read_to_string(agent.join("oauth.json")).unwrap(), oauth);
    let auth: Value =
        serde_json::from_str(&fs::read_to_string(agent.join("auth.json")).unwrap()).unwrap();
    assert_eq!(
        auth,
        json!({"fixture":{"type":"oauth","access":"offline-test-only"}})
    );
}
#[test]
fn duplicate_managed_binary_cleanup_is_nonrecursive() {
    let root = tempfile::tempdir().unwrap();
    let agent = root.path();
    fs::create_dir_all(agent.join("tools/fd")).unwrap();
    fs::create_dir_all(agent.join("bin")).unwrap();
    fs::write(agent.join("tools/fd/keep"), "important").unwrap();
    fs::write(agent.join("bin/fd"), "new").unwrap();
    migrate_tools_to_bin(agent, &|_| panic!("nothing moved")).unwrap();
    assert_eq!(
        fs::read_to_string(agent.join("tools/fd/keep")).unwrap(),
        "important"
    );
    assert_eq!(fs::read_to_string(agent.join("bin/fd")).unwrap(), "new");
}
#[cfg(unix)]
#[test]
fn created_auth_is_private_and_commands_symlink_itself_is_renamed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let agent = root.path().join("agent");
    fs::create_dir(&agent).unwrap();
    fs::write(
        agent.join("oauth.json"),
        r#"{"fixture":{"access":"offline"}}"#,
    )
    .unwrap();
    migrate_auth_to_auth_json(&agent).unwrap();
    assert_eq!(
        fs::metadata(agent.join("auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let target = root.path().join("real");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("prompt.md"), "retained").unwrap();
    symlink(&target, agent.join("commands")).unwrap();
    migrate_commands_to_prompts(&agent, "Global", &|_| {});
    assert!(fs::symlink_metadata(agent.join("prompts"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(agent.join("prompts")).unwrap(), target);
}

#[derive(Default)]
struct WarningUi {
    events: Mutex<Vec<Value>>,
    wait: bool,
}
impl MigrationWarningUi for WarningUi {
    fn log(&self, style: MigrationWarningStyle, text: &str) {
        let style = match style {
            MigrationWarningStyle::Warning => "warning",
            MigrationWarningStyle::Dim => "dim",
            MigrationWarningStyle::Plain => "plain",
        };
        self.events
            .lock()
            .unwrap()
            .push(json!(["log", style, text]));
    }
    fn set_raw_mode(&self, enabled: bool) {
        self.events.lock().unwrap().push(json!(["raw", enabled]));
    }
    fn resume(&self) {
        self.events.lock().unwrap().push(json!(["resume"]));
    }
    fn pause(&self) {
        self.events.lock().unwrap().push(json!(["pause"]));
    }
    fn wait_for_data(&self) -> futures::future::BoxFuture<'_, ()> {
        self.events.lock().unwrap().push(json!(["once", "data"]));
        Box::pin(async {
            if self.wait {
                futures::future::pending::<()>().await;
            }
        })
    }
}
#[tokio::test]
async fn deprecation_warning_text_style_and_input_trace_match_upstream() {
    let oracle: Value = serde_json::from_str(include_str!("migrations_oracle.json")).unwrap();
    let cases = oracle["warningCases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let warnings = serde_json::from_value::<Vec<String>>(case["warnings"].clone()).unwrap();
        let ui = WarningUi::default();
        show_deprecation_warnings(&warnings, &ui).await;
        assert_eq!(
            serde_json::to_value(ui.events.into_inner().unwrap()).unwrap(),
            case["trace"]
        );
    }
}
#[tokio::test]
async fn dropped_warning_future_restores_raw_input_without_printing_acknowledgement() {
    let ui = WarningUi {
        wait: true,
        ..Default::default()
    };
    let warnings = vec!["test".into()];
    let mut pending = Box::pin(show_deprecation_warnings(&warnings, &ui));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    drop(pending);
    let events = ui.events.lock().unwrap();
    assert_eq!(
        events[events.len() - 2..],
        [json!(["raw", false]), json!(["pause"])]
    );
}
