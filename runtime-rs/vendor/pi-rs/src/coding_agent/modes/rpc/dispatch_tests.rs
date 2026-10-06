//! Exercise dispatch against real services/SDK/AgentSessionRuntime. Provider IO
//! is offline faux; no command is answered by a mock runtime.
use super::*;
use crate::agent_core::{QueueMode, ThinkingLevel};
use crate::ai::auth::credential_store::InMemoryCredentialStore;
use crate::ai::models::{
    faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderHandle,
    FauxProviderOptions, FauxResponseStep,
};
use crate::coding_agent::agent_session::ExtensionBindings;
use crate::coding_agent::core::agent_session_runtime::{
    CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeOptions,
    CreateAgentSessionRuntimeResult,
};
use crate::coding_agent::core::agent_session_services::{
    create_agent_session_from_services, create_agent_session_services,
    CreateAgentSessionFromServicesOptions, CreateAgentSessionServicesOptions,
};
use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
use crate::coding_agent::core::resource_loader::{DefaultResourceLoaderOptions, InlineExtension};
use crate::coding_agent::core::sdk::NoTools;
use crate::coding_agent::core::settings_manager::SettingsManager;
use crate::coding_agent::extensions::loader::ExtensionFactory;
use crate::coding_agent::extensions::types::{
    self as ext, sync_handler, ExtensionMode, HandlerResult,
};
use crate::coding_agent::session_manager::SessionManager;
use std::sync::{atomic::AtomicUsize, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

type Trace = Arc<Mutex<Vec<Value>>>;
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("RPC regression timed out")
}
struct Fixture {
    _directory: tempfile::TempDir,
    runtime: Arc<AgentSessionRuntime>,
    dispatch: RpcDispatcher,
    responses: Arc<Mutex<Vec<RpcResponse>>>,
    emitted: Arc<Notify>,
    rebinds: Arc<AtomicUsize>,
}
impl Fixture {
    async fn new(faux: Option<&FauxProviderHandle>, factories: Vec<ExtensionFactory>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_str().unwrap().to_owned();
        let agent_dir = directory.path().join("agent").to_str().unwrap().to_owned();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let models = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(InMemoryCredentialStore::default())),
            models_path: Some(None),
            models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
            allow_model_network: false,
            refresh_on_create: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
        let model = faux.and_then(|faux| faux.get_model(None));
        let provider = faux.map(|faux| faux.provider.clone());
        let factory: CreateAgentSessionRuntimeFactory = Arc::new(move |options| {
            let model = model.clone();
            let provider = provider.clone();
            let models = models.clone();
            let mut factories = factories.clone();
            Box::pin(async move {
                if let Some(provider) = provider {
                    factories.insert(
                        0,
                        Arc::new(move |api| {
                            api.register_native_provider(&provider)?;
                            Ok(())
                        }),
                    );
                }
                let services = create_agent_session_services(CreateAgentSessionServicesOptions {
                    cwd: options.cwd,
                    agent_dir: Some(options.agent_dir),
                    model_runtime: Some(models),
                    settings_manager: Some(SettingsManager::in_memory(
                        serde_json::from_value(json!({})).unwrap(),
                    )),
                    resource_loader_options: Some(DefaultResourceLoaderOptions {
                        extension_factories: factories
                            .into_iter()
                            .map(InlineExtension::Factory)
                            .collect(),
                        no_skills: true,
                        no_prompt_templates: true,
                        no_themes: true,
                        no_context_files: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .await?;
                let mut create = CreateAgentSessionFromServicesOptions::new(
                    services.clone(),
                    options.session_manager,
                );
                create.model = model;
                create.no_tools = Some(NoTools::All);
                create.session_start_event = options
                    .session_start_event
                    .map(|event| serde_json::to_value(event).unwrap());
                let result = create_agent_session_from_services(create).await?;
                Ok(CreateAgentSessionRuntimeResult {
                    session: result.session,
                    extensions_result: result.extensions_result,
                    model_fallback_message: result.model_fallback_message,
                    diagnostics: services.diagnostics.clone(),
                    services: Arc::new(services),
                })
            })
        });
        let first = bounded(factory(CreateAgentSessionRuntimeOptions {
            cwd: cwd.clone(),
            agent_dir,
            session_manager: Arc::new(Mutex::new(
                SessionManager::in_memory(&cwd, None, None).unwrap(),
            )),
            session_start_event: None,
            project_trust_context: None,
        }))
        .await
        .unwrap();
        let runtime = Arc::new(AgentSessionRuntime::new(
            first.session,
            first.services,
            factory,
            first.diagnostics,
            first.model_fallback_message,
        ));
        let rebinds = Arc::new(AtomicUsize::new(0));
        let weak = Arc::downgrade(&runtime);
        let count = rebinds.clone();
        let rebind: RpcRebind = Arc::new(move || {
            let runtime = weak.upgrade().unwrap();
            let count = count.clone();
            Box::pin(async move {
                runtime
                    .session()
                    .bind_extensions(ExtensionBindings {
                        mode: Some(ExtensionMode::Rpc),
                        ..Default::default()
                    })
                    .await?;
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        rebind().await.unwrap();
        let callback = rebind.clone();
        runtime.set_rebind_session(Some(Arc::new(move |_| callback())));
        let responses = Arc::new(Mutex::new(vec![]));
        let emitted = Arc::new(Notify::new());
        let buffer = responses.clone();
        let notify = emitted.clone();
        let dispatch = RpcDispatcher::new(
            runtime.clone(),
            Arc::new(move |response| {
                buffer.lock().unwrap().push(response);
                notify.notify_one();
            }),
        )
        .with_rebind(rebind);
        Self {
            _directory: directory,
            runtime,
            dispatch,
            responses,
            emitted,
            rebinds,
        }
    }
    async fn call(&self, kind: RpcCommandKind) -> RpcResponse {
        bounded(self.dispatch.handle(RpcCommand {
            id: Some("request".into()),
            kind,
        }))
        .await
        .expect("non-prompt response")
    }
    async fn wire(&self, command: Value) -> Value {
        let response = bounded(
            self.dispatch
                .handle(serde_json::from_value(command).unwrap()),
        )
        .await
        .unwrap();
        serde_json::to_value(response).unwrap()
    }
    async fn prompt(&self, message: &str) {
        assert!(bounded(self.dispatch.handle(RpcCommand {
            id: Some("prompt-id".into()),
            kind: RpcCommandKind::Prompt {
                message: message.into(),
                images: None,
                streaming_behavior: None
            }
        }))
        .await
        .is_none());
    }
    async fn response(&self) -> RpcResponse {
        loop {
            let notified = self.emitted.notified();
            if let Some(response) = self.responses.lock().unwrap().pop() {
                return response;
            }
            bounded(notified).await;
        }
    }
    async fn close(&self) {
        bounded(self.runtime.dispose()).await.unwrap();
    }
}
fn offline_faux() -> FauxProviderHandle {
    faux_provider(FauxProviderOptions {
        provider: Some("rpc-native-faux".into()),
        ..Default::default()
    })
}

#[tokio::test]
async fn empty_session_shapes_and_response_correlation() {
    let f = Fixture::new(None, vec![]).await;
    let state = f.call(RpcCommandKind::GetState).await;
    assert!(state.success);
    let data = state.data.unwrap();
    assert_eq!(data["sessionId"], f.runtime.session().session_id());
    for absent in ["sessionFile", "model", "sessionName"] {
        assert!(data.get(absent).is_none(), "{absent}");
    }
    assert_eq!(data["isStreaming"], false);
    assert_eq!(data["messageCount"], 0);
    assert_eq!(data["pendingMessageCount"], 0);
    for (kind, expected) in [
        (RpcCommandKind::GetMessages, json!({"messages":[]})),
        (RpcCommandKind::GetLastAssistantText, json!({})),
        (RpcCommandKind::GetAvailableModels, json!({"models":[]})),
        (RpcCommandKind::GetForkMessages, json!({"messages":[]})),
        (RpcCommandKind::CycleModel, Value::Null),
        (RpcCommandKind::CycleThinkingLevel, Value::Null),
    ] {
        let name = kind.as_str();
        let response = f.call(kind).await;
        assert_eq!(
            response,
            RpcResponse::success(Some("request".into()), name, Some(expected))
        );
    }
    assert_eq!(
        f.wire(json!({"type":"abort_bash"})).await,
        json!({"type":"response","command":"abort_bash","success":true})
    );
    let stats = f.call(RpcCommandKind::GetSessionStats).await.data.unwrap();
    for absent in ["sessionFile", "contextUsage"] {
        assert!(stats.get(absent).is_none());
    }
    assert_eq!(stats["totalMessages"], 0);
    f.close().await;
}

#[tokio::test]
async fn queue_commands_preserve_rpc_source_image_tag_and_order() {
    let seen: Trace = Trace::default();
    let trace = seen.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let trace = trace.clone();
        api.on(
            "input",
            sync_handler(move |event, _| {
                trace.lock().unwrap().push(event.clone());
                Ok(None)
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    for (command, text) in [("steer", "s1"), ("follow_up", "f1"), ("steer", "s2")] {
        let response = f.wire(json!({"id":command,"type":command,"message":text,"images":[{"type":"image","mimeType":"image/png","data":"abc"}]})).await;
        assert_eq!(response["success"], true);
    }
    let state = f.call(RpcCommandKind::GetState).await.data.unwrap();
    assert_eq!(state["pendingMessageCount"], 3);
    assert_eq!(
        f.call(RpcCommandKind::ClearQueue).await.data,
        Some(json!({"steering":["s1","s2"],"followUp":["f1"]}))
    );
    assert_eq!(
        f.call(RpcCommandKind::ClearQueue).await.data,
        Some(json!({"steering":[],"followUp":[]}))
    );
    let trace = seen.lock().unwrap().clone();
    assert_eq!(trace.len(), 3);
    for event in trace {
        assert_eq!(event["source"], "rpc");
        assert_eq!(event["images"][0]["type"], "image");
    }
    f.close().await;
}

#[tokio::test]
async fn queueing_extension_commands_returns_error_instead_of_panicking() {
    let extension: ExtensionFactory = Arc::new(|api| {
        api.register_command("hello", None, Arc::new(|_, _| Ok(None)))?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    for kind in [
        RpcCommandKind::Steer {
            message: "/hello args".into(),
            images: None,
        },
        RpcCommandKind::FollowUp {
            message: "/hello".into(),
            images: None,
        },
    ] {
        let response = f.call(kind).await;
        assert!(!response.success);
        assert_eq!(response.error.as_deref(),Some("Extension command \"/hello\" cannot be queued. Use prompt() or execute the command when not streaming."));
    }
    assert_eq!(f.runtime.session().pending_message_count(), 0);
    f.close().await;
}

#[tokio::test]
async fn settings_mutations_and_model_lookup_use_live_session() {
    let faux = offline_faux();
    let f = Fixture::new(Some(&faux), vec![]).await;
    let model = faux.get_model(None).unwrap();
    let response = f
        .call(RpcCommandKind::SetModel {
            provider: "missing".into(),
            model_id: "x".into(),
        })
        .await;
    assert_eq!(
        response.error.as_deref(),
        Some("Model not found: missing/x")
    );
    let response = f
        .call(RpcCommandKind::SetModel {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
        })
        .await;
    assert!(response.success, "{response:?}");
    assert_eq!(response.data, Some(serde_json::to_value(&model).unwrap()));
    for kind in [
        RpcCommandKind::SetSteeringMode {
            mode: QueueMode::All,
        },
        RpcCommandKind::SetFollowUpMode {
            mode: QueueMode::All,
        },
        RpcCommandKind::SetThinkingLevel {
            level: ThinkingLevel::High,
        },
        RpcCommandKind::SetAutoCompaction { enabled: false },
        RpcCommandKind::SetAutoRetry { enabled: false },
        RpcCommandKind::AbortRetry,
        RpcCommandKind::Abort,
    ] {
        let response = f.call(kind).await;
        assert!(response.success, "{response:?}");
        assert_eq!(response.data, None);
    }
    let state = f.call(RpcCommandKind::GetState).await.data.unwrap();
    assert_eq!(state["steeringMode"], "all");
    assert_eq!(state["followUpMode"], "all");
    assert_eq!(state["autoCompactionEnabled"], false);
    assert!(!f.runtime.session().auto_retry_enabled());
    let levels = f
        .call(RpcCommandKind::GetAvailableThinkingLevels)
        .await
        .data
        .unwrap();
    assert_eq!(
        levels["levels"],
        serde_json::to_value(f.runtime.session().get_available_thinking_levels()).unwrap()
    );
    f.close().await;
}

#[tokio::test]
async fn session_name_uses_js_trim_not_unicode_white_space() {
    let f = Fixture::new(None, vec![]).await;
    for input in ["", " \t\r\n", "\u{feff}\u{a0}\u{2028}"] {
        let response = f
            .call(RpcCommandKind::SetSessionName { name: input.into() })
            .await;
        assert_eq!(
            response.error.as_deref(),
            Some("Session name cannot be empty")
        );
    }
    for (input, expected) in [
        ("\u{feff} title \u{feff}", "title"),
        ("\u{0085}", "\u{0085}"),
        ("\u{1c} title \u{1c}", "\u{1c} title \u{1c}"),
        (" a\r\n\nb ", "a b"),
    ] {
        assert!(
            f.call(RpcCommandKind::SetSessionName { name: input.into() })
                .await
                .success
        );
        assert_eq!(
            f.runtime.session().session_name().as_deref(),
            Some(expected)
        );
    }
    f.close().await;
}

#[tokio::test]
async fn prompt_ack_waits_for_preflight_without_blocking_other_commands() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let started = entered.clone();
    let gate = release.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let entered = started.clone();
        let release = gate.clone();
        api.on(
            "input",
            Arc::new(move |_, _| {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.acquire().await.unwrap().forget();
                    Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
                })
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    f.prompt("held input").await;
    bounded(entered.notified()).await;
    assert!(f.responses.lock().unwrap().is_empty());
    assert!(f.call(RpcCommandKind::GetState).await.success);
    release.add_permits(1);
    // Upstream rpc-mode.ts answers the prompt ACK with the preflight
    // disposition (`success(id, "prompt", { disposition })`); the extension
    // input handler reported handled.
    assert_eq!(
        f.response().await,
        RpcResponse::success(
            Some("prompt-id".into()),
            "prompt",
            Some(json!({"disposition": "handled"}))
        )
    );
    assert!(f.responses.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn prompt_preflight_failure_emits_one_correlated_error() {
    let f = Fixture::new(None, vec![]).await;
    f.prompt("no model").await;
    let response = f.response().await;
    assert_eq!(response.id.as_deref(), Some("prompt-id"));
    assert_eq!(response.command, "prompt");
    assert!(!response.success);
    assert_eq!(
        response.error.as_deref(),
        Some(crate::coding_agent::core::auth_guidance::format_no_model_selected_message().as_str())
    );
    assert!(f.responses.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
async fn prompt_ack_precedes_provider_completion_and_does_not_duplicate_late_error() {
    for fail in [false, true] {
        let faux = offline_faux();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let entered = started.clone();
        let gate = release.clone();
        faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(move |_| {
            let entered = entered.clone();
            let gate = gate.clone();
            Box::pin(async move {
                entered.notify_one();
                gate.acquire().await.unwrap().forget();
                if fail {
                    Err("late provider failure".into())
                } else {
                    Ok(faux_assistant_message(
                        "completed",
                        FauxMessageOptions::default(),
                    ))
                }
            })
        }))]);
        let f = Fixture::new(Some(&faux), vec![]).await;
        f.runtime.session().set_auto_retry_enabled(false);
        f.prompt("run").await;
        // Upstream rpc-mode.ts answers the prompt ACK with the preflight
        // disposition; no extension input handler ran, so the prompt started.
        assert_eq!(
            f.response().await,
            RpcResponse::success(
                Some("prompt-id".into()),
                "prompt",
                Some(json!({"disposition": "started"}))
            )
        );
        bounded(started.notified()).await;
        assert_eq!(
            f.call(RpcCommandKind::GetState).await.data.unwrap()["isStreaming"],
            true
        );
        gate_release(&release);
        bounded(f.runtime.session().wait_for_idle()).await;
        assert!(
            f.responses.lock().unwrap().is_empty(),
            "must not emit second prompt error after acceptance"
        );
        assert_eq!(faux.state().lock().unwrap().call_count, 1);
        if !fail {
            assert_eq!(
                f.call(RpcCommandKind::GetLastAssistantText).await.data,
                Some(json!({"text":"completed"}))
            );
        }
        f.close().await;
    }
}
fn gate_release(gate: &Semaphore) {
    gate.add_permits(1);
}

#[tokio::test]
async fn bash_override_is_recorded_without_running_a_real_shell() {
    let seen: Trace = Trace::default();
    let trace = seen.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let trace = trace.clone();
        api.on("user_bash",sync_handler(move |event,_| {
            trace.lock().unwrap().push(event.clone());
            Ok(Some(HandlerResult::Json(json!({"result":{"output":"extension handled","exitCode":7,"cancelled":false,"truncated":false}}))))
        }))?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    let response = f
        .call(RpcCommandKind::Bash {
            command: "not-a-real-executable".into(),
            exclude_from_context: Some(true),
        })
        .await;
    assert!(response.success, "{response:?}");
    assert_eq!(response.data.unwrap()["exitCode"], 7);
    let messages = f.call(RpcCommandKind::GetMessages).await.data.unwrap();
    assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
    assert_eq!(messages["messages"][0]["output"], "extension handled");
    assert_eq!(messages["messages"][0]["excludeFromContext"], true);
    let trace = seen.lock().unwrap().clone();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0]["cwd"], f.runtime.cwd());
    assert_eq!(trace[0]["excludeFromContext"], true);
    f.close().await;
}

#[tokio::test]
async fn extension_bash_operations_stream_and_cancel_through_real_executor() {
    let begun = Arc::new(Notify::new());
    let started = begun.clone();
    let trace: Trace = Trace::default();
    let calls = trace.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let started = started.clone();
        let calls = calls.clone();
        api.on(
            "user_bash",
            sync_handler(move |_, _| {
                let started = started.clone();
                let calls = calls.clone();
                Ok(Some(HandlerResult::UserBashOperations(
                    ext::BashOperations {
                        exec: Arc::new(move |command, cwd, callbacks| {
                            let started = started.clone();
                            let calls = calls.clone();
                            Box::pin(async move {
                                calls
                                    .lock()
                                    .unwrap()
                                    .push(json!({"command":command,"cwd":cwd}));
                                (callbacks.on_data)("one\n");
                                (callbacks.on_data)("two\n");
                                started.notify_one();
                                callbacks.signal.cancelled().await;
                                Err("aborted".into())
                            })
                        }),
                    },
                )))
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    let events: Trace = Trace::default();
    let seen = events.clone();
    let subscription = f.runtime.session().subscribe(Arc::new(move |event| {
        seen.lock()
            .unwrap()
            .push(serde_json::to_value(event).unwrap());
    }));
    let dispatcher = f.dispatch.clone();
    let pending = tokio::spawn(async move {
        dispatcher
            .handle(RpcCommand {
                id: Some("bash-id".into()),
                kind: RpcCommandKind::Bash {
                    command: "remote only".into(),
                    exclude_from_context: None,
                },
            })
            .await
            .unwrap()
    });
    bounded(begun.notified()).await;
    assert!(f.runtime.session().is_bash_running());
    assert!(f.call(RpcCommandKind::AbortBash).await.success);
    let result = bounded(pending).await.unwrap();
    assert!(result.success, "{result:?}");
    let data = result.data.unwrap();
    assert_eq!(data["cancelled"], true);
    assert_eq!(data["output"], "one\ntwo\n");
    assert!(!f.runtime.session().is_bash_running());
    assert_eq!(
        *trace.lock().unwrap(),
        vec![json!({"command":"remote only","cwd":f.runtime.cwd()})]
    );
    let events = events.lock().unwrap().clone();
    let chunks = events
        .iter()
        .filter(|event| event["type"] == "bash_execution_update")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        chunks,
        vec![
            json!({"type":"bash_execution_update","id":"bash-id","delta":"one\n"}),
            json!({"type":"bash_execution_update","id":"bash-id","delta":"two\n"})
        ]
    );
    subscription.unsubscribe();
    f.close().await;
}

#[tokio::test]
async fn new_session_rebinds_and_following_commands_use_replacement() {
    let f = Fixture::new(None, vec![]).await;
    let old = f.runtime.session();
    let old_id = old.session_id();
    assert!(
        f.call(RpcCommandKind::SetSessionName { name: "old".into() })
            .await
            .success
    );
    let result = f
        .call(RpcCommandKind::NewSession {
            parent_session: Some("parent.jsonl".into()),
        })
        .await;
    assert_eq!(result.data, Some(json!({"cancelled":false})));
    let current = f.runtime.session();
    assert!(!Arc::ptr_eq(&old, &current));
    assert_ne!(old_id, current.session_id());
    assert_eq!(
        f.rebinds.load(Ordering::SeqCst),
        3,
        "initial bind, runtime replacement, explicit mode rebind"
    );
    assert_eq!(
        current
            .session_manager
            .lock()
            .unwrap()
            .get_header()
            .unwrap()
            .parent_session
            .as_deref(),
        Some("parent.jsonl")
    );
    assert!(
        f.call(RpcCommandKind::SetSessionName { name: "new".into() })
            .await
            .success
    );
    assert_eq!(current.session_name().as_deref(), Some("new"));
    assert_eq!(old.session_name().as_deref(), Some("old"));
    f.close().await;
}

#[tokio::test]
async fn cancelled_session_transitions_keep_session_and_omit_fork_text() {
    let extension: ExtensionFactory = Arc::new(|api| {
        for event in ["session_before_switch", "session_before_fork"] {
            api.on(
                event,
                sync_handler(|_, _| Ok(Some(HandlerResult::Json(json!({"cancel":true}))))),
            )?;
        }
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    let session = f.runtime.session();
    for kind in [
        RpcCommandKind::NewSession {
            parent_session: None,
        },
        RpcCommandKind::SwitchSession {
            session_path: "nonexistent-but-vetoed.jsonl".into(),
        },
        RpcCommandKind::Fork {
            entry_id: "nonexistent-but-vetoed".into(),
        },
    ] {
        let result = f.call(kind).await;
        assert!(result.success, "{result:?}");
        assert_eq!(result.data, Some(json!({"cancelled":true})));
    }
    assert!(Arc::ptr_eq(&session, &f.runtime.session()));
    assert_eq!(f.rebinds.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn persisted_turns_entries_tree_stats_clone_and_fork() {
    let faux = offline_faux();
    faux.set_responses(vec![faux_assistant_message(
        "answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let f = Fixture::new(Some(&faux), vec![]).await;
    f.prompt("question").await;
    assert!(f.response().await.success);
    bounded(f.runtime.session().wait_for_idle()).await;
    let messages = f.call(RpcCommandKind::GetForkMessages).await.data.unwrap();
    assert_eq!(messages["messages"].as_array().unwrap().len(), 1);
    assert_eq!(messages["messages"][0]["text"], "question");
    let entry_id = messages["messages"][0]["entryId"]
        .as_str()
        .unwrap()
        .to_owned();
    let after = f
        .call(RpcCommandKind::GetEntries {
            since: Some(entry_id.clone()),
        })
        .await
        .data
        .unwrap();
    assert!(after["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["message"]["role"] == "assistant"));
    let full = f
        .call(RpcCommandKind::GetEntries { since: None })
        .await
        .data
        .unwrap();
    let leaf = full["leafId"].as_str().unwrap().to_owned();
    assert_eq!(
        f.call(RpcCommandKind::GetEntries {
            since: Some(leaf.clone())
        })
        .await
        .data,
        Some(json!({"entries":[],"leafId":leaf}))
    );
    let tree = f.call(RpcCommandKind::GetTree).await.data.unwrap();
    assert!(!tree["tree"].as_array().unwrap().is_empty());
    assert_eq!(tree["leafId"], full["leafId"]);
    let stats = f.call(RpcCommandKind::GetSessionStats).await.data.unwrap();
    assert_eq!(stats["userMessages"], 1);
    assert_eq!(stats["assistantMessages"], 1);
    // The live runtime also persists the effective system prompt. Upstream
    // counts every message entry, not just user and assistant roles.
    let message_roles: Vec<_> = full["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "message")
        .map(|entry| entry["message"]["role"].clone())
        .collect();
    assert_eq!(
        message_roles,
        vec![json!("system"), json!("user"), json!("assistant")]
    );
    assert_eq!(stats["totalMessages"], 3);
    let old = f.runtime.session();
    assert!(f.call(RpcCommandKind::Clone).await.success);
    assert!(!Arc::ptr_eq(&old, &f.runtime.session()));
    assert_eq!(
        f.call(RpcCommandKind::GetLastAssistantText).await.data,
        Some(json!({"text":"answer"}))
    );
    let result = f.call(RpcCommandKind::Fork { entry_id }).await;
    assert!(result.success, "{result:?}");
    assert_eq!(
        result.data,
        Some(json!({"text":"question","cancelled":false}))
    );
    // Fork-before-user keeps the earlier system message, but neither the
    // selected user message nor its assistant response.
    let system = full["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "message" && entry["message"]["role"] == "system")
        .unwrap()["message"]
        .clone();
    assert_eq!(
        f.call(RpcCommandKind::GetMessages).await.data,
        Some(json!({"messages":[system]}))
    );
    f.close().await;
}

#[tokio::test]
async fn missing_entries_and_unavailable_session_actions_are_errors_not_success_stubs() {
    let f = Fixture::new(None, vec![]).await;
    let result = f
        .call(RpcCommandKind::GetEntries {
            since: Some("missing".into()),
        })
        .await;
    assert_eq!(result.error.as_deref(), Some("Entry not found: missing"));
    // Settings may create a thinking-level entry, so select the genuinely
    // empty leaf before checking the clone error.
    f.runtime
        .session()
        .session_manager
        .lock()
        .unwrap()
        .reset_leaf();
    assert_eq!(
        f.call(RpcCommandKind::Clone).await.error.as_deref(),
        Some("Cannot clone session: no current entry selected")
    );
    // Upstream creates a fresh session for a nonexistent explicit path.
    // A nonempty invalid file, rather than a missing file, is the error case.
    let invalid_path = f._directory.path().join("invalid.jsonl");
    std::fs::write(&invalid_path, b"not a session\n").unwrap();
    for kind in [
        RpcCommandKind::SwitchSession {
            session_path: invalid_path.to_str().unwrap().into(),
        },
        RpcCommandKind::Fork {
            entry_id: "missing".into(),
        },
        RpcCommandKind::Compact {
            custom_instructions: Some("extra".into()),
        },
        RpcCommandKind::ExportHtml { output_path: None },
    ] {
        let response = f.call(kind).await;
        assert!(!response.success, "{response:?}");
        assert!(response.data.is_none());
        assert!(response.error.is_some());
    }
    f.close().await;
}

#[tokio::test]
async fn commands_preserve_extension_registration_order_and_camel_case_metadata() {
    let extension: ExtensionFactory = Arc::new(|api| {
        api.register_command(
            "zeta",
            Some("last lexically".into()),
            Arc::new(|_, _| Ok(None)),
        )?;
        api.register_command("alpha", None, Arc::new(|_, _| Ok(None)))?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    let data = f.call(RpcCommandKind::GetCommands).await.data.unwrap();
    let commands = data["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0]["name"], "zeta");
    assert_eq!(commands[1]["name"], "alpha");
    assert!(commands[1].get("description").is_none());
    for command in commands {
        assert_eq!(command["source"], "extension");
        assert!(command["sourceInfo"].get("base_dir").is_none());
    }
    f.close().await;
}

#[tokio::test]
async fn deterministic_wire_branches_match_actual_upstream_handler_oracle() {
    let oracle: Value = serde_json::from_str(include_str!("dispatch_oracle.json")).unwrap();
    assert_eq!(oracle["cases"].as_array().unwrap().len(), 23);
    for case in oracle["cases"].as_array().unwrap() {
        let f = Fixture::new(None, vec![]).await;
        let command: RpcCommand = serde_json::from_value(case["command"].clone()).unwrap();
        let response = bounded(f.dispatch.handle(command)).await.unwrap();
        assert_eq!(
            crate::coding_agent::modes::rpc::jsonl::serialize_json_line(&response).unwrap(),
            case["wire"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        if let Some(expected) = case.get("sessionName") {
            assert_eq!(
                serde_json::to_value(f.runtime.session().session_name()).unwrap(),
                *expected,
                "{}",
                case["name"]
            );
        }
        f.close().await;
    }
}

#[tokio::test]
async fn missing_explicit_session_path_creates_new_session_like_upstream() {
    let f = Fixture::new(None, vec![]).await;
    let old = f.runtime.session();
    let path = f._directory.path().join("new-explicit.jsonl");
    let response = f
        .call(RpcCommandKind::SwitchSession {
            session_path: path.to_str().unwrap().into(),
        })
        .await;
    assert_eq!(response.data, Some(json!({"cancelled":false})));
    assert!(!Arc::ptr_eq(&old, &f.runtime.session()));
    assert_eq!(
        std::path::Path::new(&f.runtime.session().session_file().unwrap()),
        path
    );
    f.close().await;
}

#[tokio::test]
async fn real_extension_ui_preflight_can_wait_for_client_while_rpc_state_remains_responsive() {
    use crate::coding_agent::extensions::types::ExtensionUiDialogOptions;
    use crate::coding_agent::modes::rpc::ui::RpcExtensionUi;
    let answers: Trace = Trace::default();
    let record = answers.clone();
    let extension: ExtensionFactory = Arc::new(move |api| {
        let record = record.clone();
        api.on(
            "input",
            Arc::new(move |_, ctx| {
                let record = record.clone();
                Box::pin(async move {
                    let ui = ctx.ui()?;
                    let answer = ui
                        .input(
                            "RPC extension input",
                            Some("reply"),
                            &ExtensionUiDialogOptions::default(),
                        )
                        .await?;
                    record.lock().unwrap().push(json!(answer));
                    Ok(Some(HandlerResult::Json(json!({"action":"handled"}))))
                })
            }),
        )?;
        Ok(())
    });
    let f = Fixture::new(None, vec![extension]).await;
    let requests: Trace = Trace::default();
    let sink = requests.clone();
    let emitted = Arc::new(Notify::new());
    let notify = emitted.clone();
    let ui = Arc::new(RpcExtensionUi::new(Arc::new(move |request| {
        sink.lock().unwrap().push(request);
        notify.notify_one();
    })));
    f.runtime
        .session()
        .bind_extensions(ExtensionBindings {
            ui_context: Some(ui.clone()),
            mode: Some(ExtensionMode::Rpc),
            ..Default::default()
        })
        .await
        .unwrap();
    f.prompt("ask the extension").await;
    bounded(emitted.notified()).await;
    assert!(f.responses.lock().unwrap().is_empty());
    assert!(f.call(RpcCommandKind::GetState).await.success);
    let request = requests.lock().unwrap()[0].clone();
    assert_eq!(request["method"], "input");
    assert_eq!(request["placeholder"], "reply");
    assert!(ui.respond(
        json!({"type":"extension_ui_response","id":request["id"],"value":"client answer"})
    ));
    // Upstream rpc-mode.ts answers the prompt ACK with the preflight
    // disposition; the extension input handler reported handled.
    assert_eq!(
        f.response().await,
        RpcResponse::success(
            Some("prompt-id".into()),
            "prompt",
            Some(json!({"disposition": "handled"}))
        )
    );
    assert_eq!(*answers.lock().unwrap(), vec![json!("client answer")]);
    assert_eq!(ui.pending_count(), 0);
    f.close().await;
}
