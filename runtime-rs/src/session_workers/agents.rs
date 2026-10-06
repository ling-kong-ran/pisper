//! 独立子会话的真实执行、四个并发槽、持久邮箱与上下文保留。
use super::{
    persistence::{now, read, write},
    ChildRequest, EventSink, InputKind, PromptRequest, RunOutcome, SessionExecutor, SessionScope,
};
use crate::goal_api::GoalService;
use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{watch, Notify};
pub(crate) const MAX_CONCURRENT: usize = 4;
pub(crate) const MAX_PER_PARENT: usize = 64;
pub(crate) const MAX_RECORDS: usize = 256;
pub(crate) const COMPLETION_MARKER: &str = "[Pisper internal agent completion]";
pub(crate) const CHILD_FORBIDDEN: &[&str] = &[
    "spawn_agent",
    "list_agents",
    "send_message",
    "followup_task",
    "wait_agent",
    "interrupt_agent",
    "update_team_task",
    "run_team_workflow",
    "get_goal",
    "update_goal",
    "update_plan",
    "update_task_list",
    "browser_automation",
    "mcp_list",
    "mcp_manage",
];
const SUBAGENT_PROMPT:&str="You are a Pisper subagent working in an isolated context on one delegated task. Complete only the concrete task and return a concise, evidence-based result. Inspect relevant files first. Respect the parent tools, permissions, workspace and declared file ownership. Do not duplicate unrelated work. You cannot spawn other agents. In Team mode use the restricted team communication tools for handoffs. Respond in the delegated task's language.";
type Completion = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<()>> + Send + Sync>;
/// Team 图拥有任务/依赖/租约/完成证据；执行注册表只拥有真正运行的子会话。
pub(crate) trait TaskGraph: Send + Sync {
    fn register(&self, parent: &SessionScope, input: &Value) -> Result<String>;
    fn ready(&self, parent: &str, task: &str) -> Result<bool>;
    fn ready_agent(&self, parent: &str, task: &str, _agent: &str) -> Result<bool> {
        self.ready(parent, task)
    }
    fn bind(&self, parent: &str, task: &str, agent: &Value) -> Result<()>;
    fn update_agent(&self, parent: &str, agent: &Value) -> Result<()>;
    fn update_task(&self, parent: &str, target: &str, input: &Value) -> Result<Value>;
    fn projection(&self, parent: &str) -> Option<Value>;
    fn communication(&self, parent: &str, sender: &str, target: &str, text: &str) -> Result<()>;
    fn registration_failed(&self, parent: &str, task: &str, reason: &str) -> Result<()>;
}
pub(crate) trait WorkflowRunner: Send + Sync {
    fn run(&self, parent: String, path: String, args: Value) -> BoxFuture<'static, Result<Value>>;
}
#[derive(Clone, Debug)]
pub(crate) struct CompletionBatch {
    pub(crate) session_id: String,
    pub(crate) entries: Vec<Value>,
    pub(crate) prompt: String,
    /// Suspension cancels preflight and a running native prompt cooperatively.
    pub(crate) cancellation: tokio_util::sync::CancellationToken,
}
/// true 仅在真实父会话已接受 steer/follow-up 或完成内部 prompt 后返回。
pub(crate) type CompletionDispatcher =
    Arc<dyn Fn(CompletionBatch) -> BoxFuture<'static, Result<bool>> + Send + Sync>;
struct NotificationTask {
    wake: watch::Sender<()>,
    finished: watch::Receiver<bool>,
    cancellation: tokio_util::sync::CancellationToken,
}
#[derive(Clone)]
struct Record {
    value: Value,
    spec: Option<ChildRequest>,
    child: Option<String>,
    generation: u64,
    slot: bool,
    cancel: Option<watch::Sender<bool>>,
    pending: Vec<(InputKind, String)>,
    goal_id: String,
    task_id: String,
    last_progress_ms: u64,
}
#[derive(Clone, Default)]
struct Store {
    sequence: u64,
    records: BTreeMap<String, Record>,
    mailbox: BTreeMap<String, Value>,
    closed: bool,
}
pub(crate) struct AgentService {
    path: PathBuf,
    store: Mutex<Store>,
    executor: Arc<dyn SessionExecutor>,
    goals: Arc<GoalService>,
    events: EventSink,
    graph: Mutex<Option<Arc<dyn TaskGraph>>>,
    completion: Mutex<Option<Completion>>,
    dispatcher: Mutex<Option<CompletionDispatcher>>,
    notification_tasks: Mutex<BTreeMap<String, NotificationTask>>,
    notification_suspended: Mutex<HashSet<String>>,
    workflow: Mutex<Option<Arc<dyn WorkflowRunner>>>,
    changed: Notify,
    retention: Duration,
}
fn active(record: &Value) -> bool {
    matches!(
        record["status"].as_str(),
        Some("queued" | "starting" | "running")
    )
}
fn graph_task(graph: &dyn TaskGraph, parent: &str, target: &str) -> Option<Value> {
    graph.projection(parent)?["tasks"]
        .as_array()?
        .iter()
        .find(|task| task["id"] == target || task["taskName"] == target)
        .cloned()
}
fn number(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}
fn empty_usage() -> Value {
    json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":0})
}
fn add_usage(previous: &Value, next: &Value) -> Value {
    let mut total = empty_usage();
    for key in [
        "input",
        "output",
        "cacheRead",
        "cacheWrite",
        "reasoning",
        "totalTokens",
    ] {
        total[key] = json!(number(&previous[key]).saturating_add(number(&next[key])));
    }
    total
}
pub(crate) fn task_name(value: &str) -> String {
    let mut name = String::new();
    for c in value.trim().to_ascii_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
            c
        } else {
            '_'
        };
        if c != '_' || !name.ends_with('_') {
            name.push(c);
        }
    }
    name.trim_matches('_').chars().take(48).collect::<String>()
}
fn message(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("Agent task cannot be empty.")
    }
    if value.encode_utf16().count() > 12_000 {
        bail!("Agent task is limited to 12000 characters.")
    }
    Ok(value.into())
}
fn bounded_output(value: &str) -> (String, bool) {
    const LIMIT: usize = 50 * 1024;
    if value.len() <= LIMIT {
        return (value.into(), false);
    }
    let suffix = "\n\n[Output truncated to the Pisper tool-output limit.]";
    let mut end = LIMIT - suffix.len();
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{suffix}", &value[..end]), true)
}
fn durable(store: &Store) -> Value {
    let records = store
        .records
        .values()
        .map(|r| {
            let mut value = r.value.clone();
            if let Some(object) = value.as_object_mut() {
                object.remove("fullOutput");
            }
            value
        })
        .collect::<Vec<_>>();
    let mailbox = store
        .mailbox
        .values()
        .map(|r| {
            let mut value = r.clone();
            if let Some(object) = value.as_object_mut() {
                object.remove("fullOutput");
            }
            value
        })
        .collect::<Vec<_>>();
    json!({"version":4,"sequence":store.sequence,"records":records,"mailbox":mailbox})
}
fn enqueue_mailbox(store: &mut Store, record: &Value) {
    let id = format!(
        "{}:{}",
        record["id"].as_str().unwrap_or_default(),
        number(&record["resultVersion"])
    );
    let mut value = record.clone();
    value["mailboxId"] = json!(id);
    value["queuedAt"] = json!(now());
    store.mailbox.entry(id).or_insert(value);
}
impl AgentService {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        executor: Arc<dyn SessionExecutor>,
        goals: Arc<GoalService>,
        events: EventSink,
    ) -> Result<Arc<Self>> {
        Self::with_retention(path, executor, goals, events, Duration::from_secs(600))
    }
    pub(crate) fn with_retention(
        path: impl Into<PathBuf>,
        executor: Arc<dyn SessionExecutor>,
        goals: Arc<GoalService>,
        events: EventSink,
        retention: Duration,
    ) -> Result<Arc<Self>> {
        let path = path.into();
        let document = read(&path)?;
        let mut store = Store::default();
        let mut changed = false;
        if let Some(document) = document {
            if document["version"] == 4 {
                store.sequence = number(&document["sequence"]);
                for value in document["mailbox"].as_array().into_iter().flatten() {
                    if let Some(id) = value["mailboxId"].as_str() {
                        if !id.is_empty()
                            && !value["parentSessionId"]
                                .as_str()
                                .unwrap_or_default()
                                .is_empty()
                        {
                            store.mailbox.insert(id.into(), value.clone());
                        }
                    }
                }
                for value in document["records"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(MAX_RECORDS)
                {
                    let Some(id) = value["id"].as_str().filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    if value["parentSessionId"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty()
                        || !value["model"].as_str().is_some_and(|s| s.contains('/'))
                    {
                        continue;
                    }
                    let mut value = value.clone();
                    value["fullOutput"] = value["output"].clone();
                    if active(&value) {
                        let at = now();
                        value["status"] = json!("interrupted");
                        value["error"] = json!("Agent was interrupted because Pisper restarted.");
                        value["completedAt"] = json!(at);
                        value["lastActivityAt"] = json!(at);
                        value["currentActivity"] = Value::Null;
                        for tool in value["tools"].as_array_mut().into_iter().flatten() {
                            if tool["status"] == "running" {
                                tool["status"] = json!("error");
                                tool["message"] =
                                    json!("Agent was interrupted because Pisper restarted.");
                            }
                        }
                        value["resultVersion"] = json!(number(&value["resultVersion"]) + 1);
                        enqueue_mailbox(&mut store, &value);
                        changed = true;
                    }
                    store.records.insert(
                        id.into(),
                        Record {
                            value,
                            spec: None,
                            child: None,
                            generation: 0,
                            slot: false,
                            cancel: None,
                            pending: vec![],
                            goal_id: String::new(),
                            task_id: String::new(),
                            last_progress_ms: 0,
                        },
                    );
                }
            } else {
                changed = true;
            }
        }
        if changed {
            write(&path, &durable(&store))?;
        }
        Ok(Arc::new(Self {
            path,
            store: Mutex::new(store),
            executor,
            goals,
            events,
            graph: Mutex::new(None),
            completion: Mutex::new(None),
            dispatcher: Mutex::new(None),
            notification_tasks: Mutex::new(BTreeMap::new()),
            notification_suspended: Mutex::new(HashSet::new()),
            workflow: Mutex::new(None),
            changed: Notify::new(),
            retention,
        }))
    }
    pub(crate) fn set_graph(&self, graph: Arc<dyn TaskGraph>) {
        *self.graph.lock().expect("agent graph") = Some(graph);
    }
    pub(crate) fn set_completion_notifier(&self, callback: Completion) {
        *self.completion.lock().expect("agent completion") = Some(callback);
    }
    pub(crate) fn set_completion_dispatcher(&self, callback: CompletionDispatcher) {
        *self.dispatcher.lock().expect("completion dispatcher") = Some(callback);
    }
    pub(crate) fn set_workflow(&self, workflow: Arc<dyn WorkflowRunner>) {
        *self.workflow.lock().expect("Team workflow runner") = Some(workflow);
    }
    pub(crate) fn run_workflow(
        &self,
        parent: String,
        path: String,
        args: Value,
    ) -> BoxFuture<'static, Result<Value>> {
        let workflow = self.workflow.lock().expect("Team workflow runner").clone();
        Box::pin(async move {
            workflow
                .ok_or_else(|| anyhow!("Team workflow executor has not been installed."))?
                .run(parent, path, args)
                .await
        })
    }
    pub(crate) fn resume_notifications(self: &Arc<Self>, parent: &str) {
        self.notification_suspended
            .lock()
            .expect("completion suspension")
            .remove(parent);
        self.schedule_notification(parent);
    }
    pub(crate) fn suspend_notifications(&self, parent: &str) {
        self.notification_suspended
            .lock()
            .expect("completion suspension")
            .insert(parent.into());
        if let Some(task) = self
            .notification_tasks
            .lock()
            .expect("completion tasks")
            .get(parent)
        {
            // 已进入真实 prompt 的 callback 必须继续 poll 原生 finally，不能 drop。
            task.cancellation.cancel();
            let _ = task.wake.send(());
        }
    }
    fn schedule_notification(self: &Arc<Self>, parent: &str) {
        if self
            .notification_suspended
            .lock()
            .expect("completion suspension")
            .contains(parent)
            || self
                .dispatcher
                .lock()
                .expect("completion dispatcher")
                .is_none()
        {
            return;
        }
        let mut tasks = self.notification_tasks.lock().expect("completion tasks");
        if tasks.contains_key(parent) {
            return;
        }
        let weak = Arc::downgrade(self);
        let parent = parent.to_string();
        let own_parent = parent.clone();
        let (wake, mut waking) = watch::channel(());
        let (completed, finished) = watch::channel(false);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        // Tokio 不会在 spawn 调用期间同步 poll；先登记 handle 才可能执行 worker。
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {},
                    _ = waking.changed() => {},
                }
                let Some(service) = weak.upgrade() else {
                    break;
                };
                if service.store.lock().expect("agent registry").closed
                    || worker_cancellation.is_cancelled()
                    || service
                        .notification_suspended
                        .lock()
                        .expect("completion suspension")
                        .contains(&own_parent)
                {
                    break;
                }
                let entries = service.mailbox(&own_parent);
                if entries.is_empty() {
                    break;
                }
                let callback = service
                    .dispatcher
                    .lock()
                    .expect("completion dispatcher")
                    .clone();
                let Some(callback) = callback else { break };
                let prompt = entries
                    .iter()
                    .map(completion_prompt)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let batch = CompletionBatch {
                    session_id: own_parent.clone(),
                    entries: entries.clone(),
                    prompt,
                    cancellation: worker_cancellation.clone(),
                };
                match callback(batch).await {
                    Ok(true) => {
                        if let Err(error) = service.acknowledge(&own_parent, &entries) {
                            (service.events)(
                                "agent_notification_error",
                                &json!({"sessionId":own_parent,"message":error.to_string()}),
                            );
                        }
                    }
                    Ok(false) => {}
                    Err(error) => (service.events)(
                        "agent_notification_error",
                        &json!({"sessionId":own_parent,"message":error.to_string()}),
                    ),
                }
            }
            if let Some(service) = weak.upgrade() {
                service
                    .notification_tasks
                    .lock()
                    .expect("completion tasks")
                    .remove(&own_parent);
                // 最后一次空快照与新结果并发时，再检查一次持久邮箱，避免漏掉唤醒。
                if !service.mailbox(&own_parent).is_empty()
                    && !service.store.lock().expect("agent registry").closed
                {
                    service.schedule_notification(&own_parent);
                }
            }
            let _ = completed.send(true);
        });
        tasks.insert(
            parent,
            NotificationTask {
                wake,
                finished,
                cancellation,
            },
        );
    }
    fn graph(&self) -> Option<Arc<dyn TaskGraph>> {
        self.graph.lock().expect("agent graph").clone()
    }
    fn commit<T>(&self, mutate: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut store = self.store.lock().expect("agent registry");
        let mut next = store.clone();
        let result = mutate(&mut next)?;
        write(&self.path, &durable(&next))?;
        *store = next;
        drop(store);
        self.changed.notify_waiters();
        Ok(result)
    }
    pub(crate) fn list(&self, parent: &str) -> Vec<Value> {
        let store = self.store.lock().expect("agent registry");
        let mut records = store
            .records
            .values()
            .filter(|r| parent.is_empty() || r.value["parentSessionId"] == parent)
            .map(|r| r.value.clone())
            .collect::<Vec<_>>();
        records.sort_by_key(|r| r["startedAt"].as_str().unwrap_or_default().to_string());
        records
    }
    pub(crate) fn summaries(&self, parent: &str) -> Vec<Value> {
        self.list(parent)
            .into_iter()
            .map(|mut r| {
                for key in ["message", "output", "error"] {
                    r[key] = json!(r[key]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(500)
                        .collect::<String>());
                }
                if let Some(object) = r.as_object_mut() {
                    object.remove("fullOutput");
                    object.remove("availableTools");
                    object.remove("tools");
                    object.remove("usage");
                    object.remove("runUsage");
                }
                r
            })
            .collect()
    }
    pub(crate) fn has_active(&self, parent: &str) -> bool {
        self.list(parent).iter().any(active)
    }
    pub(crate) fn find(&self, parent: &str, target: &str) -> Option<Value> {
        self.list(parent).into_iter().rev().find(|r| {
            ["id", "taskName", "canonicalName"]
                .iter()
                .any(|key| r[*key] == target)
        })
    }
    fn emit(&self, agent: &Value) {
        let parent = agent["parentSessionId"].as_str().unwrap_or_default();
        let agents = self.summaries(parent);
        let updated = agents
            .iter()
            .find(|r| r["id"] == agent["id"])
            .cloned()
            .unwrap_or(Value::Null);
        let active = agents.into_iter().filter(active).collect::<Vec<_>>();
        let activity = json!({"type":"agent","agent":updated,"updatedAt":agent["lastActivityAt"]});
        (self.events)(
            "agent_update",
            &json!({"sessionId":parent,"agent":updated,"agents":active,"team":self.graph().and_then(|g|g.projection(parent)),"currentActivity":activity}),
        );
    }
    pub(crate) async fn spawn(self: &Arc<Self>, parent: String, mut input: Value) -> Result<Value> {
        let scope = self.executor.scope(parent.clone()).await?;
        if scope.parent_session_id.is_some() {
            bail!("Subagents cannot spawn other agents.")
        }
        if scope.cwd.is_empty() || !scope.model.contains('/') {
            bail!("Agent requires an active parent model and workspace.")
        }
        let text = message(input["message"].as_str().unwrap_or_default())?;
        let name = task_name(input["taskName"].as_str().unwrap_or("task"));
        let name = if name.is_empty() { "task".into() } else { name };
        self.prune(&parent).await?;
        let goal = self.goals.get(&parent);
        let is_team = goal
            .as_ref()
            .is_some_and(|g| g.mode == "team" && g.status == "active");
        let graph = if is_team {
            Some(
                self.graph()
                    .ok_or_else(|| anyhow!("Team task graph has not been installed."))?,
            )
        } else {
            None
        };
        if let Some(graph) = &graph {
            let target = input["teamTaskId"].as_str().filter(|s| !s.is_empty());
            let previous = graph_task(graph.as_ref(), &parent, target.unwrap_or(&name));
            if target.is_some()
                || previous.as_ref().is_some_and(|task| {
                    matches!(
                        task["status"].as_str(),
                        Some("queued" | "blocked" | "failed" | "interrupted")
                    )
                })
            {
                let previous = previous.ok_or_else(|| anyhow!("Unknown persisted Team task."))?;
                let task_id = previous["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Team task has no id."))?;
                graph.update_task(&parent, task_id, &input)?;
                if let Some(agent) = previous["agentId"].as_str().filter(|s| !s.is_empty()) {
                    if self
                        .find(&parent, agent)
                        .is_some_and(|agent| active(&agent))
                    {
                        self.interrupt(
                            &parent,
                            agent,
                            "Team task was replaced with updated inputs.",
                        )
                        .await?;
                    }
                }
                input["teamTaskId"] = json!(task_id);
            }
        }
        let task_id = graph
            .as_ref()
            .map(|g| g.register(&scope, &input))
            .transpose()?
            .unwrap_or_default();
        let mut unique = HashSet::new();
        let mut tools = scope
            .tool_names
            .iter()
            .filter(|name| {
                !CHILD_FORBIDDEN.contains(&name.as_str()) && unique.insert((*name).clone())
            })
            .cloned()
            .collect::<Vec<_>>();
        if is_team {
            tools.extend(["list_team_members".into(), "send_team_message".into()]);
        }
        let owned_files = input["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let id = crate::product::new_id();
        let at = now();
        let record_result=self.commit(|store|{
            if store.closed{bail!("Agent service is shutting down.")}let count=store.records.values().filter(|r|r.value["parentSessionId"]==parent).count();if count>=MAX_PER_PARENT{bail!("Agent record limit reached for this session (64).")}if store.records.len()>=MAX_RECORDS{bail!("Global Agent record limit reached (256).")}
            store.sequence+=1;let value=json!({"id":id,"taskName":name,"canonicalName":format!("/root/{name}_{}",store.sequence),"parentSessionId":parent,"teamTaskId":task_id,"cwd":scope.cwd,"status":"queued","message":text,"model":scope.model,"thinkingLevel":scope.thinking_level,"availableTools":tools,"startedAt":at,"lastActivityAt":at,"completedAt":null,"durationMs":null,"turnCount":0,"toolCallCount":0,"tools":[],"output":"","fullOutput":"","outputTruncated":false,"usage":empty_usage(),"runUsage":empty_usage(),"runNumber":0,"resultVersion":0,"error":"","currentActivity":{"type":"queue","updatedAt":at}});
            store.records.insert(id.clone(),Record{value:value.clone(),spec:Some(ChildRequest{id:id.clone(),parent:scope.clone(),system_prompt:SUBAGENT_PROMPT.into(),tools:tools.clone(),owned_files:owned_files.clone()}),child:None,generation:0,slot:false,cancel:None,pending:vec![],goal_id:goal.as_ref().map(|g|g.id.clone()).unwrap_or_default(),task_id:task_id.clone(),last_progress_ms:0});Ok(value)
        });
        let record = match record_result {
            Ok(record) => record,
            Err(error) => {
                if let Some(graph) = &graph {
                    graph.registration_failed(&parent, &task_id, &error.to_string())?;
                }
                return Err(error);
            }
        };
        if let Some(graph) = graph {
            graph.bind(&parent, &task_id, &record)?;
        }
        self.emit(&record);
        self.pump()?;
        Ok(self.find(&parent, &id).unwrap_or(record))
    }
    async fn prune(&self, parent: &str) -> Result<()> {
        let children = self.commit(|store| {
            let mut removable = store
                .records
                .iter()
                .filter(|(_, record)| !active(&record.value) && !record.slot)
                .map(|(id, record)| {
                    (
                        id.clone(),
                        record.value["startedAt"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>();
            removable.sort_by(|left, right| left.1.cmp(&right.1));
            let mut removed = vec![];
            let mut children = vec![];
            while store
                .records
                .values()
                .filter(|r| r.value["parentSessionId"] == parent)
                .count()
                >= MAX_PER_PARENT
            {
                let Some(index) = removable.iter().position(|(id, _)| {
                    store
                        .records
                        .get(id)
                        .is_some_and(|r| r.value["parentSessionId"] == parent)
                }) else {
                    break;
                };
                removed.push(removable.remove(index).0);
                if let Some(record) = store
                    .records
                    .remove(removed.last().expect("removed record"))
                {
                    children.extend(record.child);
                }
            }
            while store.records.len() >= MAX_RECORDS && !removable.is_empty() {
                let id = removable.remove(0).0;
                removed.push(id.clone());
                if let Some(record) = store.records.remove(&id) {
                    children.extend(record.child);
                }
            }
            // 会话句柄从旧状态取出后交给拥有它的 executor 释放。
            store
                .mailbox
                .retain(|_, entry| !removed.iter().any(|id| entry["id"] == *id));
            Ok(children)
        })?;
        // 终态计时器仍负责实际内存句柄释放；已裁剪 record 的句柄必须立即关闭。
        for id in children {
            self.executor.dispose(id).await?;
        }
        Ok(())
    }
    pub(crate) fn pump(self: &Arc<Self>) -> Result<()> {
        let graph = self.graph();
        let claimed = self.commit(|store| {
            if store.closed {
                return Ok(vec![]);
            }
            let mut slots =
                MAX_CONCURRENT.saturating_sub(store.records.values().filter(|r| r.slot).count());
            if slots == 0 {
                return Ok(vec![]);
            }
            let mut queued = store
                .records
                .iter()
                .filter(|(_, r)| r.value["status"] == "queued" && !r.slot)
                .map(|(id, r)| {
                    (
                        id.clone(),
                        r.value["startedAt"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>();
            queued.sort_by(|a, b| a.1.cmp(&b.1));
            let mut claimed = vec![];
            for (id, _) in queued {
                if slots == 0 {
                    break;
                }
                let r = store.records.get_mut(&id).expect("queued agent");
                let parent = r.value["parentSessionId"].as_str().unwrap_or_default();
                if !r.task_id.is_empty()
                    && !graph
                        .as_ref()
                        .ok_or_else(|| anyhow!("Team task graph unavailable"))?
                        .ready_agent(parent, &r.task_id, &id)?
                {
                    continue;
                }
                let (tx, rx) = watch::channel(false);
                r.cancel = Some(tx);
                r.generation += 1;
                r.slot = true;
                r.value["status"] = json!("starting");
                r.value["lastActivityAt"] = json!(now());
                r.value["currentActivity"] =
                    json!({"type":"model","stage":"starting","updatedAt":now()});
                claimed.push((id, r.generation, rx));
                slots -= 1;
            }
            Ok(claimed)
        })?;
        for (id, generation, cancel) in claimed {
            let service = self.clone();
            tokio::spawn(async move {
                service.run(id, generation, cancel).await;
            });
        }
        Ok(())
    }
    fn snapshot(&self, id: &str, generation: u64) -> Result<Record> {
        self.store
            .lock()
            .expect("agent registry")
            .records
            .get(id)
            .filter(|r| r.generation == generation)
            .cloned()
            .ok_or_else(|| anyhow!("Stale Agent run."))
    }
    fn run(
        self: Arc<Self>,
        id: String,
        generation: u64,
        mut cancel: watch::Receiver<bool>,
    ) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let started = Instant::now();
            let result = self.execute(&id, generation, &mut cancel).await;
            if let Err(error) = self
                .finish(&id, generation, result, started.elapsed())
                .await
            {
                (self.events)("agent_error", &json!({"id":id,"message":error.to_string()}));
            }
            let _ = self.commit(|store| {
                if let Some(r) = store
                    .records
                    .get_mut(&id)
                    .filter(|r| r.generation == generation)
                {
                    r.slot = false;
                    r.cancel = None;
                }
                Ok(())
            });
            self.schedule_release(id, generation);
            if let Err(error) = self.pump() {
                (self.events)("agent_error", &json!({"message":error.to_string()}));
            }
        })
    }
    async fn execute(
        self: &Arc<Self>,
        id: &str,
        generation: u64,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<RunOutcome> {
        if *cancel.borrow() {
            bail!("Agent was interrupted.")
        }
        let snapshot = self.snapshot(id, generation)?;
        let child = if let Some(child) = snapshot.child {
            child
        } else {
            let scope=self.executor.create_child(snapshot.spec.ok_or_else(||anyhow!("Agent context expired from memory. Spawn a new Agent with the same taskName to continue this task."))?).await?;
            if *cancel.borrow() {
                self.executor.abort(scope.session_id.clone()).await?;
                self.executor.dispose(scope.session_id).await?;
                bail!("Agent was interrupted while creating its session.")
            }
            let child = scope.session_id;
            let bound = self.commit(|store| {
                store
                    .records
                    .get_mut(id)
                    .filter(|r| r.generation == generation && active(&r.value))
                    .ok_or_else(|| anyhow!("Stale Agent run."))?
                    .child = Some(child.clone());
                Ok(())
            });
            if let Err(error) = bound {
                self.executor.abort(child.clone()).await?;
                self.executor.dispose(child).await?;
                return Err(error);
            }
            child
        };
        let record = self.commit(|store| {
            let record = store
                .records
                .get_mut(id)
                .filter(|r| r.generation == generation && active(&r.value))
                .ok_or_else(|| anyhow!("Stale Agent run."))?;
            record.value["status"] = json!("running");
            record.value["runNumber"] = json!(number(&record.value["runNumber"]) + 1);
            record.value["turnCount"] = json!(0);
            record.value["toolCallCount"] = json!(0);
            record.value["tools"] = json!([]);
            record.value["output"] = json!("");
            record.value["runUsage"] = empty_usage();
            record.value["lastActivityAt"] = json!(now());
            Ok(record.value.clone())
        })?;
        self.emit(&record);
        let weak = Arc::downgrade(self);
        let owned_id = id.to_string();
        let events: EventSink = Arc::new(move |event, data| {
            if let Some(service) = weak.upgrade() {
                if let Err(error) = service.progress(&owned_id, generation, event, data) {
                    (service.events)(
                        "agent_error",
                        &json!({"id":owned_id,"message":error.to_string()}),
                    );
                }
            }
        });
        let text = record["message"].as_str().unwrap_or_default().to_string();
        // Pi 的真实输入队列允许预先排队。先送达排队期间的输入，再启动 prompt，
        // 避免快速完成的 prompt 在首次 poll 就返回、吞掉尚未交给引擎的消息。
        let pending = self.commit(|store| {
            Ok(std::mem::take(
                &mut store
                    .records
                    .get_mut(id)
                    .ok_or_else(|| anyhow!("Agent disappeared"))?
                    .pending,
            ))
        })?;
        for (kind, text) in pending {
            self.executor.enqueue(child.clone(), text, kind).await?;
        }
        let prompt = self.executor.prompt(
            PromptRequest {
                session_id: child.clone(),
                text,
                internal: false,
                ..Default::default()
            },
            events,
        );
        tokio::pin!(prompt);
        loop {
            tokio::select! {biased;
                changed=cancel.changed()=>{if changed.is_err()||*cancel.borrow(){
                    // Pi abort 等待引擎 idle；必须同时驱动拥有运行 future 的 prompt。
                    let (aborted, _) = tokio::time::timeout(Duration::from_secs(10), async {
                        tokio::join!(self.executor.abort(child.clone()), &mut prompt)
                    }).await.map_err(|_|anyhow!("Agent cancellation did not settle within 10 seconds."))?;
                    aborted?;bail!("Agent was interrupted.")
                }},
                result=&mut prompt=>return result,
            }
        }
    }
    fn progress(&self, id: &str, generation: u64, event: &str, data: &Value) -> Result<()> {
        let observed_ms = crate::product::now_ms();
        // 文本 delta 只续租；五秒心跳和真实状态变更才写盘/外发，避免每个 token 同步刷盘。
        if !matches!(
            event,
            "turn_start"
                | "turn_end"
                | "tool_execution_start"
                | "tool_start"
                | "tool_execution_end"
                | "tool_end"
        ) {
            let mut store = self.store.lock().expect("agent registry");
            let Some(record) = store
                .records
                .get_mut(id)
                .filter(|r| r.generation == generation && active(&r.value))
            else {
                return Ok(());
            };
            record.value["lastActivityAt"] = json!(now());
            if observed_ms.saturating_sub(record.last_progress_ms) < 5000 {
                return Ok(());
            }
        }
        let at = now();
        let record=self.commit(|store|{let Some(record)=store.records.get_mut(id).filter(|r|r.generation==generation&&active(&r.value))else{return Ok(None)};record.value["lastActivityAt"]=json!(at);
            record.last_progress_ms = observed_ms;
            match event{
                "turn_start"=>{record.value["turnCount"]=json!(number(&record.value["turnCount"])+1);record.value["currentActivity"]=json!({"type":"model","stage":"thinking","updatedAt":at});},
                "turn_end"=>{let usage=data.get("usage").unwrap_or(&data["message"]["usage"]);record.value["runUsage"]=add_usage(&record.value["runUsage"],usage);},
                "tool_execution_start"|"tool_start"=>{record.value["toolCallCount"]=json!(number(&record.value["toolCallCount"])+1);let tool=json!({"type":"tool","id":data.get("toolCallId").unwrap_or(&data["id"]),"name":data.get("toolName").unwrap_or(&data["name"]),"args":data["args"],"status":"running","startedAt":at});record.value["tools"].as_array_mut().ok_or_else(||anyhow!("Agent tool catalog invalid"))?.push(tool.clone());record.value["currentActivity"]=tool;},
                "tool_execution_end"|"tool_end"=>{let call=data.get("toolCallId").unwrap_or(&data["id"]);for tool in record.value["tools"].as_array_mut().into_iter().flatten(){if &tool["id"]==call{tool["status"]=json!(if data["isError"]==true||data["error"]==true{"error"}else{"done"});tool["finishedAt"]=json!(at);}}record.value["currentActivity"]=json!({"type":"model","stage":"processing_result","updatedAt":at});},
                _=>{}
            }Ok(Some(record.value.clone()))
        })?;
        if let Some(record) = record {
            if let Some(graph) = self.graph() {
                graph.update_agent(
                    record["parentSessionId"].as_str().unwrap_or_default(),
                    &record,
                )?;
            }
            self.emit(&record);
        }
        Ok(())
    }
    async fn finish(
        self: &Arc<Self>,
        id: &str,
        generation: u64,
        result: Result<RunOutcome>,
        elapsed: Duration,
    ) -> Result<()> {
        let record = self.commit(|store| {
            let Some(record) = store
                .records
                .get_mut(id)
                .filter(|r| r.generation == generation)
            else {
                return Ok(None);
            };
            if !active(&record.value) {
                return Ok(None);
            }
            let at = now();
            let (status, error, output, usage) = match result {
                Ok(outcome) if !outcome.aborted && outcome.error.is_none() => {
                    let usage = if crate::goal_api::usage_tokens(&outcome.usage) > 0 {
                        outcome.usage
                    } else {
                        record.value["runUsage"].clone()
                    };
                    ("completed", "".to_string(), outcome.output, usage)
                }
                Ok(outcome) => (
                    if outcome.aborted {
                        "interrupted"
                    } else {
                        "failed"
                    },
                    outcome
                        .error
                        .unwrap_or_else(|| "Agent was interrupted.".into()),
                    outcome.output,
                    record.value["runUsage"].clone(),
                ),
                Err(error) => (
                    "failed",
                    error.to_string(),
                    String::new(),
                    record.value["runUsage"].clone(),
                ),
            };
            let (output, truncated) =
                bounded_output(if output.is_empty() && status == "completed" {
                    "(Agent returned no text output.)"
                } else {
                    &output
                });
            record.value["status"] = json!(status);
            record.value["error"] = json!(error);
            record.value["output"] = json!(output);
            record.value["fullOutput"] = json!(output);
            record.value["outputTruncated"] = json!(truncated);
            record.value["runUsage"] = usage.clone();
            record.value["usage"] = add_usage(&record.value["usage"], &usage);
            record.value["completedAt"] = json!(at);
            record.value["lastActivityAt"] = json!(at);
            record.value["durationMs"] = json!(elapsed.as_millis() as u64);
            record.value["currentActivity"] = Value::Null;
            record.value["resultVersion"] = json!(number(&record.value["resultVersion"]) + 1);
            let value = record.value.clone();
            let goal_id = record.goal_id.clone();
            enqueue_mailbox(store, &value);
            Ok(Some((value, goal_id)))
        })?;
        if let Some((record, goal_id)) = record {
            let parent = record["parentSessionId"].as_str().unwrap_or_default();
            if let Some(graph) = self.graph() {
                graph.update_agent(parent, &record)?;
            }
            self.emit(&record);
            let goal = self
                .goals
                .account(parent, &goal_id, &record["runUsage"], 0.0)?;
            if goal.is_some_and(|g| g.mode == "team" && g.status == "budget_limited") {
                self.abort_parent(
                    parent,
                    "Team token budget was reached; remaining members were stopped.",
                )
                .await?;
            }
            let notifier = self.completion.lock().expect("agent completion").clone();
            if let Some(notifier) = notifier {
                notifier(record.clone()).await?;
            }
            self.schedule_notification(parent);
        }
        Ok(())
    }
    fn schedule_release(self: &Arc<Self>, id: String, generation: u64) {
        let weak = Arc::downgrade(self);
        let retention = self.retention;
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            let child = service.commit(|store| {
                let Some(record) = store
                    .records
                    .get_mut(&id)
                    .filter(|r| r.generation == generation && !r.slot && !active(&r.value))
                else {
                    return Ok(None);
                };
                record.spec = None;
                Ok(record.child.take())
            });
            if let Ok(Some(child)) = child {
                let _ = service.executor.dispose(child).await;
            }
        });
    }
    pub(crate) async fn send_message(
        &self,
        parent: &str,
        target: &str,
        text: &str,
    ) -> Result<Value> {
        self.input(parent, target, text, InputKind::Steer).await
    }
    pub(crate) async fn update_task(
        self: &Arc<Self>,
        parent: &str,
        target: &str,
        input: &Value,
    ) -> Result<Value> {
        if !self
            .goals
            .get(parent)
            .is_some_and(|goal| goal.mode == "team" && goal.status == "active")
        {
            bail!("No active Team is available.");
        }
        let graph = self
            .graph()
            .ok_or_else(|| anyhow!("Team task graph has not been installed."))?;
        let previous = graph_task(graph.as_ref(), parent, target)
            .ok_or_else(|| anyhow!("Unknown Team task."))?;
        let updated = graph.update_task(parent, target, input)?;
        if let Some(agent) = previous["agentId"].as_str().filter(|s| !s.is_empty()) {
            if self.find(parent, agent).is_some_and(|agent| active(&agent)) {
                self.interrupt(parent, agent, "Team task was updated before execution.")
                    .await?;
            }
        }
        if updated["autoStart"].as_bool().unwrap_or(true) {
            let mut params = updated.clone();
            params["teamTaskId"] = updated["id"].clone();
            self.spawn(parent.into(), params).await?;
        }
        Ok(graph_task(
            graph.as_ref(),
            parent,
            updated["id"].as_str().unwrap_or_default(),
        )
        .unwrap_or(updated))
    }
    pub(crate) fn validate_member(&self, parent: &str, sender: &str) -> Result<Value> {
        let member = self
            .find(parent, sender)
            .ok_or_else(|| anyhow!("Unknown sending agent: {sender}"))?;
        if !matches!(member["status"].as_str(), Some("starting" | "running")) {
            bail!("Only a starting or running Agent can send team messages.");
        }
        if self
            .graph()
            .and_then(|graph| graph.projection(parent))
            .is_none()
        {
            bail!("No Team is available for this member.");
        }
        Ok(member)
    }
    pub(crate) async fn send_from_agent(
        &self,
        parent: &str,
        sender: &str,
        target: &str,
        text: &str,
    ) -> Result<Value> {
        let member = self.validate_member(parent, sender)?;
        let recipient = self
            .find(parent, target)
            .ok_or_else(|| anyhow!("Unknown agent: {target}"))?;
        if member["id"] == recipient["id"] {
            bail!("An Agent cannot send a message to itself.");
        }
        let text = message(text)?;
        let result = self.send_message(parent, target, &text).await?;
        self.graph()
            .ok_or_else(|| anyhow!("Team unavailable"))?
            .communication(
                parent,
                member["id"].as_str().unwrap_or_default(),
                recipient["id"].as_str().unwrap_or_default(),
                &text,
            )?;
        Ok(result)
    }
    async fn input(
        &self,
        parent: &str,
        target: &str,
        text: &str,
        kind: InputKind,
    ) -> Result<Value> {
        let text = message(text)?;
        let record = self
            .find(parent, target)
            .ok_or_else(|| anyhow!("Unknown agent: {target}"))?;
        let id = record["id"].as_str().expect("agent id");
        let (value, child) = self.commit(|store| {
            let record = store.records.get_mut(id).expect("agent record");
            if matches!(record.value["status"].as_str(), Some("queued" | "starting")) {
                record.pending.push((kind, text.clone()));
                Ok((record.value.clone(), None))
            } else if record.value["status"] == "running" {
                Ok((record.value.clone(), record.child.clone()))
            } else {
                bail!("Agent is not running. Use followup_task to start another run.")
            }
        })?;
        if let Some(child) = child {
            self.executor.enqueue(child, text, kind).await?;
        }
        self.emit(&value);
        Ok(value)
    }
    pub(crate) async fn followup(
        self: &Arc<Self>,
        parent: &str,
        target: &str,
        text: &str,
    ) -> Result<Value> {
        let text = message(text)?;
        let current = self
            .find(parent, target)
            .ok_or_else(|| anyhow!("Unknown agent: {target}"))?;
        if active(&current) {
            return self.input(parent, target, &text, InputKind::FollowUp).await;
        }
        let id = current["id"].as_str().expect("agent id");
        // 等旧运行真正释放槽位，避免中断与 follow-up 同时驱动一个 Pi 会话。
        loop {
            let notified = self.changed.notified();
            let slot = self
                .store
                .lock()
                .expect("agent registry")
                .records
                .get(id)
                .is_some_and(|r| r.slot);
            if !slot {
                break;
            }
            notified.await;
        }
        let graph = self.graph().filter(|_| {
            self.goals
                .get(parent)
                .is_some_and(|g| g.mode == "team" && g.status == "active")
        });
        let task_id = current["teamTaskId"].as_str().unwrap_or_default();
        let (record, reset) = self.commit(|store| {
            let record = store.records.get_mut(id).ok_or_else(|| anyhow!("Agent disappeared"))?;
            if active(&record.value) {
                record.pending.push((InputKind::FollowUp, text.clone()));
                return Ok((record.value.clone(), false));
            }
            if record.child.is_none() {
                bail!("Agent context expired from memory. Spawn a new Agent with the same taskName to continue this task.")
            }
            // 先保护实际闲置句柄，再修改 Team 图；过期上下文不能清掉已验证任务。
            if !task_id.is_empty() {
                if let Some(graph) = &graph {
                    graph.update_task(parent, task_id, &json!({"message": text}))?;
                }
            }
            if graph.is_none() {
                record.task_id.clear();
                record.value["teamTaskId"] = json!("");
            }
            record.value["status"] = json!("queued");
            record.value["message"] = json!(text);
            record.value["startedAt"] = json!(now());
            record.value["completedAt"] = Value::Null;
            record.value["durationMs"] = Value::Null;
            record.value["error"] = json!("");
            Ok((record.value.clone(), true))
        })?;
        if reset && !task_id.is_empty() {
            if let Some(graph) = graph {
                graph.bind(parent, task_id, &record)?;
            }
        }
        self.emit(&record);
        self.pump()?;
        Ok(record)
    }
    pub(crate) async fn interrupt(
        self: &Arc<Self>,
        parent: &str,
        target: &str,
        reason: &str,
    ) -> Result<Value> {
        let current = self
            .find(parent, target)
            .ok_or_else(|| anyhow!("Unknown agent: {target}"))?;
        if !active(&current) {
            return Ok(current);
        }
        let id = current["id"].as_str().expect("agent id");
        let (record, child, cancel) = self.commit(|store| {
            let record = store
                .records
                .get_mut(id)
                .ok_or_else(|| anyhow!("Agent disappeared"))?;
            if !active(&record.value) {
                return Ok((record.value.clone(), None, None));
            }
            record.value["status"] = json!("interrupted");
            record.value["error"] = json!(reason);
            record.value["currentActivity"] = Value::Null;
            record.value["completedAt"] = json!(now());
            record.value["lastActivityAt"] = record.value["completedAt"].clone();
            record.value["resultVersion"] = json!(number(&record.value["resultVersion"]) + 1);
            record.pending.clear();
            let value = record.value.clone();
            let child = record.child.clone();
            let cancel = record.cancel.clone();
            enqueue_mailbox(store, &value);
            Ok((value, child, cancel))
        })?;
        if let Some(cancel) = cancel {
            let _ = cancel.send(true);
        }
        if let Some(child) = child {
            self.executor.abort(child).await?;
        }
        if let Some(graph) = self.graph() {
            graph.update_agent(parent, &record)?;
        }
        self.emit(&record);
        let notifier = self.completion.lock().expect("agent completion").clone();
        if let Some(notifier) = notifier {
            notifier(record.clone()).await?;
        }
        self.schedule_notification(parent);
        self.pump()?;
        Ok(record)
    }
    pub(crate) async fn abort_parent(
        self: &Arc<Self>,
        parent: &str,
        reason: &str,
    ) -> Result<usize> {
        self.suspend_notifications(parent);
        let ids = self
            .list(parent)
            .into_iter()
            .filter(active)
            .filter_map(|v| v["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        for id in &ids {
            self.interrupt(parent, id, reason).await?;
        }
        Ok(ids.len())
    }
    pub(crate) fn mailbox(&self, parent: &str) -> Vec<Value> {
        let store = self.store.lock().expect("agent registry");
        let mut messages = store
            .mailbox
            .values()
            .filter(|v| v["parentSessionId"] == parent)
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by_key(|v| v["queuedAt"].as_str().unwrap_or_default().to_string());
        messages
    }
    pub(crate) fn acknowledge(&self, parent: &str, entries: &[Value]) -> Result<()> {
        self.commit(|store| {
            store.mailbox.retain(|_, value| {
                value["parentSessionId"] != parent
                    || !entries.iter().any(|entry| {
                        if entry["mailboxId"].is_string() {
                            entry["mailboxId"] == value["mailboxId"]
                        } else {
                            entry["id"] == value["id"]
                                && entry["resultVersion"] == value["resultVersion"]
                        }
                    })
            });
            Ok(())
        })
    }
    pub(crate) async fn wait(&self, parent: &str, target: &str, timeout_ms: u64) -> Result<Value> {
        let target = if target.is_empty() {
            None
        } else {
            Some(
                self.find(parent, target)
                    .ok_or_else(|| anyhow!("Unknown agent: {target}"))?["id"]
                    .clone(),
            )
        };
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms.clamp(250, 30_000));
        loop {
            let notified = self.changed.notified();
            let agents = self.list(parent);
            let pending = self
                .mailbox(parent)
                .into_iter()
                .find(|a| target.as_ref().is_none_or(|t| &a["id"] == t));
            let terminal = target
                .as_ref()
                .and_then(|t| agents.iter().find(|a| &a["id"] == t && !active(a)).cloned());
            if let Some(agent) = pending.or(terminal) {
                return Ok(json!({"timedOut":false,"agents":agents,"agent":agent}));
            }
            if !agents.iter().any(active) {
                return Ok(json!({"timedOut":false,"agents":agents,"agent":null}));
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return Ok(json!({"timedOut":true,"agents":self.list(parent),"agent":null}));
            }
        }
    }
    pub(crate) async fn remove_parent(self: &Arc<Self>, parent: &str) -> Result<()> {
        self.abort_parent(parent, "Parent session was deleted.")
            .await?;
        self.wait_runs(Some(parent)).await?;
        let children = self.commit(|store| {
            let children = store
                .records
                .values()
                .filter(|r| r.value["parentSessionId"] == parent)
                .filter_map(|r| r.child.clone())
                .collect::<Vec<_>>();
            store
                .records
                .retain(|_, r| r.value["parentSessionId"] != parent);
            store.mailbox.retain(|_, v| v["parentSessionId"] != parent);
            Ok(children)
        })?;
        for child in children {
            self.executor.dispose(child).await?;
        }
        Ok(())
    }
    async fn wait_runs(&self, parent: Option<&str>) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let changed = self.changed.notified();
                if !self
                    .store
                    .lock()
                    .expect("agent registry")
                    .records
                    .values()
                    .any(|r| {
                        r.slot && parent.is_none_or(|parent| r.value["parentSessionId"] == parent)
                    })
                {
                    return;
                }
                // Also wake periodically so a disk-write error cannot hide a settled worker.
                tokio::select! {
                    _ = changed => {},
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {},
                }
            }
        })
        .await
        .map_err(|_| anyhow!("Cancelled Agent workers did not finish cleanup within 10 seconds."))
    }
    pub(crate) async fn shutdown(self: &Arc<Self>) -> Result<()> {
        self.commit(|s| {
            s.closed = true;
            Ok(())
        })?;
        let parents = self
            .list("")
            .iter()
            .filter_map(|r| r["parentSessionId"].as_str().map(str::to_owned))
            .collect::<HashSet<_>>();
        for parent in parents {
            self.abort_parent(&parent, "Agent service is shutting down.")
                .await?;
        }
        let notifications = self
            .notification_tasks
            .lock()
            .expect("completion tasks")
            .iter()
            .map(|(parent, task)| (parent.clone(), task.finished.clone()))
            .collect::<Vec<_>>();
        for (parent, _) in &notifications {
            self.suspend_notifications(parent);
            // Root 的 executor.abort 驱动原生停止，worker 同时继续 poll callback。
            self.executor.abort(parent.clone()).await?;
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            for (_, mut finished) in notifications {
                while !*finished.borrow() {
                    if finished.changed().await.is_err() {
                        break;
                    }
                }
            }
        })
        .await
        .map_err(|_| anyhow!("Completion workers did not settle within 10 seconds."))?;
        self.wait_runs(None).await?;
        let children = self
            .store
            .lock()
            .expect("agent registry")
            .records
            .values()
            .filter_map(|r| r.child.clone())
            .collect::<Vec<_>>();
        for child in children {
            self.executor.dispose(child).await?;
        }
        Ok(())
    }
}
pub(crate) fn completion_prompt(agent: &Value) -> String {
    format!("{COMPLETION_MARKER}\nBackground agent \"{}\" ({}) has completed with status: {}.\n\nOutput:\n{}\n{}\nUse this result for your next actions. If other background agents are still running, briefly note the progress; if all background agents are done, summarize the combined results for the user.",agent["taskName"].as_str().unwrap_or_default(),agent["id"].as_str().unwrap_or_default(),agent["status"].as_str().unwrap_or_default(),agent["output"].as_str().unwrap_or_default().chars().take(2000).collect::<String>(),agent["error"].as_str().filter(|s|!s.is_empty()).map(|e|format!("Error: {e}")).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Fixture {
        permits: Arc<tokio::sync::Semaphore>,
        contexts: Arc<Mutex<Vec<ChildRequest>>>,
        signals: Arc<Mutex<BTreeMap<String, watch::Sender<bool>>>>,
        inputs: Arc<Mutex<Vec<(String, String, InputKind)>>>,
        running: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        disposed: Arc<AtomicUsize>,
        settle_ms: Arc<AtomicUsize>,
    }
    impl Fixture {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                permits: Arc::new(tokio::sync::Semaphore::new(0)),
                contexts: Arc::new(Mutex::new(vec![])),
                signals: Arc::new(Mutex::new(BTreeMap::new())),
                inputs: Arc::new(Mutex::new(vec![])),
                running: Arc::new(AtomicUsize::new(0)),
                maximum: Arc::new(AtomicUsize::new(0)),
                disposed: Arc::new(AtomicUsize::new(0)),
                settle_ms: Arc::new(AtomicUsize::new(0)),
            })
        }
    }
    struct Running(Arc<AtomicUsize>);
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl SessionExecutor for Fixture {
        fn scope(&self, id: String) -> BoxFuture<'static, Result<SessionScope>> {
            Box::pin(async move {
                Ok(SessionScope {
                    session_id: id,
                    cwd: std::env::temp_dir().to_string_lossy().into(),
                    model: "fixture/local".into(),
                    tool_names: vec![
                        "read".into(),
                        "write".into(),
                        "spawn_agent".into(),
                        "update_plan".into(),
                        "get_plan".into(),
                    ],
                    execution_mode: "full-access".into(),
                    permission_mode: "ignore".into(),
                    ..Default::default()
                })
            })
        }
        fn create_child(&self, request: ChildRequest) -> BoxFuture<'static, Result<SessionScope>> {
            let contexts = self.contexts.clone();
            let signals = self.signals.clone();
            Box::pin(async move {
                let (tx, _) = watch::channel(false);
                signals.lock().unwrap().insert(request.id.clone(), tx);
                let id = request.id.clone();
                let parent = request.parent.session_id.clone();
                contexts.lock().unwrap().push(request);
                Ok(SessionScope {
                    session_id: id,
                    parent_session_id: Some(parent),
                    ..Default::default()
                })
            })
        }
        fn prompt(
            &self,
            request: PromptRequest,
            events: EventSink,
        ) -> BoxFuture<'static, Result<RunOutcome>> {
            let permits = self.permits.clone();
            let mut signal = self.signals.lock().unwrap()[&request.session_id].subscribe();
            let running = self.running.clone();
            let maximum = self.maximum.clone();
            let settle_ms = self.settle_ms.clone();
            Box::pin(async move {
                let count = running.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(count, Ordering::SeqCst);
                let _running = Running(running);
                events("turn_start", &json!({}));
                let result = tokio::select! {
                    permit=permits.acquire_owned()=>{permit.unwrap().forget();events("turn_end",&json!({"usage":{"input":3,"output":2,"totalTokens":5}}));Ok(RunOutcome{output:format!("verified {}",request.text),usage:json!({"input":3,"output":2,"totalTokens":5}),..Default::default()})},
                    _=signal.changed()=>Ok(RunOutcome{aborted:true,..Default::default()})
                };
                tokio::time::sleep(Duration::from_millis(
                    settle_ms.load(Ordering::SeqCst) as u64
                ))
                .await;
                result
            })
        }
        fn enqueue(
            &self,
            id: String,
            text: String,
            kind: InputKind,
        ) -> BoxFuture<'static, Result<()>> {
            let inputs = self.inputs.clone();
            Box::pin(async move {
                inputs.lock().unwrap().push((id, text, kind));
                Ok(())
            })
        }
        fn abort(&self, id: String) -> BoxFuture<'static, Result<()>> {
            let signals = self.signals.clone();
            Box::pin(async move {
                if let Some(signal) = signals.lock().unwrap().get(&id) {
                    let _ = signal.send(true);
                }
                Ok(())
            })
        }
        fn dispose(&self, _: String) -> BoxFuture<'static, Result<()>> {
            let disposed = self.disposed.clone();
            let running = self.running.clone();
            let settle_ms = self.settle_ms.clone();
            Box::pin(async move {
                if settle_ms.load(Ordering::SeqCst) != 0 && running.load(Ordering::SeqCst) != 0 {
                    bail!("Fixture prompt still holds its mutation guard.")
                }
                disposed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }
    fn sandbox() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pisper-native-agents-{}", crate::product::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    async fn until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn shutdown_waits_for_actual_worker_finalization_before_disposing() {
        let dir = sandbox();
        let fixture = Fixture::new();
        fixture.settle_ms.store(150, Ordering::SeqCst);
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let service = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        let child = service
            .spawn(
                "parent".into(),
                json!({"taskName":"held", "message":"held real executor"}),
            )
            .await
            .unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 1).await;
        let started = Instant::now();
        service.shutdown().await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(fixture.running.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.disposed.load(Ordering::SeqCst), 1);
        assert_eq!(
            service
                .find("parent", child["id"].as_str().unwrap())
                .unwrap()["status"],
            "interrupted"
        );
        assert!(service
            .store
            .lock()
            .unwrap()
            .records
            .values()
            .all(|r| !r.slot));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn four_real_slots_queue_cancel_and_followup_preserve_context() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let service = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        let mut ids = vec![];
        for index in 0..5 {
            let agent = service
                .spawn(
                    "parent".into(),
                    json!({"taskName":format!("task_{index}"),"message":"bounded fixture"}),
                )
                .await
                .unwrap();
            ids.push(agent["id"].as_str().unwrap().to_string());
        }
        until(|| fixture.running.load(Ordering::SeqCst) == 4).await;
        assert_eq!(fixture.contexts.lock().unwrap().len(), 4);
        assert_eq!(service.find("parent", &ids[4]).unwrap()["status"], "queued");
        assert!(service.find("another-parent", &ids[0]).is_none());
        for context in fixture.contexts.lock().unwrap().iter() {
            assert!(!context
                .tools
                .iter()
                .any(|name| CHILD_FORBIDDEN.contains(&name.as_str())));
            assert_eq!(context.parent.session_id, "parent");
        }
        service
            .send_message("parent", &ids[4], "queued handoff")
            .await
            .unwrap();
        service
            .interrupt("parent", &ids[0], "explicit user stop")
            .await
            .unwrap();
        until(|| fixture.contexts.lock().unwrap().len() == 5).await;
        assert!(fixture.maximum.load(Ordering::SeqCst) <= 4);
        fixture.permits.add_permits(4);
        until(|| !service.has_active("parent")).await;
        let result = service.wait("parent", &ids[1], 250).await.unwrap();
        assert_eq!(result["timedOut"], false);
        assert_eq!(result["agent"]["status"], "completed");
        assert_eq!(result["agent"]["runUsage"]["totalTokens"], 5);
        assert!(fixture
            .inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, text, _)| text == "queued handoff"));
        service
            .followup("parent", &ids[1], "next bounded task")
            .await
            .unwrap();
        until(|| service.find("parent", &ids[1]).unwrap()["status"] == "running").await;
        fixture.permits.add_permits(1);
        until(|| service.find("parent", &ids[1]).unwrap()["status"] == "completed").await;
        assert_eq!(fixture.contexts.lock().unwrap().len(), 5);
        assert_eq!(
            service.find("parent", &ids[1]).unwrap()["usage"]["totalTokens"],
            10
        );
        service.shutdown().await.unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 0).await;
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn restart_interrupts_active_records_and_keeps_mailbox_without_reexecution() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let service = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals.clone(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        service
            .spawn(
                "parent".into(),
                json!({"taskName":"one","message":"synthetic pending task"}),
            )
            .await
            .unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 1).await;
        // 模拟磁盘快照进入新的隔离进程目录，不让两个服务并发写同一个文件。
        let copied = dir.join("restarted.json");
        std::fs::copy(dir.join("agents.json"), &copied).unwrap();
        let restarted_fixture = Fixture::new();
        let restarted = AgentService::new(
            &copied,
            restarted_fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        assert_eq!(restarted.list("parent")[0]["status"], "interrupted");
        assert_eq!(restarted.mailbox("parent").len(), 1);
        assert_eq!(restarted_fixture.contexts.lock().unwrap().len(), 0);
        let id = restarted.list("parent")[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(restarted
            .followup("parent", &id, "continue without a context")
            .await
            .is_err());
        let entry = restarted.mailbox("parent")[0].clone();
        restarted
            .acknowledge("different-parent", &[entry.clone()])
            .unwrap();
        assert_eq!(restarted.mailbox("parent").len(), 1);
        restarted.acknowledge("parent", &[entry]).unwrap();
        assert!(restarted.mailbox("parent").is_empty());
        service.shutdown().await.unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 0).await;
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn notification_suspend_keeps_native_callback_polled_until_shutdown_settles() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let service = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        let (parent_abort, parent_aborted) = watch::channel(false);
        fixture
            .signals
            .lock()
            .unwrap()
            .insert("parent".into(), parent_abort);
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let dropped_early = Arc::new(AtomicBool::new(false));
        struct CallbackLife {
            finished: Arc<AtomicBool>,
            dropped_early: Arc<AtomicBool>,
        }
        impl Drop for CallbackLife {
            fn drop(&mut self) {
                if !self.finished.load(Ordering::SeqCst) {
                    self.dropped_early.store(true, Ordering::SeqCst);
                }
            }
        }
        let own_started = started.clone();
        let own_finished = finished.clone();
        let own_dropped = dropped_early.clone();
        service.set_completion_dispatcher(Arc::new(move |_| {
            let started = own_started.clone();
            let finished = own_finished.clone();
            let dropped_early = own_dropped.clone();
            let mut aborted = parent_aborted.clone();
            Box::pin(async move {
                let _life = CallbackLife {
                    finished: finished.clone(),
                    dropped_early,
                };
                started.store(true, Ordering::SeqCst);
                while !*aborted.borrow() {
                    aborted.changed().await.unwrap();
                }
                // 原生 finally 在 abort 请求后仍需被 poll，才能释放执行状态。
                tokio::time::sleep(Duration::from_millis(150)).await;
                finished.store(true, Ordering::SeqCst);
                Ok(false)
            })
        }));
        service
            .spawn(
                "parent".into(),
                json!({"taskName":"proof", "message":"actual result"}),
            )
            .await
            .unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 1).await;
        fixture.permits.add_permits(1);
        until(|| started.load(Ordering::SeqCst)).await;
        service.suspend_notifications("parent");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!finished.load(Ordering::SeqCst));
        assert!(!dropped_early.load(Ordering::SeqCst));
        assert_eq!(service.mailbox("parent").len(), 1);
        let stopped = Instant::now();
        service.shutdown().await.unwrap();
        assert!(stopped.elapsed() >= Duration::from_millis(150));
        assert!(finished.load(Ordering::SeqCst));
        assert!(!dropped_early.load(Ordering::SeqCst));
        assert_eq!(service.mailbox("parent").len(), 1);
        assert!(service.notification_tasks.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn completion_coalesces_real_results_and_acknowledges_only_delivered_mailbox() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let service = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        let batches = Arc::new(Mutex::new(Vec::<CompletionBatch>::new()));
        let own_batches = batches.clone();
        service.set_completion_dispatcher(Arc::new(move |batch| {
            let batches = own_batches.clone();
            Box::pin(async move {
                batches.lock().unwrap().push(batch);
                Ok(true)
            })
        }));
        for index in 0..2 {
            service
                .spawn(
                    "parent".into(),
                    json!({"taskName":format!("result_{index}"),"message":"bounded proof"}),
                )
                .await
                .unwrap();
        }
        until(|| fixture.running.load(Ordering::SeqCst) == 2).await;
        fixture.permits.add_permits(2);
        until(|| !service.has_active("parent")).await;
        assert_eq!(service.mailbox("parent").len(), 2);
        until(|| service.mailbox("parent").is_empty()).await;
        assert_eq!(batches.lock().unwrap().len(), 1);
        let batch = batches.lock().unwrap()[0].clone();
        assert_eq!(batch.entries.len(), 2);
        assert_eq!(batch.session_id, "parent");
        assert!(batch.prompt.contains(COMPLETION_MARKER));
        assert!(batch.prompt.contains("verified bounded proof"));
        service.suspend_notifications("parent");
        service
            .spawn(
                "parent".into(),
                json!({"taskName":"retain","message":"retain real result"}),
            )
            .await
            .unwrap();
        until(|| fixture.running.load(Ordering::SeqCst) == 1).await;
        fixture.permits.add_permits(1);
        until(|| !service.has_active("parent")).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(service.mailbox("parent").len(), 1);
        assert_eq!(batches.lock().unwrap().len(), 1);
        service.resume_notifications("parent");
        until(|| service.mailbox("parent").is_empty()).await;
        assert_eq!(batches.lock().unwrap().len(), 2);
        service.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
