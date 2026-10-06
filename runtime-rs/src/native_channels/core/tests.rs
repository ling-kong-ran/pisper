use super::*;
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use tokio::sync::Notify;

struct FakeGateway {
    callbacks: GatewayCallbacks,
    status: Mutex<Value>,
    sent: Mutex<Vec<Value>>,
    assets: Mutex<Vec<Value>>,
    resources: Mutex<Vec<Resource>>,
    connections: AtomicUsize,
    disconnected: AtomicUsize,
    fail_send: AtomicBool,
}
impl Gateway for FakeGateway {
    fn get_status(&self) -> Value {
        self.status.lock().unwrap().clone()
    }
    fn connect(&self, connection: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            self.connections.fetch_add(1, AtomicOrdering::SeqCst);
            let status = json!({"state":"connected","lastError":"","connectedAt":"2026-01-01T00:00:00.000Z","bot":{"name":"Fixture Bot","id":"123456789012345"}});
            *self.status.lock().unwrap() = status.clone();
            self.sent
                .lock()
                .unwrap()
                .push(json!({"connect":connection}));
            Ok(status)
        })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.disconnected.fetch_add(1, AtomicOrdering::SeqCst);
            *self.status.lock().unwrap() = json!({"state":"idle","bot":null,"lastError":""});
            Ok(())
        })
    }
    fn send(&self, message: Value, payload: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap()
                .push(json!({"message":message,"payload":payload}));
            if self.fail_send.load(AtomicOrdering::SeqCst) {
                Err(ChannelError::new("synthetic send failure"))
            } else {
                Ok(())
            }
        })
    }
    fn send_to_peer(
        &self,
        peer_id: String,
        payload: Value,
        scope: Value,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap()
                .push(json!({"peerId":peer_id,"payload":payload,"scope":scope}));
            if self.fail_send.load(AtomicOrdering::SeqCst) {
                Err(ChannelError::new("synthetic send failure"))
            } else {
                Ok(())
            }
        })
    }
    fn send_asset(&self, peer_id: String, asset: Value, scope: Value) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.assets
                .lock()
                .unwrap()
                .push(json!({"peerId":peer_id,"asset":asset,"scope":scope}));
            Ok(())
        })
    }
    fn download_resources(&self, _: Value) -> BoxFuture<'_, Result<Vec<Resource>>> {
        Box::pin(async move { Ok(std::mem::take(&mut *self.resources.lock().unwrap())) })
    }
}
struct FakeOnboarding {
    completed: CompletedSink,
    started: Mutex<Vec<Value>>,
    disposed: AtomicBool,
}
impl Onboarding for FakeOnboarding {
    fn start(&self, options: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            self.started.lock().unwrap().push(options.clone());
            Ok(json!({"id":"fixture-job","options":options,"mode":"qr"}))
        })
    }
    fn get(&self, id: &str) -> Option<Value> {
        if id == "fixture-job" {
            Some(json!({"id":id,"status":"waiting"}))
        } else {
            None
        }
    }
    fn cancel(&self, id: &str) -> bool {
        id == "fixture-job"
    }
    fn verify(&self, id: &str, code: Value) -> Result<Option<Value>> {
        Ok(if id == "fixture-job" {
            Some(json!({"id":id,"code":code}))
        } else {
            None
        })
    }
    fn dispose(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.disposed.store(true, AtomicOrdering::SeqCst);
            Ok(())
        })
    }
}
struct Fixture {
    service: Arc<ChannelService>,
    state: Arc<Mutex<Value>>,
    gateways: Arc<Mutex<HashMap<String, Arc<FakeGateway>>>>,
    onboardings: Arc<Mutex<HashMap<String, Arc<FakeOnboarding>>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    prompts: Arc<Mutex<Vec<Value>>>,
    approval_release: Arc<Mutex<Option<Arc<Notify>>>>,
}
impl Fixture {
    fn new(stored: Value) -> Self {
        Self::with_prompt(stored, None)
    }
    fn with_prompt(
        stored: Value,
        prompt: Option<
            Arc<dyn Fn(PromptRequest) -> BoxFuture<'static, Result<PromptResult>> + Send + Sync>,
        >,
    ) -> Self {
        let persisted = Arc::new(Mutex::new(stored));
        let read = persisted.clone();
        let write = persisted.clone();
        let state = StatePort {
            read: Arc::new(move || Ok(read.lock().unwrap().clone())),
            update: Arc::new(move |mutation| {
                let mut stored = write.lock().unwrap();
                let mut latest = stored.clone();
                mutation(&mut latest)?;
                *stored = latest.clone();
                Ok(latest)
            }),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let records = prompts.clone();
        let counter = Arc::new(AtomicUsize::new(0));
        let default_prompt = Arc::new(move |request: PromptRequest| {
            records.lock().unwrap().push(json!({"sessionId":request.session_id,"message":request.message,"attachments":request.attachments,"cwd":request.cwd,"title":request.title,"model":request.model,"executionMode":request.execution_mode,"goalMode":request.goal_mode,"teamMode":request.team_mode}));
            let id = counter.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            Box::pin(async move {
                Ok(PromptResult {
                    session_id: format!("session-{id}"),
                    cwd: "/workspace".into(),
                    model: "fixture/chat".into(),
                    text: "**done**".into(),
                    assets: vec![json!({"id":"output","name":"image.png"})],
                })
            }) as BoxFuture<'static, Result<PromptResult>>
        });
        let record = calls.clone();
        let set_execution = Arc::new(move |id: String, mode: String| {
            record
                .lock()
                .unwrap()
                .push(json!({"setMode":id,"mode":mode}));
            Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<()>>
        });
        let record = calls.clone();
        let set_run = Arc::new(move |id: String, mode: String| {
            record
                .lock()
                .unwrap()
                .push(json!({"setRun":id,"mode":mode}));
            Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<()>>
        });
        let record = calls.clone();
        let set_cwd = Arc::new(move |id: String, cwd: String| {
            record.lock().unwrap().push(json!({"setCwd":id,"cwd":cwd}));
            Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<()>>
        });
        let approval_release = Arc::new(Mutex::new(None::<Arc<Notify>>));
        let unblock = approval_release.clone();
        let record = calls.clone();
        let resolve = Arc::new(move |id: String, approval: String, approved: bool| {
            record
                .lock()
                .unwrap()
                .push(json!({"resolve":id,"approval":approval,"approved":approved}));
            if let Some(release) = unblock.lock().unwrap().as_ref() {
                release.notify_one();
            }
            Box::pin(async { Ok(json!({"found":true})) }) as BoxFuture<'static, Result<Value>>
        });
        let record = calls.clone();
        let abort = Arc::new(move |id: String| {
            record.lock().unwrap().push(json!({"abort":id}));
            Box::pin(async { Ok(true) }) as BoxFuture<'static, Result<bool>>
        });
        let agent = AgentPort {
            prompt: prompt.unwrap_or(default_prompt),
            validate_directory: Arc::new(|directory| Box::pin(async move { Ok(directory) })),
            set_cwd,
            set_execution_mode: set_execution,
            set_run_mode: set_run,
            resolve_approval: resolve,
            abort,
        };
        let gateways = Arc::new(Mutex::new(HashMap::new()));
        let onboardings = Arc::new(Mutex::new(HashMap::new()));
        let mut gateway_factories = HashMap::new();
        let mut onboarding_factories = HashMap::new();
        for platform in state::PLATFORMS {
            let all = gateways.clone();
            let name = platform.to_owned();
            let factory: GatewayFactory = Arc::new(move |callbacks| {
                let gateway = Arc::new(FakeGateway {
                    callbacks,
                    status: Mutex::new(json!({"state":"idle","bot":null,"lastError":""})),
                    sent: Mutex::new(Vec::new()),
                    assets: Mutex::new(Vec::new()),
                    resources: Mutex::new(Vec::new()),
                    connections: AtomicUsize::new(0),
                    disconnected: AtomicUsize::new(0),
                    fail_send: AtomicBool::new(false),
                });
                all.lock().unwrap().insert(name.clone(), gateway.clone());
                gateway
            });
            gateway_factories.insert(platform.into(), factory);
            let all = onboardings.clone();
            let name = platform.to_owned();
            let factory: OnboardingFactory = Arc::new(move |completed| {
                let onboarding = Arc::new(FakeOnboarding {
                    completed,
                    started: Mutex::new(Vec::new()),
                    disposed: AtomicBool::new(false),
                });
                all.lock().unwrap().insert(name.clone(), onboarding.clone());
                onboarding
            });
            onboarding_factories.insert(platform.into(), factory);
        }
        let service = ChannelService::new(
            "/workspace".into(),
            agent,
            state,
            gateway_factories,
            onboarding_factories,
        );
        Self {
            service,
            state: persisted,
            gateways,
            onboardings,
            calls,
            prompts,
            approval_release,
        }
    }
    fn gateway(&self, platform: &str) -> Arc<FakeGateway> {
        self.gateways.lock().unwrap()[platform].clone()
    }
    fn message(peer: &str, sender: &str, content: &str) -> Value {
        json!({"peerId":peer,"senderId":sender,"senderName":"Fixture User","messageId":"fixture-message","chatType":"p2p","content":content,"resources":[],"contextToken":"fixture-context"})
    }
    async fn connect(&self, platform: &str) {
        self.service.complete_onboarding(platform,json!({"appId":"1234567890123","appSecret":"synthetic-feishu-key","token":"1234:synthetic-token","ownerOpenId":"owner","ownerUserId":"owner","accountId":"1234567890123"})).await.unwrap();
    }
}

#[tokio::test]
async fn migration_drops_personal_and_webhook_credentials_and_preserves_latest_unknown_state() {
    for stored in [
        json!({"version":1,"channels":[{"webhookUrl":"private-obsolete"}]}),
        json!({"version":4,"future":{"keep":true},"connections":{"qq":{"mode":"personal","token":"private-obsolete"},"telegram":{"mode":"personal","apiHash":"private-obsolete"}},"scopes":{}}),
    ] {
        let fixture = Fixture::new(stored);
        fixture.service.init().await.unwrap();
        let value = fixture.state.lock().unwrap().clone();
        assert_eq!(value["version"], 5);
        assert!(!value.to_string().contains("private-obsolete"));
        if value.get("future").is_some() {
            assert_eq!(value["future"]["keep"], true);
        }
        fixture.service.dispose().await.unwrap();
    }
    let fixture = Fixture::new(
        json!({"version":2,"connection":{"appId":"old","enabled":false},"scopes":{"peer":{"sessionId":"old-session","title":"Old"}}}),
    );
    fixture.service.init().await.unwrap();
    assert_eq!(
        fixture.service.get_state().unwrap()["scopes"][0]["key"],
        "feishu:peer"
    );
    assert_eq!(
        fixture.service.get_state().unwrap()["connections"]["feishu"]["accountId"],
        "old"
    );
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn saved_enabled_reconnect_public_masking_onboarding_and_atomic_weixin_sync() {
    let fixture = Fixture::new(
        json!({"version":5,"connections":{"feishu":{"enabled":true,"appId":"123456789012345","appSecret":"private-fixture"},"weixin":{"enabled":false,"token":"private-fixture"}},"scopes":{}}),
    );
    fixture.service.init().await.unwrap();
    fixture.service.wait_idle().await;
    assert_eq!(
        fixture
            .gateway("feishu")
            .connections
            .load(AtomicOrdering::SeqCst),
        1
    );
    assert_eq!(
        fixture
            .gateway("weixin")
            .connections
            .load(AtomicOrdering::SeqCst),
        0
    );
    let public = fixture.service.get_state().unwrap();
    assert_eq!(
        public["connections"]["feishu"]["accountId"],
        "1234567••••2345"
    );
    assert!(!public.to_string().contains("private-fixture"));
    let onboarding = fixture
        .service
        .start_onboarding("weixin", Value::Null)
        .await
        .unwrap();
    assert_eq!(
        onboarding["options"]["localTokens"],
        json!(["private-fixture"])
    );
    assert!(fixture
        .service
        .get_onboarding("weixin", "fixture-job")
        .is_some());
    assert!(fixture.service.cancel_onboarding("weixin", "fixture-job"));
    assert_eq!(
        fixture
            .service
            .verify_onboarding("weixin", "fixture-job", json!("123456"))
            .unwrap()
            .unwrap()["code"],
        "123456"
    );
    assert!(fixture
        .service
        .start_onboarding("qq", json!({"mode":"personal"}))
        .await
        .is_err());
    assert!(fixture
        .service
        .verify_onboarding("qq", "fixture-job", json!("x"))
        .is_err());
    let original = fixture.state.lock().unwrap().clone();
    fixture
        .service
        .update_weixin_sync(json!("new-sync"))
        .unwrap();
    assert_eq!(
        fixture.state.lock().unwrap()["connections"]["weixin"]["syncBuf"],
        "new-sync"
    );
    assert_eq!(
        fixture.state.lock().unwrap()["templates"],
        original["templates"]
    );
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn commands_share_session_modes_directory_approval_stop_and_reset() {
    let fixture = Fixture::new(json!({}));
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    fixture
        .service
        .handle_message("weixin", Fixture::message("peer", "owner", "first"))
        .await
        .unwrap();
    for command in [
        "/mode workspace",
        "/run team",
        "/dir /other",
        "/approve approval-1",
        "/deny approval-2",
        "/stop",
        "/status",
        "/mode",
        "/run",
        "/dir",
        "/mode invalid extra",
        "/run invalid",
    ] {
        fixture
            .service
            .handle_message("weixin", Fixture::message("peer", "owner", command))
            .await
            .unwrap();
    }
    let private = fixture.state.lock().unwrap().clone();
    assert_eq!(
        private["scopes"]["weixin:peer"]["executionMode"],
        "workspace-write"
    );
    assert_eq!(private["scopes"]["weixin:peer"]["runMode"], "team");
    assert_eq!(private["scopes"]["weixin:peer"]["cwd"], "/other");
    let calls = fixture.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|value| value["abort"] == "session-1"));
    assert!(calls.iter().any(|value| value["approved"] == false));
    fixture
        .service
        .handle_message("weixin", Fixture::message("peer", "owner", "/reset"))
        .await
        .unwrap();
    assert!(fixture.service.get_state().unwrap()["scopes"]
        .as_array()
        .unwrap()
        .is_empty());
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn owner_access_attachments_independent_peers_assets_and_default_modes_are_real() {
    let fixture = Fixture::new(json!({}));
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    fixture
        .service
        .handle_message("weixin", Fixture::message("a", "other", "denied"))
        .await
        .unwrap();
    assert!(fixture.prompts.lock().unwrap().is_empty());
    fixture.service.update("weixin",json!({"executionMode":"full-access","runMode":"goal","replyModel":{"provider":"fixture","model":"chosen"}})).await.unwrap();
    fixture.gateway("weixin").resources.lock().unwrap().extend([
        Resource {
            name: "image.JPG".into(),
            kind: "file".into(),
            mime_type: None,
            bytes: vec![1, 2],
        },
        Resource {
            name: "notes.txt".into(),
            kind: "file".into(),
            mime_type: None,
            bytes: vec![b'a', 255],
        },
        Resource {
            name: "report.pdf".into(),
            kind: "file".into(),
            mime_type: None,
            bytes: vec![3],
        },
        Resource {
            name: "ignored.bin".into(),
            kind: "file".into(),
            mime_type: None,
            bytes: vec![4],
        },
    ]);
    fixture
        .service
        .handle_message("weixin", Fixture::message("a", "owner", ""))
        .await
        .unwrap();
    fixture
        .service
        .handle_message("weixin", Fixture::message("b", "owner", "second"))
        .await
        .unwrap();
    let prompts = fixture.prompts.lock().unwrap().clone();
    assert_eq!(prompts[0]["message"], "请分析这些附件。");
    assert_eq!(prompts[0]["attachments"].as_array().unwrap().len(), 3);
    assert_eq!(prompts[0]["attachments"][0]["mimeType"], "image/jpeg");
    assert_eq!(prompts[0]["attachments"][1]["text"], "a�");
    assert_eq!(prompts[0]["goalMode"], true);
    assert_eq!(prompts[0]["model"]["model"], "chosen");
    assert_eq!(
        fixture.service.get_state().unwrap()["scopes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fixture.gateway("weixin").assets.lock().unwrap().len(), 2);
    assert_eq!(
        fixture
            .gateway("weixin")
            .connections
            .load(AtomicOrdering::SeqCst),
        1
    );
    fixture.service.remove("weixin").await.unwrap();
    assert!(fixture.service.get_state().unwrap()["connections"]["weixin"].is_null());
    assert!(fixture.service.get_state().unwrap()["scopes"]
        .as_array()
        .unwrap()
        .is_empty());
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn latest_scope_notification_templates_and_all_settled_failures_preserve_writer() {
    let fixture =
        Fixture::new(json!({"version":5,"future":{"keep":true},"connections":{},"scopes":{}}));
    fixture.service.init().await.unwrap();
    fixture.connect("feishu").await;
    fixture.connect("weixin").await;
    let event = "workflow.completed";
    fixture
        .service
        .update_template(
            event,
            "feishu",
            json!({"content":"Custom {{workflow.name}}"}),
        )
        .unwrap();
    fixture
        .service
        .update_template(
            event,
            "weixin",
            json!({"content":"Other {{workflow.name}}"}),
        )
        .unwrap();
    let before = fixture.state.lock().unwrap().clone();
    assert!(fixture
        .service
        .update_template(event, "weixin", json!({"content":" "}))
        .is_err());
    assert_eq!(*fixture.state.lock().unwrap(), before);
    (fixture.service.state.update)(Box::new(|value|{value["scopes"]=json!({"feishu:old":{"platform":"feishu","peerId":"old","updatedAt":"2026-01-01T00:00:00Z"},"feishu:new":{"platform":"feishu","peerId":"new","updatedAt":"2026-01-02T00:00:00Z"},"weixin:peer":{"platform":"weixin","peerId":"peer","contextToken":"private-context","updatedAt":"2026-01-02T00:00:00Z"}});Ok(())})).unwrap();
    fixture
        .gateway("weixin")
        .fail_send
        .store(true, AtomicOrdering::SeqCst);
    let result = fixture
        .service
        .notify(event, &json!({"workflow":{"name":"Build"}}), &json!({}))
        .await
        .unwrap();
    assert_eq!(result[0]["status"], "fulfilled");
    assert_eq!(result[1]["status"], "rejected");
    assert_eq!(
        fixture
            .gateway("feishu")
            .sent
            .lock()
            .unwrap()
            .last()
            .unwrap()["peerId"],
        "new"
    );
    assert_eq!(
        fixture
            .gateway("feishu")
            .sent
            .lock()
            .unwrap()
            .last()
            .unwrap()["payload"]["markdown"],
        "Custom Build"
    );
    assert_eq!(fixture.state.lock().unwrap()["future"]["keep"], true);
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn permission_session_is_saved_immediately_and_approval_bypasses_running_peer_queue() {
    let arrived = Arc::new(Notify::new());
    let approved = Arc::new(Notify::new());
    let request_arrived = arrived.clone();
    let release = approved.clone();
    let prompt = Arc::new(move |request: PromptRequest| {
        let arrived = request_arrived.clone();
        let release = release.clone();
        Box::pin(async move {
            (request.on_event)(
                "permission_request".into(),
                json!({"sessionId":"held-session","id":"pending-1","toolName":"bash","reason":"fixture"}),
            );
            arrived.notify_one();
            release.notified().await;
            (request.on_event)("permission_resolved".into(), json!({"id":"pending-1"}));
            Ok(PromptResult {
                session_id: "held-session".into(),
                cwd: "/workspace".into(),
                model: "fixture/chat".into(),
                text: "done".into(),
                assets: vec![],
            })
        }) as BoxFuture<'static, Result<PromptResult>>
    });
    let fixture = Fixture::with_prompt(json!({}), Some(prompt));
    *fixture.approval_release.lock().unwrap() = Some(approved.clone());
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "held request"));
    tokio::time::timeout(std::time::Duration::from_secs(2), arrived.notified())
        .await
        .unwrap();
    assert_eq!(
        fixture.state.lock().unwrap()["scopes"]["weixin:peer"]["sessionId"],
        "held-session"
    );
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "/approve"));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.service.wait_idle(),
    )
    .await
    .unwrap();
    assert_eq!(fixture.calls.lock().unwrap()[0]["approval"], "pending-1");
    assert!(fixture.state.lock().unwrap()["scopes"]["weixin:peer"]
        .get("pendingApprovalId")
        .is_none());
    fixture.service.dispose().await.unwrap();
}
#[tokio::test]
async fn dispose_cancels_active_agent_waits_finalizers_and_never_starts_queued_prompt() {
    let arrived = Arc::new(Notify::new());
    let observed = Arc::new(AtomicUsize::new(0));
    let started = observed.clone();
    let notified = arrived.clone();
    let settled = Arc::new(AtomicBool::new(false));
    let finalizer = settled.clone();
    let prompt = Arc::new(move |request: PromptRequest| {
        started.fetch_add(1, AtomicOrdering::SeqCst);
        let notified = notified.clone();
        let finalizer = finalizer.clone();
        Box::pin(async move {
            notified.notify_one();
            request.cancellation.cancelled().await;
            finalizer.store(true, AtomicOrdering::SeqCst);
            Err(ChannelError::new("cancelled"))
        }) as BoxFuture<'static, Result<PromptResult>>
    });
    let fixture = Fixture::with_prompt(json!({}), Some(prompt));
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "first"));
    tokio::time::timeout(std::time::Duration::from_secs(2), arrived.notified())
        .await
        .unwrap();
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "queued"));
    tokio::time::timeout(std::time::Duration::from_secs(2), fixture.service.dispose())
        .await
        .unwrap()
        .unwrap();
    assert!(settled.load(AtomicOrdering::SeqCst));
    assert_eq!(observed.load(AtomicOrdering::SeqCst), 1);
    assert!(fixture.service.connect("weixin").await.is_err());
    assert!(fixture
        .gateways
        .lock()
        .unwrap()
        .values()
        .all(|gateway| gateway.disconnected.load(AtomicOrdering::SeqCst) == 1));
}

fn sanitize(mut value: Value) -> Value {
    match &mut value {
        Value::Array(values) => {
            for value in values {
                *value = sanitize(value.clone());
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                *value = if ["createdAt", "updatedAt", "connectedAt"].contains(&key.as_str())
                    && state::truthy(value)
                {
                    json!("<time>")
                } else {
                    sanitize(value.clone())
                };
            }
        }
        _ => {}
    }
    value
}
pub(super) async fn compare_oracle_cases(rows: &Value) {
    for row in rows.as_array().unwrap() {
        let fixture = Fixture::new(row["stored"].clone());
        fixture.service.init().await.unwrap();
        fixture.service.wait_idle().await;
        let mut results = Vec::new();
        for action in row["actions"].as_array().unwrap() {
            let values = action.as_array().unwrap();
            let kind = values[0].as_str().unwrap();
            let argument = |index: usize| values.get(index).cloned().unwrap_or(Value::Null);
            let text = |index: usize| state::text(&argument(index));
            let result = match kind {
                "complete" => fixture
                    .service
                    .complete_onboarding(&text(1), argument(2))
                    .await
                    .unwrap(),
                "message" => {
                    fixture
                        .service
                        .handle_message(&text(1), Fixture::message(&text(2), &text(3), &text(4)))
                        .await
                        .unwrap();
                    Value::Null
                }
                "update" => fixture.service.update(&text(1), argument(2)).await.unwrap(),
                "template" => fixture
                    .service
                    .update_template(&text(1), &text(2), argument(3))
                    .unwrap(),
                "notify" => fixture
                    .service
                    .notify(&text(1), &argument(2), &argument(3))
                    .await
                    .unwrap(),
                "sync" => {
                    fixture.service.update_weixin_sync(argument(1)).unwrap();
                    Value::Null
                }
                "start" => fixture
                    .service
                    .start_onboarding(&text(1), Value::Null)
                    .await
                    .unwrap(),
                "verify" => fixture
                    .service
                    .verify_onboarding(&text(1), &text(2), argument(3))
                    .unwrap()
                    .unwrap_or(Value::Null),
                _ => panic!("unknown oracle action"),
            };
            results.push(sanitize(result));
        }
        assert_eq!(
            sanitize(fixture.service.get_state().unwrap()),
            row["expectedPublic"],
            "{} public",
            row["name"]
        );
        assert_eq!(
            sanitize(json!(*fixture.prompts.lock().unwrap())),
            row["expectedPrompts"],
            "{} prompt",
            row["name"]
        );
        assert_eq!(
            sanitize(json!(*fixture.calls.lock().unwrap())),
            row["expectedCalls"],
            "{} calls",
            row["name"]
        );
        assert_eq!(
            json!(results),
            row["results"],
            "{} action results",
            row["name"]
        );
        let mut sent = json!({});
        let mut assets = json!({});
        for platform in state::PLATFORMS {
            let gateway = fixture.gateway(platform);
            sent[platform] = sanitize(json!(gateway
                .sent
                .lock()
                .unwrap()
                .iter()
                .filter(|value| value.get("connect").is_none())
                .cloned()
                .collect::<Vec<_>>()));
            assets[platform] = sanitize(json!(*gateway.assets.lock().unwrap()));
        }
        assert_eq!(sent, row["expectedSent"], "{} sent", row["name"]);
        assert_eq!(assets, row["expectedAssets"], "{} assets", row["name"]);
        fixture.service.dispose().await.unwrap();
    }
}

#[tokio::test]
async fn canonical_channel_and_notification_updates_coexist_without_public_extension_leaks() {
    let fixture = Fixture::new(
        json!({"version":5,"futureRoot":{"keep":true},"connections":{},"scopes":{},
        "templates":{"workflow.completed":{"futureTemplate":{"private":"retain"},"enabled":false,"channels":{
            "browser":{"content":"old {{workflow.name}}","futureVariant":{"keep":true},"targets":["obsolete"]},
            "future-platform":{"content":"private","futureVariant":17,"targets":["obsolete"]}}},"future.event":{"keep":true}}}),
    );
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    let channel_writer = fixture.service.clone();
    let notification_writer = fixture.service.state.clone();
    let channel = async move {
        channel_writer
            .update("weixin", json!({"runMode":"team"}))
            .await
            .unwrap();
    };
    let notification = async move {
        (notification_writer.update)(Box::new(|value| {
            value["templates"]["workflow.completed"]["channels"]["browser"]["content"] =
                json!("new {{workflow.name}}");
            Ok(())
        }))
        .unwrap();
    };
    tokio::join!(channel, notification);
    let private = fixture.state.lock().unwrap().clone();
    assert_eq!(private["futureRoot"]["keep"], true);
    assert_eq!(
        private["templates"]["workflow.completed"]["futureTemplate"]["private"],
        "retain"
    );
    assert_eq!(
        private["templates"]["workflow.completed"]["channels"]["browser"]["futureVariant"]["keep"],
        true
    );
    assert!(
        private["templates"]["workflow.completed"]["channels"]["browser"]
            .get("targets")
            .is_none()
    );
    assert!(
        private["templates"]["workflow.completed"]["channels"]["future-platform"]
            .get("targets")
            .is_none()
    );
    assert_eq!(private["templates"]["future.event"]["keep"], true);
    assert_eq!(private["connections"]["weixin"]["runMode"], "team");
    assert_eq!(
        private["templates"]["workflow.completed"]["channels"]["browser"]["content"],
        "new {{workflow.name}}"
    );
    let public = fixture.service.get_state().unwrap().to_string();
    assert!(!public.contains("futureTemplate"));
    assert!(!public.contains("futureVariant"));
    assert!(!public.contains("futureRoot"));
    assert!(!public.contains("future-platform"));
    assert!(!public.contains("future.event"));
    fixture.service.dispose().await.unwrap();
}

#[tokio::test]
async fn peer_queue_keeps_fifo_and_uses_the_session_returned_by_previous_message() {
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let records = Arc::new(Mutex::new(Vec::new()));
    let calls = records.clone();
    let first = arrived.clone();
    let unblock = release.clone();
    let prompt = Arc::new(move |request: PromptRequest| {
        let first = first.clone();
        let unblock = unblock.clone();
        let calls = calls.clone();
        Box::pin(async move {
            calls
                .lock()
                .unwrap()
                .push(json!({"sessionId":request.session_id,"message":request.message}));
            if request.message == "first" {
                first.notify_one();
                unblock.notified().await;
            }
            Ok(PromptResult {
                session_id: "stable-session".into(),
                cwd: "/workspace".into(),
                model: "fixture/chat".into(),
                text: request.message,
                assets: vec![],
            })
        }) as BoxFuture<'static, Result<PromptResult>>
    });
    let fixture = Fixture::with_prompt(json!({}), Some(prompt));
    fixture.service.init().await.unwrap();
    fixture.connect("weixin").await;
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "first"));
    tokio::time::timeout(std::time::Duration::from_secs(2), arrived.notified())
        .await
        .unwrap();
    fixture
        .service
        .enqueue("weixin", Fixture::message("peer", "owner", "second"));
    release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.service.wait_idle(),
    )
    .await
    .unwrap();
    assert_eq!(
        *records.lock().unwrap(),
        vec![
            json!({"sessionId":"","message":"first"}),
            json!({"sessionId":"stable-session","message":"second"})
        ]
    );
    let weak = Arc::downgrade(&fixture.service);
    fixture.service.dispose().await.unwrap();
    drop(fixture);
    assert!(weak.upgrade().is_none());
}
