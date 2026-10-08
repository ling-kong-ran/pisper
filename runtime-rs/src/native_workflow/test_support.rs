use super::{Result, WorkflowError};
use crate::workflow_engine::{
    AgentRequest, AgentResult, ImageNodeRequest, ImageNodeResult, MediaInputs, WorkflowExecutor,
    WorkflowService,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) struct TempDirectory {
    pub(crate) path: PathBuf,
}
impl TempDirectory {
    pub(crate) fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pisper-native-workflow-{}", super::id().unwrap()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}
impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
pub(crate) type PromptFixture =
    Arc<dyn Fn(AgentRequest) -> BoxFuture<'static, Result<AgentResult>> + Send + Sync>;
pub(crate) type ImageFixture =
    Arc<dyn Fn(ImageNodeRequest) -> BoxFuture<'static, Result<ImageNodeResult>> + Send + Sync>;

/// A real owned Future whose cleanup deliberately remains pending after its
/// cancellation token fires. Dropping it cannot masquerade as completed finally.
pub(crate) struct PromptLifecycle {
    pub(crate) started: tokio::sync::Notify,
    pub(crate) cancelling: tokio::sync::Notify,
    pub(crate) finish: tokio::sync::Semaphore,
    pub(crate) finally_finished: std::sync::atomic::AtomicBool,
    pub(crate) dropped: std::sync::atomic::AtomicBool,
    pub(crate) dropped_before_finally: std::sync::atomic::AtomicBool,
}
impl PromptLifecycle {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            started: tokio::sync::Notify::new(),
            cancelling: tokio::sync::Notify::new(),
            finish: tokio::sync::Semaphore::new(0),
            finally_finished: std::sync::atomic::AtomicBool::new(false),
            dropped: std::sync::atomic::AtomicBool::new(false),
            dropped_before_finally: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub(crate) fn fixture(self: &Arc<Self>) -> PromptFixture {
        let lifecycle = self.clone();
        Arc::new(move |request| {
            let lifecycle = lifecycle.clone();
            Box::pin(async move {
                let _owner = PromptLifecycleGuard(lifecycle.clone());
                (request.on_session)("cooperative-session".into()).await;
                lifecycle.started.notify_one();
                request.cancellation.cancelled().await;
                lifecycle.cancelling.notify_one();
                let _finish = lifecycle.finish.acquire().await.unwrap();
                lifecycle
                    .finally_finished
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Err(WorkflowError::cancelled())
            })
        })
    }
}
struct PromptLifecycleGuard(Arc<PromptLifecycle>);
impl Drop for PromptLifecycleGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.0.dropped.store(true, SeqCst);
        if !self.0.finally_finished.load(SeqCst) {
            self.0.dropped_before_finally.store(true, SeqCst);
        }
    }
}
pub(crate) struct Executor {
    pub(crate) prompts: Arc<Mutex<Vec<Value>>>,
    pub(crate) aborted: Arc<Mutex<Vec<String>>>,
    pub(crate) notifications: Arc<Mutex<Vec<Value>>>,
    pub(crate) prompt_fixture: Option<PromptFixture>,
    pub(crate) image_fixture: Option<ImageFixture>,
    pub(crate) notification_failure: bool,
}
impl Default for Executor {
    fn default() -> Self {
        Self {
            prompts: Arc::new(Mutex::new(vec![])),
            aborted: Arc::new(Mutex::new(vec![])),
            notifications: Arc::new(Mutex::new(vec![])),
            prompt_fixture: None,
            image_fixture: None,
            notification_failure: false,
        }
    }
}
impl WorkflowExecutor for Executor {
    fn catalog(&self) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async {
            Ok(
                json!({"cwd":"fixture","skills":[],"models":[{"provider":"fixture","model":"model","label":"fixture / model"}],"notificationTargets":{"browser":{"enabled":true},"feishu":{"enabled":true},"weixin":{"enabled":true},"qq":{"enabled":false},"telegram":{"enabled":false}}}),
            )
        })
    }
    fn prompt(&self, request: AgentRequest) -> BoxFuture<'_, Result<AgentResult>> {
        self.prompts.lock().unwrap().push(json!({"sessionId":request.session_id,"message":request.message,"executionMode":request.execution_mode,"isolatedContext":request.isolated_context,"model":request.model,"cwd":request.cwd,"title":request.title,"attachments":request.attachments,"requestedToolNames":request.requested_tool_names}));
        if let Some(fixture) = &self.prompt_fixture {
            return fixture(request);
        }
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(WorkflowError::cancelled());
            }
            let session = if request.session_id.is_empty() {
                super::id()?
            } else {
                request.session_id
            };
            (request.on_session)(session.clone()).await;
            if request.cancellation.is_cancelled() {
                return Err(WorkflowError::cancelled());
            }
            Ok(AgentResult {
                text: "fixture output".into(),
                session_id: session,
                assets: vec![],
            })
        })
    }
    fn abort(&self, id: String) -> BoxFuture<'_, Result<()>> {
        self.aborted.lock().unwrap().push(id);
        Box::pin(async { Ok(()) })
    }
    fn notify(&self, event: String, data: Value, options: Value) -> BoxFuture<'_, Result<()>> {
        self.notifications
            .lock()
            .unwrap()
            .push(json!({"event":event,"data":data,"options":options}));
        let fail = self.notification_failure;
        Box::pin(async move {
            if fail {
                Err(WorkflowError::invalid(
                    "通知发送失败：weixin: prepare failed",
                ))
            } else {
                Ok(())
            }
        })
    }
    fn media_inputs(&self, inputs: Value) -> BoxFuture<'_, Result<MediaInputs>> {
        Box::pin(async move {
            if inputs
                .as_object()
                .is_some_and(|inputs| inputs.values().any(Value::is_object))
            {
                Err(WorkflowError::coded(
                    "workflow_media_missing",
                    "workflow_media_missing",
                ))
            } else {
                Ok(MediaInputs::default())
            }
        })
    }
    fn image_node(&self, request: ImageNodeRequest) -> BoxFuture<'_, Result<ImageNodeResult>> {
        if let Some(fixture) = &self.image_fixture {
            return fixture(request);
        }
        Box::pin(async {
            Err(WorkflowError::coded(
                "workflow_image_unavailable",
                "workflow_image_unavailable",
            ))
        })
    }
}
pub(crate) async fn service(
    directory: &TempDirectory,
    executor: Arc<Executor>,
) -> Arc<WorkflowService> {
    WorkflowService::open(
        directory.path.join("workflows.json"),
        directory.path.to_string_lossy().into_owned(),
        executor,
        4,
    )
    .await
    .unwrap()
}
pub(crate) async fn wait_run(service: &WorkflowService, id: &str, status: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(run) = service.get_run(id).await {
                if run["status"] == status {
                    return run;
                }
                if ["failed", "cancelled", "completed"]
                    .contains(&run["status"].as_str().unwrap_or(""))
                    && run["status"] != status
                {
                    panic!("unexpected terminal workflow state: {run}");
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
