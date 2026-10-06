//! release 工作流领域的原生状态机；HTTP、Pi 会话与媒体实现通过窄接口组合。
use crate::native_workflow::{self as store, inputs, model, Result, WorkflowError};
use futures::{future::BoxFuture, stream::FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, Mutex, Notify};

pub(crate) type SessionObserver = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;
pub(crate) struct AgentRequest {
    pub(crate) session_id: String,
    pub(crate) message: String,
    pub(crate) attachments: Vec<Value>,
    pub(crate) cwd: String,
    pub(crate) title: String,
    pub(crate) model: Value,
    pub(crate) execution_mode: String,
    pub(crate) isolated_context: bool,
    pub(crate) requested_tool_names: Vec<String>,
    pub(crate) on_session: SessionObserver,
    pub(crate) cancellation: Arc<RunCancellation>,
}
#[derive(Default)]
pub(crate) struct AgentResult {
    pub(crate) text: String,
    pub(crate) session_id: String,
    pub(crate) assets: Vec<Value>,
}
#[derive(Default)]
pub(crate) struct MediaInputs {
    pub(crate) attachments: Vec<Value>,
    pub(crate) context: String,
}
pub(crate) struct ImageNodeRequest {
    pub(crate) node: Value,
    pub(crate) inputs: Value,
    pub(crate) predecessors: Vec<Value>,
    pub(crate) workflow_id: String,
    pub(crate) run_id: String,
    pub(crate) cwd: String,
    pub(crate) cancellation: Arc<RunCancellation>,
    pub(crate) resume_output: Option<Value>,
}
pub(crate) struct ImageNodeResult {
    pub(crate) output: Value,
    pub(crate) summary: String,
    pub(crate) assets: Vec<Value>,
}

pub(crate) trait WorkflowExecutor: Send + Sync {
    fn catalog(&self) -> BoxFuture<'_, Result<Value>>;
    fn prompt(&self, request: AgentRequest) -> BoxFuture<'_, Result<AgentResult>>;
    fn abort(&self, session_id: String) -> BoxFuture<'_, Result<()>>;
    fn notify(&self, event: String, data: Value, options: Value) -> BoxFuture<'_, Result<()>>;
    fn media_inputs(&self, inputs: Value) -> BoxFuture<'_, Result<MediaInputs>>;
    fn image_node(&self, request: ImageNodeRequest) -> BoxFuture<'_, Result<ImageNodeResult>>;
}

#[derive(Default)]
pub(crate) struct RunCancellation {
    token: tokio_util::sync::CancellationToken,
}
impl RunCancellation {
    pub(crate) fn cancel(&self) {
        self.token.cancel();
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
    pub(crate) async fn cancelled(&self) {
        self.token.cancelled().await;
    }
    pub(crate) fn token(&self) -> tokio_util::sync::CancellationToken {
        self.token.clone()
    }
    pub(crate) fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
        }
    }
}
struct RunControl {
    cancellation: Arc<RunCancellation>,
    sessions: std::sync::Mutex<HashSet<String>>,
    approvals: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    done: Notify,
    finished: AtomicBool,
    storage_error: std::sync::Mutex<Option<WorkflowError>>,
}
impl RunControl {
    fn new() -> Self {
        Self {
            cancellation: Arc::new(RunCancellation::default()),
            sessions: std::sync::Mutex::new(HashSet::new()),
            approvals: Mutex::new(HashMap::new()),
            done: Notify::new(),
            finished: AtomicBool::new(false),
            storage_error: std::sync::Mutex::new(None),
        }
    }
}
struct Inner {
    state: Value,
    starting: HashMap<String, Arc<RunControl>>,
    active: HashMap<String, Arc<RunControl>>,
    jobs: HashMap<String, Arc<RunControl>>,
    closed: bool,
}
pub(crate) struct WorkflowService {
    path: PathBuf,
    cwd: String,
    executor: Arc<dyn WorkflowExecutor>,
    max_concurrent: usize,
    inner: Mutex<Inner>,
}
impl WorkflowService {
    pub(crate) async fn open(
        path: PathBuf,
        cwd: String,
        executor: Arc<dyn WorkflowExecutor>,
        max_concurrent: usize,
    ) -> Result<Arc<Self>> {
        let stored = store::read_json(&path, json!({"version":2,"workflows":[],"runs":[]}))?;
        let mut workflows = stored["workflows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| model::normalize(w, &cwd, false))
            .collect::<Result<Vec<_>>>()?;
        for workflow in &mut workflows {
            if ["running", "waiting_approval"]
                .contains(&workflow["lastStatus"].as_str().unwrap_or(""))
            {
                workflow["lastStatus"] = json!("interrupted");
            }
        }
        let mut runs = stored["runs"].as_array().cloned().unwrap_or_default();
        if runs.len() > 200 {
            runs.drain(..runs.len() - 200);
        }
        for run in &mut runs {
            if ["running", "waiting_approval"].contains(&run["status"].as_str().unwrap_or("")) {
                run["status"] = json!("interrupted");
                run["finishedAt"] = json!(store::now());
                run["error"] = json!("应用重启，工作流运行已中断。");
            }
        }
        let state = json!({"version":2,"workflows":workflows,"runs":runs});
        store::save_json(&path, &state)?;
        Ok(Arc::new(Self {
            path,
            cwd,
            executor,
            max_concurrent: max_concurrent.max(1),
            inner: Mutex::new(Inner {
                state,
                starting: HashMap::new(),
                active: HashMap::new(),
                jobs: HashMap::new(),
                closed: false,
            }),
        }))
    }
    pub(crate) fn executor(&self) -> Arc<dyn WorkflowExecutor> {
        self.executor.clone()
    }
    pub(crate) async fn state(&self, session_id: Option<&str>) -> Value {
        let inner = self.inner.lock().await;
        let mut state = inner.state.clone();
        if let Some(id) = session_id.filter(|s| !s.is_empty()) {
            if let Some(runs) = state["runs"].as_array_mut() {
                runs.retain(|run| run["sourceSessionId"] == id);
            }
        }
        state["limits"] = json!({"maxConcurrent":self.max_concurrent,"running":inner.active.len()});
        state
    }
    pub(crate) async fn list(&self) -> Vec<Value> {
        self.inner.lock().await.state["workflows"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    pub(crate) async fn get_run(&self, id: &str) -> Option<Value> {
        self.inner.lock().await.state["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|run| run["id"] == id))
            .cloned()
    }
    pub(crate) async fn get_workflow(&self, id: &str) -> Option<Value> {
        self.inner.lock().await.state["workflows"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == id))
            .cloned()
    }
    async fn transaction<T>(&self, change: impl FnOnce(&mut Inner) -> Result<T>) -> Result<T> {
        let mut inner = self.inner.lock().await;
        let before = inner.state.clone();
        let result = match change(&mut inner) {
            Ok(value) => value,
            Err(error) => {
                inner.state = before;
                return Err(error);
            }
        };
        if let Err(error) = store::save_json(&self.path, &inner.state) {
            inner.state = before;
            return Err(error);
        }
        Ok(result)
    }
    fn normalize_input(&self, input: &Value, current: Option<&Value>) -> Result<Value> {
        let mut merged = current.cloned().unwrap_or(json!({}));
        let object = input
            .as_object()
            .ok_or_else(|| WorkflowError::invalid("工作流输入必须是对象。"))?;
        for (key, value) in object {
            merged[key] = value.clone();
        }
        if model::text(&merged["name"], 120).is_empty() {
            return Err(WorkflowError::invalid("工作流名称不能为空。"));
        }
        if object.contains_key("cwd") {
            let cwd = input["cwd"]
                .as_str()
                .ok_or_else(|| WorkflowError::invalid("工作目录无效。"))?;
            let selected = if cwd.trim().is_empty() {
                std::path::PathBuf::from(&self.cwd)
            } else {
                let requested = std::path::Path::new(cwd);
                if requested.is_absolute() {
                    requested.to_owned()
                } else {
                    std::path::Path::new(&self.cwd).join(requested)
                }
            };
            let path = std::fs::canonicalize(selected)
                .map_err(|_| WorkflowError::invalid("工作目录不存在。"))?;
            if !path.is_dir() {
                return Err(WorkflowError::invalid("工作目录不是目录。"));
            }
            merged["cwd"] = json!(path
                .to_string_lossy()
                .strip_prefix("\\\\?\\")
                .unwrap_or(&path.to_string_lossy()));
        }
        merged["updatedAt"] = json!(store::now());
        let mut workflow = model::normalize(&merged, &self.cwd, true)?;
        if workflow["status"] == "published" {
            model::graph(&workflow)?;
            if workflow["publishedAt"].is_null() {
                workflow["publishedAt"] = json!(store::now());
            }
        }
        Ok(workflow)
    }
    pub(crate) async fn create(&self, input: Value) -> Result<Value> {
        if !input.is_object() {
            return Err(WorkflowError::invalid("工作流输入必须是对象。"));
        }
        let mut input = input;
        input["id"] = json!(store::id()?);
        input["revision"] = json!(1);
        input["createdAt"] = json!(store::now());
        let workflow = self.normalize_input(&input, None)?;
        self.transaction(|inner| {
            inner.state["workflows"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("workflow state invalid"))?
                .insert(0, workflow.clone());
            Ok(workflow)
        })
        .await
    }
    pub(crate) async fn update(&self, id: &str, input: Value) -> Result<Option<Value>> {
        self.transaction(|inner| {
            if active_workflow(inner, id) {
                return Err(WorkflowError::busy("工作流正在运行，暂时不能修改。"));
            }
            let workflows = inner.state["workflows"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("workflow state invalid"))?;
            let Some(index) = workflows.iter().position(|w| w["id"] == id) else {
                return Ok(None);
            };
            let current = workflows[index].clone();
            let mut updated = self.normalize_input(&input, Some(&current))?;
            for key in [
                "id",
                "createdAt",
                "lastRunAt",
                "lastStatus",
                "lastSummary",
                "lastError",
            ] {
                updated[key] = current[key].clone();
            }
            updated["revision"] = json!(current["revision"].as_f64().unwrap_or(1.0) + 1.0);
            workflows[index] = updated.clone();
            Ok(Some(updated))
        })
        .await
    }
    pub(crate) async fn duplicate(&self, id: &str, input: Value) -> Result<Option<Value>> {
        let Some(mut workflow) = self.get_workflow(id).await else {
            return Ok(None);
        };
        let mut ids = HashMap::new();
        for node in workflow["nodes"]
            .as_array_mut()
            .ok_or_else(|| WorkflowError::io("workflow nodes invalid"))?
        {
            let old = model::text(&node["id"], usize::MAX);
            let fresh = store::id()?;
            node["id"] = json!(fresh);
            ids.insert(old, fresh);
        }
        for edge in workflow["edges"]
            .as_array_mut()
            .ok_or_else(|| WorkflowError::io("workflow edges invalid"))?
        {
            edge["id"] = json!(store::id()?);
            for endpoint in ["source", "target"] {
                let old = model::text(&edge[endpoint], usize::MAX);
                edge[endpoint] = json!(ids.get(&old).cloned().unwrap_or_default());
            }
        }
        workflow["name"] = json!(input["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{} 副本", model::text(&workflow["name"], 120))));
        for (key, value) in input.as_object().into_iter().flatten() {
            if key != "name" {
                workflow[key] = value.clone();
            }
        }
        workflow["status"] = json!("draft");
        workflow["publishedAt"] = Value::Null;
        workflow["lastRunAt"] = Value::Null;
        workflow["lastStatus"] = json!("idle");
        workflow["lastSummary"] = json!("");
        workflow["lastError"] = json!("");
        Ok(Some(self.create(workflow).await?))
    }
    pub(crate) async fn export(&self, id: &str) -> Option<Value> {
        let mut workflow = self.get_workflow(id).await?;
        if let Some(object) = workflow.as_object_mut() {
            for key in [
                "id",
                "createdAt",
                "updatedAt",
                "publishedAt",
                "lastRunAt",
                "lastStatus",
                "lastSummary",
                "lastError",
            ] {
                object.remove(key);
            }
        }
        Some(json!({"format":"pisper-workflow","version":1,"workflow":workflow}))
    }
    pub(crate) async fn import(&self, input: Value) -> Result<Value> {
        if input["format"] != "pisper-workflow" || !input["workflow"].is_object() {
            return Err(WorkflowError::invalid("不是有效的 Pisper 工作流文件。"));
        }
        let mut workflow = input["workflow"].clone();
        workflow["status"] = json!("draft");
        workflow["visibility"] = json!("private");
        self.create(workflow).await
    }
    pub(crate) async fn remove(&self, id: &str) -> Result<bool> {
        self.transaction(|inner| {
            if active_workflow(inner, id) {
                return Err(WorkflowError::busy("工作流正在运行，暂时不能删除。"));
            }
            let workflows = inner.state["workflows"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("workflow state invalid"))?;
            let before = workflows.len();
            workflows.retain(|w| w["id"] != id);
            let deleted = before != workflows.len();
            if deleted {
                if let Some(runs) = inner.state["runs"].as_array_mut() {
                    runs.retain(|r| r["workflowId"] != id);
                }
            }
            Ok(deleted)
        })
        .await
    }

    pub(crate) async fn run(self: &Arc<Self>, id: &str, options: Value) -> Result<Option<Value>> {
        let (workflow, starter) = {
            let mut inner = self.inner.lock().await;
            if inner.closed {
                return Err(WorkflowError::cancelled());
            }
            let Some(workflow) = inner.state["workflows"]
                .as_array()
                .and_then(|workflows| workflows.iter().find(|workflow| workflow["id"] == id))
                .cloned()
            else {
                return Ok(None);
            };
            if active_workflow(&inner, id) {
                return Err(WorkflowError::busy("工作流已经在运行。"));
            }
            if inner.active.len() + inner.starting.len() >= self.max_concurrent {
                return Err(WorkflowError::busy(format!(
                    "工作流并发已达到上限（{}）。",
                    self.max_concurrent
                )));
            }
            let starter = Arc::new(RunControl::new());
            inner.starting.insert(id.into(), starter.clone());
            (workflow, starter)
        };
        let (service, id) = (self.clone(), id.to_owned());
        let (sender, receiver) = oneshot::channel();
        // 准备任务由服务持有；观察请求离开不能泄漏 starting 预留或留下无人管理的执行。
        tokio::spawn(async move {
            let result = tokio::select! {_=starter.cancellation.cancelled()=>Err(WorkflowError::cancelled()),result=service.start_run(&id,options,workflow)=>result};
            {
                let mut inner = service.inner.lock().await;
                if inner
                    .starting
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &starter))
                {
                    inner.starting.remove(&id);
                }
            }
            starter.finished.store(true, Ordering::Release);
            starter.done.notify_waiters();
            let _ = sender.send(result);
        });
        receiver
            .await
            .map_err(|_| WorkflowError::io("工作流准备任务意外终止。"))?
    }
    async fn start_run(
        self: &Arc<Self>,
        id: &str,
        options: Value,
        workflow: Value,
    ) -> Result<Option<Value>> {
        let mut graph = model::graph(&workflow)?;
        let node_id = options["nodeId"].as_str().filter(|s| !s.is_empty());
        let mut reused = Vec::new();
        let mut resume_output = None;
        let mut image_predecessors = None;
        let source = if node_id.is_some() {
            self.get_run(options["sourceRunId"].as_str().unwrap_or(""))
                .await
        } else {
            None
        };
        let source_inputs = source.as_ref().map(|run| &run["inputs"]);
        let normalized_inputs = inputs::validate(
            workflow.get("inputs"),
            options.get("inputs").or(source_inputs),
        )?;
        let input_names = workflow["inputs"]
            .as_array()
            .filter(|a| !a.is_empty())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|v| v["name"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or(vec!["task".into()]);
        let node_ids = graph.order.clone();
        for node in &graph.nodes {
            for template in [
                &node["prompt"],
                &node["approval"]["message"],
                &node["notification"]["title"],
                &node["notification"]["content"],
            ] {
                inputs::validate_template(
                    template.as_str().unwrap_or(""),
                    &input_names,
                    &node_ids,
                )?;
            }
        }
        self.executor
            .media_inputs(normalized_inputs.clone())
            .await?;
        if let Some(node_id) = node_id {
            let stale = || {
                WorkflowError::coded("workflow_image_source_stale", "workflow_image_source_stale")
            };
            let node = graph
                .nodes
                .iter()
                .find(|node| node["id"] == node_id)
                .cloned()
                .ok_or_else(stale)?;
            let source = source
                .as_ref()
                .filter(|run| {
                    run["workflowId"] == id
                        && !["running", "waiting_approval"]
                            .contains(&run["status"].as_str().unwrap_or(""))
                })
                .ok_or_else(stale)?;
            if !model::IMAGE_KINDS.contains(&node["kind"].as_str().unwrap_or("")) {
                return Err(stale());
            }
            let ancestors = reachable(node_id, &graph.incoming, "source");
            let descendants = reachable(node_id, &graph.outgoing, "target");
            for ancestor in ancestors {
                let current = graph
                    .nodes
                    .iter()
                    .find(|n| n["id"] == ancestor)
                    .ok_or_else(stale)?;
                let cached = source["nodes"]
                    .as_array()
                    .and_then(|nodes| nodes.iter().find(|n| n["id"] == ancestor))
                    .filter(|n| {
                        n["status"] == "completed"
                            && n["configurationHash"] == model::configuration_hash(current)
                    })
                    .ok_or_else(stale)?;
                let _ = cached;
            }
            let previous = source["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find(|n| n["id"] == node_id));
            if node["kind"] == "media-generate" {
                if let Some(previous) = previous.filter(|n| {
                    n["status"] == "failed"
                        && n["configurationHash"] == model::configuration_hash(&node)
                        && n["output"].is_object()
                }) {
                    resume_output = Some(previous["output"].clone());
                }
            }
            reused = source["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| {
                    n["id"] != node_id
                        && !descendants.contains(n["id"].as_str().unwrap_or(""))
                        && n["status"] == "completed"
                        && graph.nodes.iter().any(|current| current["id"] == n["id"])
                })
                .cloned()
                .map(|mut n| {
                    n["reused"] = json!(true);
                    n
                })
                .collect();
            image_predecessors = Some(
                graph.incoming[node_id]
                    .iter()
                    .filter_map(|edge| reused.iter().find(|n| n["id"] == edge["source"]).cloned())
                    .collect::<Vec<_>>(),
            );
            graph.nodes = vec![node];
            graph.order = vec![node_id.to_owned()];
            graph.edges.clear();
            graph.incoming = HashMap::from([(node_id.to_owned(), Vec::new())]);
            graph.outgoing = graph.incoming.clone();
        }
        let mut node_runs = reused.clone();
        for node_id in &graph.order {
            let node = graph
                .nodes
                .iter()
                .find(|n| n["id"] == *node_id)
                .ok_or_else(|| WorkflowError::io("graph node missing"))?;
            node_runs.push(json!({"id":node["id"],"label":node["label"],"kind":node["kind"],"status":"pending","attempts":0,"summary":"","output":"","error":"","sessionId":"","startedAt":null,"finishedAt":null,"durationMs":0,"selectedPort":"","approval":null,"skipReason":"","configurationHash":model::configuration_hash(node)}));
        }
        let run_id = store::id()?;
        let stamp = store::now();
        let mut run = json!({"id":run_id,"workflowId":workflow["id"],"workflowName":workflow["name"],"workflowRevision":workflow["revision"],"trigger":if node_id.is_some(){"node"}else{options["trigger"].as_str().unwrap_or("manual")},"sourceSessionId":options["sourceSessionId"].as_str().unwrap_or(""),"sourceMessage":model::text(&options["sourceMessage"],12000),"retryOf":options["retryOf"].as_str().unwrap_or(""),"inputs":normalized_inputs,"status":"running","startedAt":stamp,"finishedAt":null,"durationMs":0,"completedNodes":reused.len(),"totalNodes":node_runs.len(),"currentNodeId":"","currentNodeLabel":"","summary":"","error":"","sessionId":"","assets":[],"nodes":node_runs});
        if let Some(node_id) = node_id {
            run["sourceRunId"] = source
                .as_ref()
                .map(|r| r["id"].clone())
                .unwrap_or(Value::Null);
            run["nodeId"] = json!(node_id);
        }
        let control = Arc::new(RunControl::new());
        {
            let mut inner = self.inner.lock().await;
            if inner.closed {
                return Err(WorkflowError::cancelled());
            }
            if active_workflow_runs(&inner, id) {
                return Err(WorkflowError::busy("工作流已经在运行。"));
            }
            if inner.active.len() >= self.max_concurrent {
                return Err(WorkflowError::busy(format!(
                    "工作流并发已达到上限（{}）。",
                    self.max_concurrent
                )));
            }
            let before = inner.state.clone();
            inner.state["runs"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("run state invalid"))?
                .push(run.clone());
            if let Some(runs) = inner.state["runs"].as_array_mut() {
                if runs.len() > 200 {
                    runs.drain(..runs.len() - 200);
                }
            }
            if let Some(workflow) = find_workflow_mut(&mut inner.state, id) {
                workflow["lastRunAt"] = json!(stamp);
                workflow["lastStatus"] = json!("running");
                workflow["lastError"] = json!("");
            }
            if let Err(error) = store::save_json(&self.path, &inner.state) {
                inner.state = before;
                return Err(error);
            }
            inner.active.insert(run_id.clone(), control.clone());
            inner.jobs.insert(run_id.clone(), control.clone());
            inner.starting.remove(id);
        }
        let service = self.clone();
        tokio::spawn(async move {
            service
                .execute(
                    workflow,
                    run_id,
                    graph,
                    control,
                    image_predecessors,
                    resume_output,
                )
                .await;
        });
        Ok(Some(run))
    }
    pub(crate) async fn retry(self: &Arc<Self>, id: &str) -> Result<Option<Value>> {
        let Some(run) = self.get_run(id).await.filter(|r| {
            ["failed", "cancelled", "interrupted"].contains(&r["status"].as_str().unwrap_or(""))
        }) else {
            return Ok(None);
        };
        self.run(run["workflowId"].as_str().unwrap_or(""),json!({"trigger":"retry","inputs":run["inputs"],"sourceSessionId":run["sourceSessionId"],"sourceMessage":run["sourceMessage"],"retryOf":run["id"]})).await
    }
    pub(crate) async fn approval(
        &self,
        run_id: &str,
        node_id: &str,
        approved: bool,
        comment: String,
    ) -> Result<Option<Value>> {
        let control = self.inner.lock().await.active.get(run_id).cloned();
        let Some(control) = control else {
            return Ok(None);
        };
        let pending = control.approvals.lock().await.remove(node_id);
        let Some(pending) = pending else {
            return Ok(None);
        };
        let _ = pending.send(
            json!({"approved":approved,"comment":comment.chars().take(1000).collect::<String>()}),
        );
        Ok(self.get_run(run_id).await)
    }
    pub(crate) async fn stop(&self, run_id: &str) -> Result<Option<Value>> {
        let control = self.inner.lock().await.active.get(run_id).cloned();
        let Some(control) = control else {
            return Ok(None);
        };
        control.cancellation.cancel();
        control.approvals.lock().await.clear();
        let sessions = control
            .sessions
            .lock()
            .map(|mut ids| ids.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        for id in sessions {
            let _ = self.executor.abort(id).await;
        }
        Ok(self.get_run(run_id).await)
    }
    pub(crate) async fn dispose(&self) -> Result<()> {
        let (controls, starters) = {
            let mut inner = self.inner.lock().await;
            inner.closed = true;
            let controls = inner
                .jobs
                .iter()
                .map(|(id, c)| (id.clone(), c.clone()))
                .collect::<Vec<_>>();
            let starters = inner.starting.values().cloned().collect::<Vec<_>>();
            (controls, starters)
        };
        for starter in &starters {
            starter.cancellation.cancel();
        }
        for starter in starters {
            let notified = starter.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !starter.finished.load(Ordering::Acquire) {
                notified.await;
            }
        }
        for (id, _) in &controls {
            self.stop(id).await?;
        }
        for (_, control) in controls {
            let notified = control.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !control.finished.load(Ordering::Acquire) {
                notified.await;
            }
        }
        Ok(())
    }
    async fn change_run(&self, run_id: &str, change: impl FnOnce(&mut Value)) -> Result<()> {
        self.transaction(|inner| {
            let run = find_run_mut(&mut inner.state, run_id)
                .ok_or_else(|| WorkflowError::io("工作流运行记录不存在。"))?;
            change(run);
            Ok(())
        })
        .await
    }
    async fn execute(
        self: Arc<Self>,
        workflow: Value,
        run_id: String,
        graph: model::Graph,
        control: Arc<RunControl>,
        image_predecessors: Option<Vec<Value>>,
        resume_output: Option<Value>,
    ) {
        let started = Instant::now();
        let mut launched = HashSet::new();
        let mut blocked = HashSet::new();
        let mut active = FuturesUnordered::new();
        let mut failure = None;
        loop {
            let Some(snapshot) = self.get_run(&run_id).await else {
                failure = Some(WorkflowError::io("工作流运行记录不存在。"));
                break;
            };
            for node_id in &graph.order {
                if launched.contains(node_id) {
                    continue;
                }
                let incoming = &graph.incoming[node_id];
                let dependency_ready = incoming.iter().all(|edge| {
                    blocked.contains(edge["source"].as_str().unwrap_or(""))
                        || snapshot["nodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|n| n["id"] == edge["source"])
                            .is_some_and(|n| {
                                !["pending", "running", "waiting_approval"]
                                    .contains(&n["status"].as_str().unwrap_or(""))
                            })
                });
                if !dependency_ready {
                    continue;
                }
                if incoming.iter().any(|edge| {
                    blocked.contains(edge["source"].as_str().unwrap_or(""))
                        || snapshot["nodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|n| n["id"] == edge["source"] && n["status"] == "failed")
                }) {
                    blocked.insert(node_id.clone());
                    launched.insert(node_id.clone());
                    continue;
                }
                let Some(node) = graph.nodes.iter().find(|n| n["id"] == *node_id).cloned() else {
                    continue;
                };
                launched.insert(node_id.clone());
                let service = self.clone();
                let wf = workflow.clone();
                let rid = run_id.clone();
                let ctl = control.clone();
                let g = graph.clone();
                let predecessors = image_predecessors.clone();
                let resume = resume_output.clone();
                active.push(async move {
                    service
                        .execute_node(&wf, &rid, &g, ctl, node, predecessors, resume)
                        .await
                });
            }
            if active.is_empty() {
                break;
            }
            if let Some(Err(error)) = active.next().await {
                if error.code == "workflow_storage_error" {
                    if let Ok(mut saved) = control.storage_error.lock() {
                        *saved = Some(error.clone());
                    }
                }
                failure = Some(error);
                control.cancellation.cancel();
            }
            if control.cancellation.is_cancelled() && failure.is_none() {
                failure = Some(WorkflowError::cancelled());
            }
        }
        let snapshot = self.get_run(&run_id).await.unwrap_or(json!({}));
        if failure.is_none() {
            if let Some(failed) = snapshot["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|n| n["status"] == "failed")
            {
                failure = Some(WorkflowError::invalid(model::text(&failed["error"], 1200)));
            }
        }
        if let Some(error) = control
            .storage_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
        {
            failure = Some(error);
        }
        let status = if failure
            .as_ref()
            .is_some_and(|error| error.code == "workflow_storage_error")
        {
            "failed"
        } else if control.cancellation.is_cancelled() {
            "cancelled"
        } else if failure.is_some() {
            "failed"
        } else {
            "completed"
        };
        let error = failure
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        let summary = graph
            .order
            .iter()
            .filter(|id| graph.outgoing[*id].is_empty())
            .filter_map(|id| {
                snapshot["nodes"].as_array().and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|n| n["id"] == *id && n["status"] != "skipped")
                })
            })
            .map(|n| model::text(&n["summary"], 1200))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let summary = if summary.is_empty() {
            "工作流已完成。".into()
        } else {
            summary
        };
        let result = self
            .transaction(|inner| {
                if let Some(run) = find_run_mut(&mut inner.state, &run_id) {
                    if let Some(nodes) = run["nodes"].as_array_mut() {
                        for node in nodes.iter_mut().filter(|node| {
                            node["status"] == "pending"
                                || failure
                                    .as_ref()
                                    .is_some_and(|error| error.code == "workflow_storage_error")
                                    && ["running", "waiting_approval"]
                                        .contains(&node["status"].as_str().unwrap_or(""))
                        }) {
                            node["status"] = json!("skipped");
                            node["summary"] = json!(if status == "cancelled" {
                                "工作流已停止。"
                            } else {
                                "上游节点失败，未执行。"
                            });
                            node["output"] = node["summary"].clone();
                            node["finishedAt"] = json!(store::now());
                        }
                    }
                    run["status"] = json!(status);
                    run["error"] = json!(error);
                    if status == "completed" {
                        run["summary"] = json!(summary);
                    }
                    run["currentNodeId"] = json!("");
                    run["currentNodeLabel"] = json!("");
                    run["finishedAt"] = json!(store::now());
                    run["durationMs"] = json!(started.elapsed().as_millis() as u64);
                }
                if let Some(workflow) =
                    find_workflow_mut(&mut inner.state, workflow["id"].as_str().unwrap_or(""))
                {
                    workflow["lastStatus"] = json!(status);
                    workflow["lastError"] = json!(error);
                    workflow["updatedAt"] = json!(store::now());
                    if status == "completed" {
                        workflow["lastSummary"] = json!(summary);
                    }
                }
                Ok(())
            })
            .await;
        if let Err(storage_error) = result {
            self.record_storage_failure(&run_id, &storage_error).await;
            tracing::error!(run_id=%run_id,"Cannot persist terminal workflow state");
        }
        self.inner.lock().await.active.remove(&run_id);
        if self
            .get_run(&run_id)
            .await
            .is_some_and(|run| run["status"] == status)
            && workflow["notifications"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
            && ["completed", "failed"].contains(&status)
        {
            let event = format!("workflow.{status}");
            let data = if status == "completed" {
                json!({"workflow":{"name":workflow["name"],"summary":summary,"duration":duration_label(started.elapsed()),"runId":run_id}})
            } else {
                json!({"workflow":{"name":workflow["name"],"node":snapshot["nodes"].as_array().into_iter().flatten().find(|n|n["status"]=="failed").map(|n|n["label"].clone()).unwrap_or(json!("未知节点")),"error":error,"runId":run_id}})
            };
            let _ = self
                .executor
                .notify(event, data, json!({"platforms":workflow["notifications"]}))
                .await;
        }
        self.inner.lock().await.jobs.remove(&run_id);
        control.finished.store(true, Ordering::Release);
        control.done.notify_waiters();
    }
    async fn record_storage_failure(&self, run_id: &str, error: &WorkflowError) {
        let mut inner = self.inner.lock().await;
        inner.state["persistenceError"] = json!({"code":error.code,"message":error.message});
        let mut workflow_id = String::new();
        if let Some(run) = find_run_mut(&mut inner.state, run_id) {
            workflow_id = model::text(&run["workflowId"], 200);
            run["status"] = json!("failed");
            run["error"] = json!(error.message);
            run["errorCode"] = json!(error.code);
            run["finishedAt"] = json!(store::now());
        }
        if let Some(workflow) = find_workflow_mut(&mut inner.state, &workflow_id) {
            workflow["lastStatus"] = json!("failed");
            workflow["lastError"] = json!(error.message);
        }
    }
    async fn execute_node(
        self: &Arc<Self>,
        workflow: &Value,
        run_id: &str,
        graph: &model::Graph,
        control: Arc<RunControl>,
        node: Value,
        image_predecessors: Option<Vec<Value>>,
        resume_output: Option<Value>,
    ) -> Result<()> {
        let node_id = node["id"].as_str().unwrap_or("");
        let started = Instant::now();
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| WorkflowError::io("工作流运行记录不存在。"))?;
        let incoming = &graph.incoming[node_id];
        let source_runs = incoming
            .iter()
            .filter_map(|edge| {
                run["nodes"]
                    .as_array()
                    .and_then(|nodes| nodes.iter().find(|n| n["id"] == edge["source"]))
            })
            .cloned()
            .collect::<Vec<_>>();
        let edge_active = |edge: &Value| {
            source_runs
                .iter()
                .find(|n| n["id"] == edge["source"])
                .is_some_and(|n| {
                    !["failed", "cancelled"].contains(&n["status"].as_str().unwrap_or(""))
                        && !(n["status"] == "skipped" && n["skipReason"] == "branch_not_selected")
                        && (n["kind"] != "condition" || n["selectedPort"] == edge["sourcePort"])
                })
        };
        if !incoming.is_empty() && !incoming.iter().any(edge_active) {
            self.change_run(run_id, |run| {
                if let Some(n) = find_node_mut(run, node_id) {
                    n["status"] = json!("skipped");
                    n["skipReason"] = json!("branch_not_selected");
                    n["summary"] = json!("上游条件分支未命中。");
                    n["output"] = json!("上游条件分支未命中。");
                    n["finishedAt"] = json!(store::now());
                }
                increment_completed(run);
            })
            .await?;
            return Ok(());
        }
        let predecessors = image_predecessors.unwrap_or_else(|| {
            source_runs
                .into_iter()
                .filter(|n| {
                    !["skipped", "failed", "cancelled"]
                        .contains(&n["status"].as_str().unwrap_or(""))
                })
                .collect()
        });
        let start_result = self
            .change_run(run_id, |run| {
                if let Some(n) = find_node_mut(run, node_id) {
                    n["status"] = json!("running");
                    n["startedAt"] = json!(store::now());
                }
                run["currentNodeId"] = node["id"].clone();
                run["currentNodeLabel"] = node["label"].clone();
            })
            .await;
        let result = if let Err(error) = start_result {
            Err(error)
        } else if control.cancellation.is_cancelled() {
            Err(WorkflowError::cancelled())
        } else {
            self.node_body(
                workflow,
                run_id,
                graph,
                control.clone(),
                &node,
                &predecessors,
                resume_output,
            )
            .await
        };
        self.change_run(run_id, |run| {
            if let Some(n) = find_node_mut(run, node_id) {
                match result {
                    Ok((output, summary, port, assets)) => {
                        n["status"] = json!("completed");
                        n["output"] = output;
                        n["summary"] = json!(summary);
                        if let Some(port) = port {
                            n["selectedPort"] = json!(port);
                        }
                        merge_assets(run, assets);
                    }
                    Err(error) => {
                        if let Some(output) = error
                            .partial_output
                            .as_ref()
                            .and_then(|value| store::image_protocol::output(value).ok())
                        {
                            n["output"] = output;
                        }
                        n["error"] = json!(error.message);
                        if control.cancellation.is_cancelled() || error.code == "WORKFLOW_CANCELLED"
                        {
                            n["status"] = json!("cancelled");
                        } else if node["failurePolicy"] == "skip"
                            && error.code != "workflow_storage_error"
                        {
                            n["status"] = json!("skipped");
                            n["skipReason"] = json!("failure_policy");
                            n["summary"] = json!(format!("已跳过：{}", error.message));
                            n["output"] = n["summary"].clone();
                        } else {
                            n["status"] = json!("failed");
                        }
                    }
                }
            }
            if let Some(n) = find_node_mut(run, node_id) {
                n["finishedAt"] = json!(store::now());
                n["durationMs"] = json!(started.elapsed().as_millis() as u64);
            }
            increment_completed(run);
        })
        .await?;
        Ok(())
    }
    async fn node_body(
        self: &Arc<Self>,
        workflow: &Value,
        run_id: &str,
        graph: &model::Graph,
        control: Arc<RunControl>,
        node: &Value,
        predecessors: &[Value],
        resume_output: Option<Value>,
    ) -> Result<(Value, String, Option<String>, Vec<Value>)> {
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| WorkflowError::io("工作流运行记录不存在。"))?;
        let kind = node["kind"].as_str().unwrap_or("");
        let context = model::template_context(workflow, &run, predecessors);
        let outputs = || Value::Array(predecessors.iter().map(|n| n["output"].clone()).collect());
        match kind {
            "trigger" => Ok((
                run["inputs"].clone(),
                if run["inputs"].as_object().is_some_and(|o| !o.is_empty()) {
                    pretty(&run["inputs"])
                } else {
                    "工作流已触发。".into()
                },
                None,
                vec![],
            )),
            "condition" => {
                let nodes = run["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|node| node["id"].as_str().map(|id| (id.to_string(), node.clone())))
                    .collect::<serde_json::Map<_, _>>();
                let matched = model::condition(
                    &node["condition"],
                    &json!({"inputs":run["inputs"],"previous":predecessors.first().cloned().unwrap_or(Value::Null),"nodes":nodes}),
                );
                Ok((
                    json!(matched),
                    if matched {
                        "条件成立，进入 true 分支。"
                    } else {
                        "条件不成立，进入 false 分支。"
                    }
                    .into(),
                    Some(if matched { "true" } else { "false" }.into()),
                    vec![],
                ))
            }
            "parallel" => Ok((
                outputs(),
                if predecessors.is_empty() {
                    "并行分支已启动。"
                } else {
                    "并行分支已汇合。"
                }
                .into(),
                None,
                vec![],
            )),
            "approval" => {
                let node_id = node["id"].as_str().unwrap_or("");
                let message =
                    inputs::render(node["approval"]["message"].as_str().unwrap_or(""), &context)?;
                let timeout = Duration::from_secs_f64(
                    node["approval"]["timeoutMinutes"].as_f64().unwrap_or(60.0) * 60.0,
                );
                let (sender, receiver) = oneshot::channel();
                control
                    .approvals
                    .lock()
                    .await
                    .insert(node_id.into(), sender);
                let requested = store::now();
                let expires = (chrono::Utc::now()
                    + chrono::Duration::from_std(timeout).map_err(WorkflowError::io)?)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                self.change_run(run_id,|run|{run["status"]=json!("waiting_approval");if let Some(n)=find_node_mut(run,node_id){n["status"]=json!("waiting_approval");n["approval"]=json!({"message":if message.is_empty(){format!("是否允许工作流继续执行「{}」？",model::text(&node["label"],120))}else{message},"requestedAt":requested,"expiresAt":expires});}}).await?;
                let decision = tokio::select! {_=control.cancellation.cancelled()=>Err(WorkflowError::cancelled()),response=tokio::time::timeout(timeout,receiver)=>match response{Ok(Ok(value))=>Ok(value),Ok(Err(_))=>Err(WorkflowError::cancelled()),Err(_)=>Err(WorkflowError::invalid(format!("审批节点「{}」等待超时。",model::text(&node["label"],120))))}};
                control.approvals.lock().await.remove(node_id);
                let decision = decision?;
                self.change_run(run_id, |run| {
                    run["status"] = json!("running");
                    if let Some(n) = find_node_mut(run, node_id) {
                        n["approval"]["approved"] = decision["approved"].clone();
                        n["approval"]["comment"] = decision["comment"].clone();
                        n["approval"]["resolvedAt"] = json!(store::now());
                    }
                })
                .await?;
                if decision["approved"] != true {
                    return Err(WorkflowError::invalid(
                        decision["comment"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                format!("审批节点「{}」已拒绝。", model::text(&node["label"], 120))
                            }),
                    ));
                }
                Ok((
                    decision.clone(),
                    decision["comment"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("审批已通过。")
                        .into(),
                    None,
                    vec![],
                ))
            }
            "notification" => {
                let content = inputs::render(
                    node["notification"]["content"].as_str().unwrap_or(""),
                    &context,
                )?;
                let content = if content.trim().is_empty() {
                    let summaries = predecessors
                        .iter()
                        .map(|n| model::text(&n["summary"], 1200))
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if summaries.is_empty() {
                        "通知已发送。".into()
                    } else {
                        summaries
                    }
                } else {
                    content.trim().to_owned()
                };
                let title = inputs::render(
                    node["notification"]["title"].as_str().unwrap_or(""),
                    &context,
                )?;
                if node["notificationTargets"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
                {
                    let mut options =
                        json!({"platforms":node["notificationTargets"],"content":content});
                    if !title.trim().is_empty() {
                        options["title"] = json!(title.trim());
                    }
                    self.executor.notify("workflow.completed".into(),json!({"workflow":{"name":workflow["name"],"summary":content,"duration":duration_since(run["startedAt"].as_str().unwrap_or("")),"runId":run_id}}),options).await?;
                }
                Ok((
                    outputs(),
                    content.chars().take(1200).collect(),
                    None,
                    vec![],
                ))
            }
            kind if model::IMAGE_KINDS.contains(&kind) => {
                let mut rendered_node = node.clone();
                rendered_node["prompt"] = json!(inputs::render(
                    node["prompt"].as_str().unwrap_or(""),
                    &context
                )?);
                self.change_run(run_id, |run| {
                    if let Some(n) = find_node_mut(run, node["id"].as_str().unwrap_or("")) {
                        n["attempts"] = json!(1);
                    }
                })
                .await?;
                let child = Arc::new(RunCancellation::default());
                let request = ImageNodeRequest {
                    node: rendered_node,
                    inputs: run["inputs"].clone(),
                    predecessors: predecessors.to_vec(),
                    workflow_id: model::text(&workflow["id"], 200),
                    run_id: run_id.into(),
                    cwd: model::text(&workflow["cwd"], usize::MAX),
                    cancellation: child.clone(),
                    resume_output,
                };
                let duration =
                    Duration::from_secs_f64(node["timeoutMinutes"].as_f64().unwrap_or(20.0) * 60.0);
                let mut execution = self.executor.image_node(request);
                let result = tokio::select! {
                    _ = control.cancellation.cancelled() => {
                        child.cancel();
                        let completed = execution.await;
                        let mut error = WorkflowError::cancelled();
                        error.partial_output = match completed { Ok(result) => Some(result.output), Err(result) => result.partial_output };
                        Err(error)
                    },
                    value = tokio::time::timeout(duration, &mut execution) => match value {
                        Ok(result) => result,
                        Err(_) => {
                            child.cancel();
                            let completed = execution.await;
                            let mut error = WorkflowError::coded("WORKFLOW_TIMEOUT", "workflow_image_timeout");
                            error.partial_output = match completed { Ok(result) => Some(result.output), Err(result) => result.partial_output };
                            Err(error)
                        }
                    }
                }?;
                let output = store::image_protocol::output(&result.output)?;
                Ok((
                    output,
                    result.summary.chars().take(1200).collect(),
                    None,
                    result.assets,
                ))
            }
            "prompt" | "skill" | "file" | "mcp" => {
                self.agent_node(
                    workflow,
                    run_id,
                    graph,
                    control,
                    node,
                    predecessors,
                    &run,
                    &context,
                )
                .await
            }
            _ => Ok((outputs(), "控制节点已通过。".into(), None, vec![])),
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn agent_node(
        self: &Arc<Self>,
        workflow: &Value,
        run_id: &str,
        graph: &model::Graph,
        control: Arc<RunControl>,
        node: &Value,
        predecessors: &[Value],
        run: &Value,
        context: &Value,
    ) -> Result<(Value, String, Option<String>, Vec<Value>)> {
        let node_id = node["id"].as_str().unwrap_or("");
        let mut last_error = None;
        let attempts = node["retries"].as_f64().unwrap_or(0.0).floor() as usize + 1;
        for attempt in 1..=attempts {
            self.change_run(run_id, |run| {
                if let Some(n) = find_node_mut(run, node_id) {
                    n["attempts"] = json!(attempt);
                }
            })
            .await?;
            let inherited = if predecessors.len() == 1
                && graph
                    .outgoing
                    .get(predecessors[0]["id"].as_str().unwrap_or(""))
                    .is_some_and(|edges| edges.len() == 1)
            {
                predecessors[0]["sessionId"].as_str().unwrap_or("")
            } else {
                ""
            };
            let media = self.executor.media_inputs(run["inputs"].clone()).await?;
            if control.cancellation.is_cancelled() {
                return Err(WorkflowError::cancelled());
            }
            let mut message = format!(
                "你正在执行工作流「{}」的节点「{}」。\n{}\n{}",
                model::text(&workflow["name"], 120),
                model::text(&node["label"], 120),
                match node["kind"].as_str().unwrap_or("") {
                    "skill" =>
                        if node["skillName"].as_str().is_some_and(|s| !s.is_empty()) {
                            format!(
                                "必须使用 Skill「{}」完成任务。",
                                model::text(&node["skillName"], 120)
                            )
                        } else {
                            "使用最合适的已启用 Skill 完成任务。".into()
                        },
                    "file" => "使用可用的文件工具完成这个文件处理任务。".into(),
                    "mcp" =>
                        if node["requestedToolNames"]
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                        {
                            format!(
                                "优先调用这些 MCP 工具：{}。",
                                model::strings(&node["requestedToolNames"], 20).join(", ")
                            )
                        } else {
                            "优先使用已启用的 MCP 工具完成这个任务。".into()
                        },
                    _ => "完成这个 Agent 任务。".into(),
                },
                inputs::render(node["prompt"].as_str().unwrap_or(""), context)?
            );
            if run["inputs"].as_object().is_some_and(|o| !o.is_empty()) {
                message.push_str(&format!("\n\n工作流输入：\n{}", pretty(&run["inputs"])));
            }
            if !media.context.is_empty() {
                message.push('\n');
                message.push_str(&media.context);
            }
            if !predecessors.is_empty() {
                message.push_str("\n\n前序节点结果：\n");
                message.push_str(
                    &predecessors
                        .iter()
                        .map(|n| {
                            format!(
                                "{}：{}",
                                model::text(&n["label"], 120),
                                pretty(&n["output"])
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            message.push_str(if node["outputFormat"] == "json" {
                "\n只输出有效 JSON，不要使用 Markdown 代码块。"
            } else {
                "\n完成后简洁总结结果，供后续节点继续使用。"
            });
            if node["kind"] == "skill" && node["skillName"].as_str().is_some_and(|s| !s.is_empty())
            {
                message = format!("/skill:{}\n{message}", model::text(&node["skillName"], 120));
            }
            let service = self.clone();
            let rid = run_id.to_owned();
            let nid = node_id.to_owned();
            let ctl = control.clone();
            let on_session: SessionObserver = Arc::new(move |id| {
                let service = service.clone();
                let rid = rid.clone();
                let nid = nid.clone();
                let ctl = ctl.clone();
                Box::pin(async move {
                    if let Ok(mut sessions) = ctl.sessions.lock() {
                        sessions.insert(id.clone());
                    }
                    if let Err(error) = service
                        .change_run(&rid, |run| {
                            run["sessionId"] = json!(id);
                            if let Some(n) = find_node_mut(run, &nid) {
                                n["sessionId"] = json!(id);
                            }
                        })
                        .await
                    {
                        if let Ok(mut saved) = ctl.storage_error.lock() {
                            *saved = Some(error);
                        }
                        ctl.cancellation.cancel();
                    }
                })
            });
            let cancellation = Arc::new(control.cancellation.child());
            let request = AgentRequest {
                session_id: inherited.into(),
                message,
                attachments: media.attachments,
                cwd: model::text(&workflow["cwd"], usize::MAX),
                title: format!("工作流 · {}", model::text(&workflow["name"], 120)),
                model: if node["model"].is_null() {
                    workflow["model"].clone()
                } else {
                    node["model"].clone()
                },
                execution_mode: node["executionMode"]
                    .as_str()
                    .unwrap_or("full-access")
                    .into(),
                isolated_context: true,
                requested_tool_names: model::strings(&node["requestedToolNames"], 20),
                on_session,
                cancellation: cancellation.clone(),
            };
            let duration =
                Duration::from_secs_f64(node["timeoutMinutes"].as_f64().unwrap_or(20.0) * 60.0);
            let mut execution = self.executor.prompt(request);
            // The executor owns native finally/asset capture. Borrow its future
            // in select, then keep polling it after cancellation or expiry.
            let result = tokio::select! {
                _ = control.cancellation.cancelled() => {
                    cancellation.cancel();
                    let _ = execution.await;
                    Err(WorkflowError::cancelled())
                },
                _ = tokio::time::sleep(duration) => {
                        cancellation.cancel();
                        let current = self.get_run(run_id).await;
                        let session = current.as_ref().and_then(|r|r["nodes"].as_array()).and_then(|nodes|nodes.iter().find(|n|n["id"]==node_id)).and_then(|n|n["sessionId"].as_str()).unwrap_or("");
                        if session.is_empty() {
                            let _ = execution.await;
                        } else {
                            // Native abort can await the prompt's finally. Both
                            // futures must advance together until they finish.
                            let _ = tokio::join!(execution, self.executor.abort(session.into()));
                        }
                        Err(WorkflowError::coded("WORKFLOW_TIMEOUT",format!("节点「{}」执行超过 {} 分钟。",model::text(&node["label"],120),node["timeoutMinutes"])))
                },
                result = &mut execution => result,
            };
            match result {
                Ok(result) => {
                    if control.cancellation.is_cancelled() {
                        return Err(WorkflowError::cancelled());
                    }
                    if !result.session_id.is_empty() {
                        if let Ok(mut sessions) = control.sessions.lock() {
                            sessions.insert(result.session_id.clone());
                        }
                        self.change_run(run_id, |run| {
                            run["sessionId"] = json!(result.session_id);
                            if let Some(n) = find_node_mut(run, node_id) {
                                n["sessionId"] = json!(result.session_id);
                            }
                        })
                        .await?;
                    }
                    let text = if result.text.trim().is_empty() {
                        "节点已完成。".into()
                    } else {
                        result.text.trim().chars().take(100000).collect::<String>()
                    };
                    let output = if node["outputFormat"] == "json" {
                        match serde_json::from_str(&text) {
                            Ok(value) => value,
                            Err(_) => {
                                last_error = Some(WorkflowError::invalid(
                                    "节点声明了 JSON 输出，但模型没有返回有效 JSON。",
                                ));
                                continue;
                            }
                        }
                    } else {
                        json!(text)
                    };
                    let summary = pretty(&output).chars().take(1200).collect();
                    return Ok((output, summary, None, result.assets));
                }
                Err(error) => {
                    if control.cancellation.is_cancelled() || error.code == "WORKFLOW_CANCELLED" {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| WorkflowError::invalid("工作流节点未执行。")))
    }
}
fn active_workflow(inner: &Inner, id: &str) -> bool {
    inner.starting.contains_key(id) || active_workflow_runs(inner, id)
}
fn active_workflow_runs(inner: &Inner, id: &str) -> bool {
    inner.state["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|run| {
            run["workflowId"] == id && inner.active.contains_key(run["id"].as_str().unwrap_or(""))
        })
}
fn find_workflow_mut<'a>(state: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    state["workflows"]
        .as_array_mut()?
        .iter_mut()
        .find(|w| w["id"] == id)
}
fn find_run_mut<'a>(state: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    state["runs"]
        .as_array_mut()?
        .iter_mut()
        .find(|r| r["id"] == id)
}
fn find_node_mut<'a>(run: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    run["nodes"]
        .as_array_mut()?
        .iter_mut()
        .find(|n| n["id"] == id)
}
fn increment_completed(run: &mut Value) {
    run["completedNodes"] = json!(run["completedNodes"].as_u64().unwrap_or(0) + 1);
}
fn merge_assets(run: &mut Value, assets: Vec<Value>) {
    if let Some(current) = run["assets"].as_array_mut() {
        for asset in assets {
            if !current.iter().any(|item| item["id"] == asset["id"]) {
                current.push(asset);
            }
        }
    }
}
fn reachable(start: &str, map: &HashMap<String, Vec<Value>>, endpoint: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let mut pending = vec![start.to_owned()];
    while let Some(id) = pending.pop() {
        for edge in map.get(&id).into_iter().flatten() {
            let next = model::text(&edge[endpoint], usize::MAX);
            if result.insert(next.clone()) {
                pending.push(next);
            }
        }
    }
    result
}
fn pretty(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
}
pub(crate) fn duration_label(duration: Duration) -> String {
    let seconds = duration.as_secs_f64().round() as u64;
    if seconds < 60 {
        format!("{seconds} 秒")
    } else {
        format!("{} 分 {} 秒", seconds / 60, seconds % 60)
    }
}
fn duration_since(stamp: &str) -> String {
    let millis = chrono::DateTime::parse_from_rfc3339(stamp)
        .map(|date| {
            (chrono::Utc::now() - date.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0) as u64
        })
        .unwrap_or(0);
    duration_label(Duration::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::{self, wait_run, Executor, TempDirectory};
    use axum::http::StatusCode;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Barrier;

    #[tokio::test(start_paused = true)]
    async fn approval_timeout_is_real_and_blocks_descendants() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow = service.create(json!({"name":"timed approval","nodes":[{"id":"before","prompt":"before"},{"id":"approval","kind":"approval","approval":{"timeoutMinutes":1}},{"id":"after","prompt":"after"}],"edges":[{"source":"before","target":"approval"},{"source":"approval","target":"after"}]})).await.unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        for _ in 0..100 {
            if service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"]
                == "waiting_approval"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"],
            "waiting_approval"
        );
        tokio::time::advance(Duration::from_secs(61)).await;
        for _ in 0..100 {
            if service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"] == "failed" {
                break;
            }
            tokio::task::yield_now().await;
        }
        let failed = service.get_run(run["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["nodes"][1]["status"], "failed");
        assert_eq!(failed["nodes"][2]["status"], "skipped");
        assert_eq!(executor.prompts.lock().unwrap().len(), 1);
        assert!(service
            .approval(run["id"].as_str().unwrap(), "approval", true, "late".into())
            .await
            .unwrap()
            .is_none());
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn cancelling_image_node_keeps_valid_paid_partial_output() {
        let directory = TempDirectory::new();
        let entered = Arc::new(Notify::new());
        let entered_fixture = entered.clone();
        let output = json!({"type":"workflow-images","version":1,"frames":[{"media":{"id":"paid","name":"paid.png","mimeType":"image/png","size":24},"width":1,"height":1,"action":"idle","direction":"S","columns":4,"rows":1,"frameCount":4,"durationMs":125}]});
        let partial = output.clone();
        let executor = Arc::new(Executor {
            image_fixture: Some(Arc::new(move |request| {
                let entered = entered_fixture.clone();
                let output = output.clone();
                Box::pin(async move {
                    entered.notify_one();
                    request.cancellation.cancelled().await;
                    Err(WorkflowError::coded(
                        "workflow_image_cancelled",
                        "workflow_image_cancelled",
                    )
                    .with_partial_output(output))
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor).await;
        let workflow=service.create(json!({"name":"paid cancellation","nodes":[{"id":"image","kind":"media-generate","image":{"directions":["S","W"]}}],"edges":[]})).await.unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        entered.notified().await;
        service.stop(run["id"].as_str().unwrap()).await.unwrap();
        let cancelled = wait_run(&service, run["id"].as_str().unwrap(), "cancelled").await;
        assert_eq!(cancelled["nodes"][0]["output"], partial);
        assert_eq!(cancelled["nodes"][0]["attempts"], 1);
        service.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn release_order_session_inheritance_and_revision_persist() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow=service.create(json!({"name":"发布检查","status":"published","notifications":["browser","feishu"],"nodes":[{"id":"b","kind":"prompt","prompt":"第二步","executionMode":"workspace-write"},{"id":"trigger","kind":"trigger"},{"id":"a","kind":"prompt","prompt":"第一步"},{"id":"notify","kind":"notification"}],"edges":[{"source":"trigger","target":"a"},{"source":"a","target":"b"},{"source":"b","target":"notify"}]})).await.unwrap();
        let run = service
            .run(
                workflow["id"].as_str().unwrap(),
                json!({"sourceSessionId":"source"}),
            )
            .await
            .unwrap()
            .unwrap();
        let completed = wait_run(&service, run["id"].as_str().unwrap(), "completed").await;
        assert_eq!(completed["completedNodes"], 4);
        assert_eq!(completed["workflowRevision"], 1.0);
        assert_eq!(completed["summary"], "fixture output");
        let prompts = executor.prompts.lock().unwrap().clone();
        assert!(prompts[0]["message"].as_str().unwrap().contains("第一步"));
        assert!(prompts[1]["message"].as_str().unwrap().contains("第二步"));
        assert!(prompts[1]["message"]
            .as_str()
            .unwrap()
            .contains("fixture output"));
        assert_eq!(prompts[1]["sessionId"], completed["sessionId"]);
        assert_eq!(prompts[0]["executionMode"], "full-access");
        assert_eq!(prompts[1]["executionMode"], "workspace-write");
        assert_eq!(prompts[0]["isolatedContext"], true);
        service.dispose().await.unwrap();
        let restored = WorkflowService::open(
            directory.path.join("workflows.json"),
            directory.path.to_string_lossy().into_owned(),
            executor,
            4,
        )
        .await
        .unwrap();
        assert_eq!(
            restored.state(Some("source")).await["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            restored.state(Some("other")).await["runs"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let updated = restored
            .update(
                workflow["id"].as_str().unwrap(),
                json!({"description":"第二版"}),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated["revision"], 2.0);
        assert_eq!(
            restored.get_run(run["id"].as_str().unwrap()).await.unwrap()["workflowRevision"],
            1.0
        );
        let duplicate = restored
            .duplicate(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(duplicate["status"], "draft");
        assert_eq!(duplicate["revision"], 1.0);
        let duplicate_ids = duplicate["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect::<HashSet<_>>();
        assert!(duplicate["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |edge| duplicate_ids.contains(edge["source"].as_str().unwrap())
                    && duplicate_ids.contains(edge["target"].as_str().unwrap())
            ));
        restored.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn json_condition_skips_only_unselected_branch_and_renders_notification() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(|request| {
                Box::pin(async move {
                    (request.on_session)("json-session".into()).await;
                    Ok(AgentResult {
                        text: "{\"accepted\":true}".into(),
                        session_id: "json-session".into(),
                        assets: vec![],
                    })
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow=service.create(json!({"name":"条件流程","status":"published","inputs":[{"name":"approved","type":"boolean","required":true}],"nodes":[{"id":"trigger","kind":"trigger"},{"id":"condition","kind":"condition","condition":{"source":"inputs.approved","operator":"equals","value":true}},{"id":"accepted","kind":"prompt","prompt":"批准分支","outputFormat":"json"},{"id":"rejected","kind":"prompt","prompt":"拒绝分支"},{"id":"notify","kind":"notification","notification":{"title":"{{workflow.name}}","content":"批准：{{nodes.accepted.output.accepted}}"},"notificationTargets":["browser"]}],"edges":[{"source":"trigger","target":"condition"},{"source":"condition","sourcePort":"true","target":"accepted"},{"source":"condition","sourcePort":"false","target":"rejected"},{"source":"accepted","target":"notify"}]})).await.unwrap();
        let run = service
            .run(
                workflow["id"].as_str().unwrap(),
                json!({"inputs":{"approved":true}}),
            )
            .await
            .unwrap()
            .unwrap();
        let completed = wait_run(&service, run["id"].as_str().unwrap(), "completed").await;
        let nodes = completed["nodes"].as_array().unwrap();
        assert_eq!(
            nodes.iter().find(|n| n["id"] == "condition").unwrap()["selectedPort"],
            "true"
        );
        assert_eq!(
            nodes.iter().find(|n| n["id"] == "accepted").unwrap()["output"],
            json!({"accepted":true})
        );
        assert_eq!(
            nodes.iter().find(|n| n["id"] == "rejected").unwrap()["skipReason"],
            "branch_not_selected"
        );
        assert_eq!(executor.prompts.lock().unwrap().len(), 1);
        assert_eq!(
            executor.notifications.lock().unwrap()[0]["options"]["content"],
            "批准：true"
        );
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn graph_branches_start_concurrently_and_join_waits_for_both() {
        let directory = TempDirectory::new();
        let barrier = Arc::new(Barrier::new(3));
        let releases = Arc::new(tokio::sync::Semaphore::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let fixture_barrier = barrier.clone();
        let fixture_releases = releases.clone();
        let fixture_started = started.clone();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(move |request| {
                let barrier = fixture_barrier.clone();
                let releases = fixture_releases.clone();
                let started = fixture_started.clone();
                Box::pin(async move {
                    let id = store::id()?;
                    (request.on_session)(id.clone()).await;
                    started.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    let permit = releases.acquire().await.map_err(WorkflowError::io)?;
                    permit.forget();
                    Ok(AgentResult {
                        text: "branch".into(),
                        session_id: id,
                        assets: vec![],
                    })
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor).await;
        let workflow=service.create(json!({"name":"parallel","nodes":[{"id":"trigger","kind":"trigger"},{"id":"parallel","kind":"parallel"},{"id":"a","kind":"prompt","prompt":"分支A"},{"id":"b","kind":"prompt","prompt":"分支B"},{"id":"join","kind":"notification"}],"edges":[{"source":"trigger","target":"parallel"},{"source":"parallel","target":"a"},{"source":"parallel","target":"b"},{"source":"a","target":"join"},{"source":"b","target":"join"}]})).await.unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), barrier.wait())
            .await
            .unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 2);
        releases.add_permits(1);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let current = service.get_run(run["id"].as_str().unwrap()).await.unwrap();
                let complete = current["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|n| {
                        ["a", "b"].contains(&n["id"].as_str().unwrap_or(""))
                            && n["status"] == "completed"
                    })
                    .count();
                if complete == 1 {
                    assert_eq!(
                        current["nodes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|n| n["id"] == "join")
                            .unwrap()["status"],
                        "pending"
                    );
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        releases.add_permits(1);
        wait_run(&service, run["id"].as_str().unwrap(), "completed").await;
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn approvals_can_approve_reject_and_cancel_without_duplicate_decisions() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow=service.create(json!({"name":"approval","nodes":[{"id":"approval","kind":"approval","approval":{"message":"确认发布？","timeoutMinutes":1}},{"id":"publish","kind":"prompt","prompt":"执行发布"}]})).await.unwrap();
        let id = workflow["id"].as_str().unwrap();
        let first = service.run(id, json!({})).await.unwrap().unwrap();
        wait_run(&service, first["id"].as_str().unwrap(), "waiting_approval").await;
        assert!(executor.prompts.lock().unwrap().is_empty());
        service
            .approval(
                first["id"].as_str().unwrap(),
                "approval",
                true,
                "可以发布".into(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(service
            .approval(
                first["id"].as_str().unwrap(),
                "approval",
                true,
                String::new()
            )
            .await
            .unwrap()
            .is_none());
        let completed = wait_run(&service, first["id"].as_str().unwrap(), "completed").await;
        assert_eq!(
            completed["nodes"][0]["output"],
            json!({"approved":true,"comment":"可以发布"})
        );
        let rejected = service.run(id, json!({})).await.unwrap().unwrap();
        wait_run(
            &service,
            rejected["id"].as_str().unwrap(),
            "waiting_approval",
        )
        .await;
        service
            .approval(
                rejected["id"].as_str().unwrap(),
                "approval",
                false,
                "不同意".into(),
            )
            .await
            .unwrap();
        let failed = wait_run(&service, rejected["id"].as_str().unwrap(), "failed").await;
        assert_eq!(failed["error"], "不同意");
        assert_eq!(failed["nodes"][1]["status"], "skipped");
        assert_eq!(executor.prompts.lock().unwrap().len(), 1);
        let cancelled = service.run(id, json!({})).await.unwrap().unwrap();
        wait_run(
            &service,
            cancelled["id"].as_str().unwrap(),
            "waiting_approval",
        )
        .await;
        service
            .stop(cancelled["id"].as_str().unwrap())
            .await
            .unwrap();
        wait_run(&service, cancelled["id"].as_str().unwrap(), "cancelled").await;
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn retries_and_skip_policy_preserve_later_execution_and_real_attempt_count() {
        let directory = TempDirectory::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let fixture_calls = calls.clone();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(move |request| {
                let calls = fixture_calls.clone();
                Box::pin(async move {
                    (request.on_session)("retry-session".into()).await;
                    if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                        return Err(WorkflowError::invalid("暂时失败"));
                    }
                    Ok(AgentResult {
                        text: "后续节点完成".into(),
                        session_id: "retry-session".into(),
                        assets: vec![],
                    })
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor).await;
        let workflow=service.create(json!({"name":"skip","nodes":[{"id":"unstable","kind":"prompt","prompt":"执行","retries":1,"failurePolicy":"skip"},{"id":"next","kind":"prompt","prompt":"继续"}]})).await.unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        let done = wait_run(&service, run["id"].as_str().unwrap(), "completed").await;
        assert_eq!(done["nodes"][0]["attempts"], 2);
        assert_eq!(done["nodes"][0]["skipReason"], "failure_policy");
        assert_eq!(done["nodes"][1]["status"], "completed");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn running_workflow_prevents_mutation_and_stop_aborts_the_owned_session() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(|request| {
                Box::pin(async move {
                    (request.on_session)("active-session".into()).await;
                    request.cancellation.cancelled().await;
                    Err(WorkflowError::cancelled())
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow = service
            .create(json!({"name":"long","nodes":[{"id":"wait","prompt":"等待"}]}))
            .await
            .unwrap();
        let id = workflow["id"].as_str().unwrap();
        let run = service.run(id, json!({})).await.unwrap().unwrap();
        let run_id = run["id"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while service.get_run(run_id).await.unwrap()["sessionId"] != "active-session" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            service.run(id, json!({})).await.unwrap_err().status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            service
                .update(id, json!({"description":"changed"}))
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            service.remove(id).await.unwrap_err().status,
            StatusCode::CONFLICT
        );
        service.stop(run_id).await.unwrap();
        wait_run(&service, run_id, "cancelled").await;
        assert_eq!(*executor.aborted.lock().unwrap(), vec!["active-session"]);
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn cooperative_cancel_keeps_future_and_admission_until_actual_finally() {
        let directory = TempDirectory::new();
        let lifecycle = test_support::PromptLifecycle::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(lifecycle.fixture()),
            ..Executor::default()
        });
        let service = WorkflowService::open(
            directory.path.join("workflows.json"),
            directory.path.to_string_lossy().into_owned(),
            executor,
            1,
        )
        .await
        .unwrap();
        let workflow = service
            .create(json!({"name":"cancel owner","nodes":[{"id":"owned","prompt":"run"}]}))
            .await
            .unwrap();
        let other = service
            .create(json!({"name":"other","nodes":[{"id":"owned","prompt":"run"}]}))
            .await
            .unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), lifecycle.started.notified())
            .await
            .unwrap();
        service.stop(run["id"].as_str().unwrap()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), lifecycle.cancelling.notified())
            .await
            .unwrap();
        assert!(!lifecycle.dropped.load(Ordering::SeqCst));
        assert!(!lifecycle.finally_finished.load(Ordering::SeqCst));
        assert_eq!(
            service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"],
            "running"
        );
        assert_eq!(
            service
                .run(other["id"].as_str().unwrap(), json!({}))
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            service
                .remove(workflow["id"].as_str().unwrap())
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        lifecycle.finish.add_permits(1);
        wait_run(&service, run["id"].as_str().unwrap(), "cancelled").await;
        assert!(lifecycle.finally_finished.load(Ordering::SeqCst));
        assert!(lifecycle.dropped.load(Ordering::SeqCst));
        assert!(!lifecycle.dropped_before_finally.load(Ordering::SeqCst));
        service.dispose().await.unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn cooperative_timeout_keeps_future_and_admission_until_actual_finally() {
        let directory = TempDirectory::new();
        let lifecycle = test_support::PromptLifecycle::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(lifecycle.fixture()),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow = service.create(json!({"name":"timeout owner","nodes":[{"id":"owned","prompt":"run","timeoutMinutes":1}]})).await.unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        lifecycle.started.notified().await;
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::timeout(Duration::from_secs(3), lifecycle.cancelling.notified())
            .await
            .unwrap();
        assert!(!lifecycle.dropped.load(Ordering::SeqCst));
        assert_eq!(
            service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"],
            "running"
        );
        assert_eq!(
            service
                .run(workflow["id"].as_str().unwrap(), json!({}))
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );
        lifecycle.finish.add_permits(1);
        let failed = wait_run(&service, run["id"].as_str().unwrap(), "failed").await;
        assert!(failed["nodes"][0]["error"]
            .as_str()
            .unwrap()
            .contains("执行超过"));
        assert!(lifecycle.finally_finished.load(Ordering::SeqCst));
        assert!(lifecycle.dropped.load(Ordering::SeqCst));
        assert!(!lifecycle.dropped_before_finally.load(Ordering::SeqCst));
        assert_eq!(
            *executor.aborted.lock().unwrap(),
            vec!["cooperative-session"]
        );
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn cooperative_cancel_before_session_publication_prevents_late_native_start() {
        let directory = TempDirectory::new();
        let preparing = Arc::new(Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let native_starts = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new({
                let preparing = preparing.clone();
                let release = release.clone();
                let native_starts = native_starts.clone();
                move |request| {
                    let preparing = preparing.clone();
                    let release = release.clone();
                    let native_starts = native_starts.clone();
                    Box::pin(async move {
                        preparing.notify_one();
                        let _release = release.acquire().await.unwrap();
                        (request.on_session)("late-session".into()).await;
                        if request.cancellation.is_cancelled() {
                            return Err(WorkflowError::cancelled());
                        }
                        native_starts.fetch_add(1, Ordering::SeqCst);
                        Ok(AgentResult::default())
                    })
                }
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor.clone()).await;
        let workflow = service
            .create(json!({"name":"preparing","nodes":[{"id":"prepare","prompt":"run"}]}))
            .await
            .unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), preparing.notified())
            .await
            .unwrap();
        service.stop(run["id"].as_str().unwrap()).await.unwrap();
        assert_eq!(
            service.get_run(run["id"].as_str().unwrap()).await.unwrap()["status"],
            "running"
        );
        release.add_permits(1);
        wait_run(&service, run["id"].as_str().unwrap(), "cancelled").await;
        assert_eq!(native_starts.load(Ordering::SeqCst), 0);
        assert!(executor.aborted.lock().unwrap().is_empty());
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn restart_marks_persisted_running_and_approvals_interrupted_and_retry_retains_inputs() {
        let directory = TempDirectory::new();
        let workflow=model::normalize(&json!({"id":"workflow","name":"restart","lastStatus":"waiting_approval","nodes":[{"id":"node","prompt":"run"}]}),".",true).unwrap();
        store::save_json(&directory.path.join("workflows.json"),&json!({"version":2,"workflows":[workflow],"runs":[{"id":"old-run","workflowId":"workflow","status":"waiting_approval","inputs":{"task":"old input"},"sourceSessionId":"origin","sourceMessage":"request","nodes":[]}]})).unwrap();
        let service = test_support::service(&directory, Arc::new(Executor::default())).await;
        let interrupted = service.get_run("old-run").await.unwrap();
        assert_eq!(interrupted["status"], "interrupted");
        assert!(interrupted["finishedAt"].is_string());
        assert_eq!(
            service.get_workflow("workflow").await.unwrap()["lastStatus"],
            "interrupted"
        );
        let retry = service.retry("old-run").await.unwrap().unwrap();
        assert_eq!(retry["retryOf"], "old-run");
        assert_eq!(retry["inputs"], json!({"task":"old input"}));
        assert_eq!(retry["sourceSessionId"], "origin");
        wait_run(&service, retry["id"].as_str().unwrap(), "completed").await;
        service.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn failed_create_rolls_back_and_execution_persistence_failure_never_reports_success() {
        let directory = TempDirectory::new();
        let path = directory.path.join("workflows.json");
        let original = directory.path.join("saved.json");
        let fixture_path = path.clone();
        let fixture_original = original.clone();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(move |request| {
                let path = fixture_path.clone();
                let original = fixture_original.clone();
                Box::pin(async move {
                    (request.on_session)("storage-session".into()).await;
                    std::fs::rename(&path, &original).unwrap();
                    std::fs::create_dir(&path).unwrap();
                    Ok(AgentResult {
                        text: "must not report success".into(),
                        session_id: "storage-session".into(),
                        assets: vec![],
                    })
                })
            })),
            ..Executor::default()
        });
        let service = test_support::service(&directory, executor).await;
        let workflow = service
            .create(json!({"name":"storage","nodes":[{"id":"task","prompt":"run"}]}))
            .await
            .unwrap();
        std::fs::rename(&path, &original).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            service
                .create(json!({"name":"failed","nodes":[]}))
                .await
                .unwrap_err()
                .code,
            "workflow_storage_error"
        );
        assert_eq!(service.list().await.len(), 1);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&original, &path).unwrap();
        let run = service
            .run(workflow["id"].as_str().unwrap(), json!({}))
            .await
            .unwrap()
            .unwrap();
        let failed = wait_run(&service, run["id"].as_str().unwrap(), "failed").await;
        assert_eq!(failed["errorCode"], "workflow_storage_error");
        assert_ne!(failed["summary"], "must not report success");
        assert_eq!(
            service.state(None).await["persistenceError"]["code"],
            "workflow_storage_error"
        );
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&original, &path).unwrap();
        service.dispose().await.unwrap();
    }
}
