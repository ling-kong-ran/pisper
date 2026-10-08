use super::sessions::*;
use crate::coding_agent::{
    cli::args::Args, core::settings_manager::SettingsManager, session_manager::SessionManager,
};
use crate::coding_agent::{
    core::settings_manager::parse_settings_value, utils::paths::canonicalize_path,
};
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::Arc;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

pub(super) struct Fixture {
    pub _dir: tempfile::TempDir,
    pub root: String,
    pub cwd: String,
    pub other: String,
    pub sessions: String,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = canonicalize_path(dir.path().to_str().unwrap());
        let cwd = std::path::Path::new(&root)
            .join("work")
            .to_str()
            .unwrap()
            .to_owned();
        let other = std::path::Path::new(&root)
            .join("other")
            .to_str()
            .unwrap()
            .to_owned();
        let sessions = std::path::Path::new(&root)
            .join("sessions")
            .to_str()
            .unwrap()
            .to_owned();
        for p in [&cwd, &other, &sessions] {
            fs::create_dir_all(p).unwrap();
        }
        Self {
            _dir: dir,
            root,
            cwd,
            other,
            sessions,
        }
    }
    pub fn p(&self, s: &str) -> String {
        if let Some(suffix) = s.strip_prefix("/root") {
            let mut p = std::path::PathBuf::from(&self.root);
            for c in suffix.trim_start_matches('/').split('/') {
                p.push(c);
            }
            p.to_str().unwrap().into()
        } else {
            s.into()
        }
    }
    pub fn portable(&self, s: &str) -> String {
        s.replace(&self.root, "/root").replace('\\', "/")
    }
    pub fn seed(&self, id: &str, name: &str, cwd: &str, date: &str) -> String {
        let file = std::path::Path::new(&self.sessions).join(format!("{name}.jsonl"));
        let entries = [
            json!({"type":"session","version":3,"id":id,"timestamp":date,"cwd":cwd}),
            json!({"type":"message","id":"u","parentId":null,"timestamp":date,"message":{"role":"user","content":"seed","timestamp":1}}),
            json!({"type":"message","id":"a","parentId":"u","timestamp":date,"message":{"role":"assistant","content":[{"type":"text","text":"answer"}],"api":"faux","provider":"faux","model":"faux","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":2}}),
        ];
        fs::write(
            &file,
            entries
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        // Discovery orders by filesystem mtime, independently of JSONL timestamps.
        let day: u64 = date[8..10].parse().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(day * 86400),
                ),
            )
            .unwrap();
        file.to_str().unwrap().into()
    }
    fn settings(&self) -> SettingsManager {
        SettingsManager::in_memory(parse_settings_value("{}").unwrap())
    }
}
#[derive(Default)]
struct Ui {
    reports: Mutex<Vec<(StartupOutput, String)>>,
    confirm: bool,
    selection: Option<String>,
    fail: bool,
    hang: bool,
    stops: AtomicUsize,
}
impl SessionStartupUi for Ui {
    fn report(&self, kind: StartupOutput, s: &str) {
        self.reports.lock().unwrap().push((kind, s.into()));
    }
    fn confirm<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move { Ok(self.confirm) })
    }
    fn select_session<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
        _: &'a SettingsManager,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            if self.hang {
                std::future::pending::<()>().await;
            }
            if self.fail {
                anyhow::bail!("picker failed");
            }
            Ok(self.selection.clone())
        })
    }
    fn stop_theme_watcher(&self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
    }
}
fn opened(result: SessionSelection) -> Box<SessionManager> {
    match result {
        SessionSelection::Open(s) => s,
        SessionSelection::Exit(c) => panic!("unexpected exit {c}"),
    }
}
fn exit(result: SessionSelection, code: i32) {
    assert!(matches!(result,SessionSelection::Exit(c) if c==code));
}
#[test]
fn real_discovery_matches_upstream_exact_prefix_local_global_and_paths() {
    let oracle = super::options_tests::oracle();
    for case in oracle["resolution"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["platform"] == if cfg!(windows) { "win32" } else { "linux" })
    {
        let f = Fixture::new();
        let mut files = std::collections::HashMap::new();
        for (index, row) in case["local"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(case["global"].as_array().into_iter().flatten())
            .enumerate()
        {
            let name = row[1].as_str().unwrap();
            let file = f.seed(
                row[0].as_str().unwrap(),
                name,
                &f.p(row[2].as_str().unwrap()),
                &format!("2026-01-{:02}T00:00:00.000Z", 20 - index),
            );
            files.insert(file, name.to_owned());
        }
        let resolved = resolve_session_path(
            &f.p(case["arg"].as_str().unwrap()),
            &f.cwd,
            Some(&f.sessions),
        )
        .unwrap();
        let mut actual = serde_json::to_value(resolved).unwrap();
        for key in ["path", "cwd"] {
            if let Some(s) = actual[key].as_str() {
                let text = files.get(s).cloned().unwrap_or_else(|| f.portable(s));
                actual[key] = json!(text);
            }
        }
        assert_eq!(actual, case["value"], "{}", case["id"]);
    }
}
#[tokio::test]
async fn metadata_and_no_session_do_not_persist_requested_id() {
    for parsed in [
        Args {
            help: Some(true),
            ..Default::default()
        },
        Args {
            list_models: Some(None),
            ..Default::default()
        },
        Args {
            no_session: Some(true),
            ..Default::default()
        },
    ] {
        let f = Fixture::new();
        let parsed = Args {
            session_id: Some("fixed".into()),
            ..parsed
        };
        let session = opened(
            create_session_manager(
                &parsed,
                &f.cwd,
                Some(&f.sessions),
                &f.settings(),
                Arc::new(Ui::default()),
            )
            .await
            .unwrap(),
        );
        assert_eq!(session.get_session_id(), "fixed");
        assert!(session.get_session_file().is_none());
        assert_eq!(fs::read_dir(&f.sessions).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn explicit_local_path_opens_original_cwd_and_global_id_requires_confirmation() {
    let f = Fixture::new();
    let path = f.seed("global", "global-file", &f.other, "2026-01-01T00:00:00Z");
    let by_path = opened(
        create_session_manager(
            &Args {
                session: Some(path.clone()),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            Arc::new(Ui::default()),
        )
        .await
        .unwrap(),
    );
    assert_eq!(by_path.get_cwd(), f.other);
    let ui = Arc::new(Ui::default());
    exit(
        create_session_manager(
            &Args {
                session: Some("global".into()),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            ui.clone(),
        )
        .await
        .unwrap(),
        0,
    );
    assert_eq!(ui.reports.lock().unwrap().last().unwrap().1, "Aborted.");
    let ui = Arc::new(Ui {
        confirm: true,
        ..Default::default()
    });
    let fork = opened(
        create_session_manager(
            &Args {
                session: Some("global".into()),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            ui,
        )
        .await
        .unwrap(),
    );
    assert_eq!(fork.get_cwd(), f.cwd);
    assert_ne!(fork.get_session_id(), "global");
    assert_eq!(fork.build_session_context().messages.len(), 2);
    assert_eq!(
        fork.get_header().unwrap().parent_session.as_deref(),
        Some(path.as_str())
    );
}
#[tokio::test]
async fn fork_checks_target_collision_before_resolving_source_and_preserves_original() {
    let f = Fixture::new();
    let path = f.seed("occupied", "local-file", &f.cwd, "2026-01-01T00:00:00Z");
    let before = fs::read(&path).unwrap();
    let ui = Arc::new(Ui::default());
    let parsed = Args {
        fork: Some("missing".into()),
        session_id: Some("occupied".into()),
        ..Default::default()
    };
    exit(
        create_session_manager(
            &parsed,
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            ui.clone(),
        )
        .await
        .unwrap(),
        1,
    );
    assert_eq!(
        ui.reports.lock().unwrap()[0].1,
        "Session already exists with id 'occupied'"
    );
    let fork = opened(
        create_session_manager(
            &Args {
                fork: Some(path.clone()),
                session_id: Some("new-id".into()),
                ..Default::default()
            },
            &f.other,
            Some(&f.sessions),
            &f.settings(),
            ui,
        )
        .await
        .unwrap(),
    );
    assert_eq!(fork.get_session_id(), "new-id");
    assert_eq!(fork.get_cwd(), f.other);
    assert_eq!(fs::read(path).unwrap(), before);
}
#[tokio::test]
async fn exact_session_id_reopens_only_local_or_warns_and_creates() {
    let f = Fixture::new();
    f.seed("existing", "existing-file", &f.cwd, "2026-01-01T00:00:00Z");
    f.seed("remote", "remote-file", &f.other, "2026-01-02T00:00:00Z");
    let ui = Arc::new(Ui::default());
    let reopened = opened(
        create_session_manager(
            &Args {
                session_id: Some("existing".into()),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            ui.clone(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(reopened.build_session_context().messages.len(), 2);
    assert!(ui.reports.lock().unwrap().is_empty());
    let fresh = opened(
        create_session_manager(
            &Args {
                session_id: Some("remote".into()),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            ui.clone(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(fresh.get_cwd(), f.cwd);
    assert_eq!(fresh.get_session_id(), "remote");
    assert!(fresh.build_session_context().messages.is_empty());
    assert!(ui.reports.lock().unwrap()[0].1.starts_with("Warning:"));
}
#[tokio::test]
async fn continue_ignores_newer_other_project_and_missing_source_exits_one() {
    let f = Fixture::new();
    f.seed("local", "local-file", &f.cwd, "2026-01-01T00:00:00Z");
    f.seed("remote", "remote-file", &f.other, "2026-01-02T00:00:00Z");
    let session = opened(
        create_session_manager(
            &Args {
                r#continue: Some(true),
                ..Default::default()
            },
            &f.cwd,
            Some(&f.sessions),
            &f.settings(),
            Arc::new(Ui::default()),
        )
        .await
        .unwrap(),
    );
    assert_eq!(session.get_session_id(), "local");
    for parsed in [
        Args {
            session: Some("missing".into()),
            ..Default::default()
        },
        Args {
            fork: Some("missing".into()),
            ..Default::default()
        },
    ] {
        let ui = Arc::new(Ui::default());
        exit(
            create_session_manager(
                &parsed,
                &f.cwd,
                Some(&f.sessions),
                &f.settings(),
                ui.clone(),
            )
            .await
            .unwrap(),
            1,
        );
        assert_eq!(
            ui.reports.lock().unwrap()[0].1,
            "No session found matching 'missing'"
        );
    }
}
#[tokio::test]
async fn resume_stops_theme_watcher_on_open_cancel_error_and_dropped_future() {
    let f = Fixture::new();
    let path = f.seed("chosen", "chosen-file", &f.cwd, "2026-01-01T00:00:00Z");
    let parsed = Args {
        resume: Some(true),
        ..Default::default()
    };
    let settings = f.settings();
    for ui in [
        Ui {
            selection: Some(path),
            ..Default::default()
        },
        Ui::default(),
        Ui {
            fail: true,
            ..Default::default()
        },
        Ui {
            hang: true,
            ..Default::default()
        },
    ] {
        let ui = Arc::new(ui);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            create_session_manager(&parsed, &f.cwd, Some(&f.sessions), &settings, ui.clone()),
        )
        .await;
        if ui.hang {
            assert!(result.is_err());
        } else if ui.fail {
            assert!(result.unwrap().is_err());
        } else if ui.selection.is_some() {
            assert_eq!(opened(result.unwrap().unwrap()).get_session_id(), "chosen");
        } else {
            exit(result.unwrap().unwrap(), 0);
        }
        assert_eq!(ui.stops.load(Ordering::SeqCst), 1);
    }
}
