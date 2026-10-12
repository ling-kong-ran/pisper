use super::*;
use futures::{
    task::{waker_ref, ArcWake},
    FutureExt, StreamExt,
};
use pi_rust::{
    agent_core::ThinkingLevel,
    ai::models::faux::{faux_assistant_message, faux_provider, FauxProviderOptions},
    coding_agent::{
        agent_session::ExtensionBindings,
        core::{
            agent_session_runtime::AgentSessionRuntime,
            agent_session_services::AgentSessionServices,
            resource_loader::{
                DefaultResourceLoader, DefaultResourceLoaderOptions, InlineExtension,
            },
            sdk::{create_agent_session, CreateAgentSessionOptions, NoTools},
            settings_manager::SettingsManager,
        },
        extensions::{loader::ExtensionFactory, types::HandlerFn},
        session_manager::SessionManager,
    },
};
use std::path::PathBuf;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Weak,
    },
    task::{Context, Poll},
};
use tokio::sync::{Notify, Semaphore};

struct RegistryReaderWaker {
    state: Weak<AppState>,
    unlocked: AtomicBool,
    wakes: AtomicUsize,
}
impl ArcWake for RegistryReaderWaker {
    fn wake_by_ref(probe: &Arc<Self>) {
        if let Some(state) = probe.state.upgrade() {
            if state.chat_runs.try_lock().is_err() {
                probe.unlocked.store(false, Ordering::Release);
            }
        }
        probe.wakes.fetch_add(1, Ordering::Relaxed);
    }
}
fn registry_reader(state: &Arc<AppState>) -> Arc<RegistryReaderWaker> {
    Arc::new(RegistryReaderWaker {
        state: Arc::downgrade(state),
        unlocked: AtomicBool::new(true),
        wakes: AtomicUsize::new(0),
    })
}

async fn hosted_fixture(state: &Arc<AppState>, extension: Option<ExtensionFactory>) -> String {
    let settings =
        SettingsManager::create_with(&state.cwd, &state.agent_dir, Default::default()).unwrap();
    let mut loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: state.cwd.clone(),
        agent_dir: state.agent_dir.clone(),
        settings_manager: Some(Arc::new(settings.clone())),
        extension_factories: extension
            .into_iter()
            .map(InlineExtension::Factory)
            .collect(),
        no_skills: true,
        no_prompt_templates: true,
        no_themes: true,
        no_context_files: true,
        ..Default::default()
    });
    loader.reload_without_trust().unwrap();
    let loader = Arc::new(Mutex::new(loader));
    let model_runtime = state.runtime.session().model_runtime().clone();
    let model = model_runtime
        .get_model("terminal-lease-fixture", "faux-1")
        .await
        .unwrap();
    let manager = Arc::new(Mutex::new(
        SessionManager::create(
            &state.cwd,
            Some(
                &PathBuf::from(&state.agent_dir)
                    .join("sessions")
                    .to_string_lossy(),
            ),
            None,
        )
        .unwrap(),
    ));
    let created = create_agent_session(CreateAgentSessionOptions {
        cwd: Some(state.cwd.clone()),
        agent_dir: Some(state.agent_dir.clone()),
        settings_manager: Some(settings.clone()),
        model_runtime: Some(model_runtime.clone()),
        model: Some(model),
        thinking_level: Some(ThinkingLevel::Off),
        no_tools: Some(NoTools::All),
        resource_loader: Some(loader.clone()),
        session_manager: Some(manager.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    crate::session_api::persist_empty_session(&mut manager.lock().unwrap()).unwrap();
    created
        .session
        .bind_extensions(ExtensionBindings::default())
        .await
        .unwrap();
    let runtime = Arc::new(AgentSessionRuntime::new(
        created.session,
        Arc::new(AgentSessionServices {
            cwd: state.cwd.clone(),
            agent_dir: state.agent_dir.clone(),
            settings_manager: settings,
            model_runtime,
            resource_loader: loader,
            diagnostics: Vec::new(),
        }),
        Arc::new(|_| Box::pin(async { anyhow::bail!("Fixture never replaces a session") })),
        Vec::new(),
        None,
    ));
    state
        .sessions
        .insert(state, runtime)
        .unwrap()
        .session()
        .session_id()
}

async fn isolated_state() -> (PathBuf, Arc<AppState>) {
    let dir = std::env::temp_dir().join(format!("pisper-terminal-lease-{}", uuid::Uuid::new_v4()));
    let agent = dir.join("agent");
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let state = crate::boot(Some(&crate::runtime_host::EmbeddedOptions {
        agent_dir: agent,
        workspace,
        resource_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."),
        token: "isolated-terminal-lease-fixture-token-12345".into(),
        store: false,
    }))
    .await
    .unwrap();
    // 本夹具验证 lease/SSE/JSONL/真实 sweep，不验证 memory 文件持久化或
    // memory 域的 Arc 生命周期；同一真实 SQLite store 使用隔离内存连接。
    *state.memory.lock().unwrap() =
        crate::memory_store::MemoryStore::open(":memory:", &state.cwd).unwrap();
    (dir, state)
}

// SSE 终态必须承诺会话已可接收下一条消息；慢速回收其他会话不能占住该租约。
#[tokio::test]
async fn terminal_sse_releases_run_before_unrelated_session_reclamation() {
    let (dir, state) = isolated_state().await;
    let faux = faux_provider(FauxProviderOptions {
        provider: Some("terminal-lease-fixture".into()),
        ..Default::default()
    });
    faux.set_responses(vec![
        faux_assistant_message("first persisted reply", Default::default()).into(),
        faux_assistant_message("second persisted reply", Default::default()).into(),
    ]);
    let model_runtime = state.runtime.session().model_runtime().clone();
    model_runtime
        .register_native_provider_sync(faux.provider.clone())
        .unwrap();
    model_runtime.get_available(None, None).await.unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let gate: ExtensionFactory = {
        let entered = entered.clone();
        let release = release.clone();
        Arc::new(move |api| {
            let entered = entered.clone();
            let release = release.clone();
            let handler: HandlerFn = Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.acquire().await.unwrap().forget();
                    Ok(None)
                })
            });
            api.on("session_shutdown", handler).map(|_| ())
        })
    };
    for _ in 0..3 {
        hosted_fixture(&state, Some(gate.clone())).await;
    }
    let id = hosted_fixture(&state, None).await;
    let first = chat_owned(
        state.clone(),
        json!({"sessionId":id,"message":"first"}),
        None,
    )
    .await
    .unwrap();
    let run_id = first.headers()["x-pisper-run-id"]
        .to_str()
        .unwrap()
        .to_string();
    let mut raw_events = state.chat_runs.lock().unwrap()[&run_id].tx.subscribe();
    let broadcast_probe = registry_reader(&state);
    let broadcast_waker = waker_ref(&broadcast_probe);
    let mut receive = Box::pin(raw_events.recv());
    assert!(matches!(
        receive
            .as_mut()
            .poll(&mut Context::from_waker(&broadcast_waker)),
        Poll::Pending
    ));
    let lease_probe = registry_reader(&state);
    let lease_waker = waker_ref(&lease_probe);
    let mut lease = Box::pin(state.sessions.get(&id).unwrap().run.clone().lock_owned());
    assert!(matches!(
        lease.as_mut().poll(&mut Context::from_waker(&lease_waker)),
        Poll::Pending
    ));
    let first_body = axum::body::to_bytes(first.into_body(), 1_000_000)
        .await
        .unwrap();
    let first_text = String::from_utf8(first_body.to_vec()).unwrap();
    assert!(
        first_text.contains("event: done"),
        "First chat must complete through real SSE"
    );
    // 取消等待者的锁预留，下一条真实请求仍需通过生产准入。
    drop(lease);
    drop(receive);
    assert!(broadcast_probe.wakes.load(Ordering::Acquire) > 0);
    assert!(lease_probe.wakes.load(Ordering::Acquire) > 0);
    assert!(
        broadcast_probe.unlocked.load(Ordering::Acquire),
        "Broadcast waker must re-enter the registry after unlock"
    );
    assert!(
        lease_probe.unlocked.load(Ordering::Acquire),
        "Lease waker must re-enter the registry after unlock"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("Real resident reclamation must enter its shutdown extension");
    let next = chat_owned(
        state.clone(),
        json!({"sessionId":id,"message":"second"}),
        None,
    )
    .await;
    // 先释放真实回收，再断言准入结果，RED 也不会遗留等待中的领域任务。
    release.add_permits(10);
    let admitted = match next {
        Ok(response) => {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            assert!(String::from_utf8(body.to_vec())
                .unwrap()
                .contains("second persisted reply"));
            Ok(status)
        }
        Err(error) => Err((error.status, error.code)),
    };
    let workers = state
        .chat_runs
        .lock()
        .unwrap()
        .values()
        .map(|run| (run.finished.clone(), run.settled.clone()))
        .collect::<Vec<_>>();
    for (finished, settled) in workers {
        let notified = settled.notified();
        if !finished.load(std::sync::atomic::Ordering::Acquire) {
            tokio::time::timeout(std::time::Duration::from_secs(5), notified)
                .await
                .expect("Released chat worker must join before fixture shutdown");
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let memory_owner = Arc::downgrade(&state.memory);
    let state_owner = Arc::downgrade(&state);
    crate::serve_runtime(state.clone(), listener, async {})
        .await
        .unwrap();
    drop(model_runtime);
    drop(faux);
    drop(gate);
    drop(state);
    assert_eq!(
        admitted,
        Ok(StatusCode::OK),
        "A visible terminal SSE frame must release the run lease before unrelated cleanup"
    );
    if let Err(error) = std::fs::remove_dir_all(&dir) {
        panic!(
            "Owned fixture cleanup failed: {error}; memory owners={}, runtime owners={}",
            memory_owner
                .upgrade()
                .map(|memory| Arc::strong_count(&memory) - 1)
                .unwrap_or(0),
            state_owner
                .upgrade()
                .map(|state| Arc::strong_count(&state) - 1)
                .unwrap_or(0),
        );
    }
}

#[tokio::test]
async fn interleaved_publish_preserves_earlier_live_frames_before_terminal() {
    let (dir, state) = isolated_state().await;
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, _) = tokio::sync::broadcast::channel(8);
    let publication = Arc::new(product::TerminalPublication::default());
    state.chat_runs.lock().unwrap().insert(
        id.clone(),
        product::ChatRun {
            frames: Arc::new(Mutex::new(Vec::new())),
            tx,
            closed: AtomicBool::new(false),
            cancel: Default::default(),
            finished: Arc::new(AtomicBool::new(true)),
            settled: Arc::new(Notify::new()),
            terminal_publication: publication.clone(),
        },
    );
    let live = run_events(
        State(state.clone()),
        Path(id.clone()),
        Query(Default::default()),
    )
    .await
    .unwrap();
    let (first_tx, first) = stage_record(
        &state,
        &id,
        "text_delta",
        json!({"delta":"earlier owned text"}),
    )
    .unwrap();
    let (terminal_tx, terminal) =
        stage_record(&state, &id, "done", json!({"text":"complete owned text"})).unwrap();
    // 两个真实生产者在 stage 后交错；晚帧先唤醒，早帧仍必须由 SSE 输出。
    publication.publish();
    terminal_tx.send(terminal).unwrap();
    first_tx.send(first).unwrap();
    let bytes = axum::body::to_bytes(live.into_body(), 1_000_000)
        .await
        .unwrap();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let events = raw
        .lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .collect::<Vec<_>>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    crate::serve_runtime(state.clone(), listener, async {})
        .await
        .unwrap();
    drop(state);
    assert_eq!(
        events,
        ["text_delta", "done"],
        "Out-of-order wakeups must not skip earlier ledger frames or finish SSE early"
    );
    assert!(raw.contains("earlier owned text"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn staged_terminal_replay_waits_for_publication() {
    let publication = Arc::new(product::TerminalPublication::default());
    let (tx, rx) = tokio::sync::broadcast::channel(8);
    let run = product::ChatRun {
        frames: Arc::new(Mutex::new(Vec::new())),
        tx,
        closed: AtomicBool::new(false),
        cancel: Default::default(),
        finished: Arc::new(AtomicBool::new(false)),
        settled: Arc::new(Notify::new()),
        terminal_publication: publication.clone(),
    };
    run.stage(1, "done", json!({"text":"persisted"}));
    let response = response(run.frames.clone(), rx, 0, true, Some(publication.clone()));
    let mut body = response.into_body().into_data_stream();
    assert!(
        body.next().now_or_never().is_none(),
        "Staged replay must not reveal a terminal before lease publication"
    );
    publication.publish();
    let bytes = body.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains("event: done"));
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn worker_unwind_releases_staged_publication_and_run_lease() {
    let publication = Arc::new(product::TerminalPublication::default());
    let finished = Arc::new(AtomicBool::new(false));
    let run = Arc::new(tokio::sync::Mutex::new(()));
    let settled = RunSettlement(
        finished.clone(),
        Arc::new(Notify::new()),
        Some(run.clone().lock_owned().await),
        publication.clone(),
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _settled = settled;
        panic!("synthetic finalizer unwind");
    }));
    assert!(result.is_err());
    assert!(run.try_lock().is_ok());
    assert!(publication.wait().now_or_never().is_some());
    assert!(finished.load(Ordering::Acquire));
}
