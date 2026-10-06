use crate::native_workflow::{self as store, inputs, model, Result, WorkflowError};
use crate::scheduled_jobs::time;
use crate::workflow_engine::{
    duration_label, AgentRequest, AgentResult, RunCancellation, WorkflowService,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify};

struct Execution {
    cancellation: Arc<RunCancellation>,
    session: Mutex<String>,
    storage_error: Mutex<Option<WorkflowError>>,
    finished: AtomicBool,
    done: Notify,
}
struct ScheduleInner {
    state: Value,
    executions: HashMap<String, Arc<Execution>>,
    closed: bool,
}
pub(crate) struct ScheduleService {
    path: PathBuf,
    cwd: String,
    workflows: Arc<WorkflowService>,
    inner: Mutex<ScheduleInner>,
    shutdown: Arc<RunCancellation>,
    timer_finished: AtomicBool,
    timer_done: Notify,
}
impl ScheduleService {
    pub(crate) async fn open(
        path: PathBuf,
        cwd: String,
        workflows: Arc<WorkflowService>,
        tick: Duration,
    ) -> Result<Arc<Self>> {
        let stored = store::read_json(&path, json!({"version":1,"tasks":[],"runs":[]}))?;
        let stamp = store::now();
        let mut runs = stored["runs"].as_array().cloned().unwrap_or_default();
        if runs.len() > 200 {
            runs.drain(..runs.len() - 200);
        }
        for run in &mut runs {
            if run["status"] == "running" {
                let elapsed = run["startedAt"]
                    .as_str()
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                    .map(|date| (Utc::now() - date).num_milliseconds().max(0) as u64)
                    .unwrap_or(0);
                run["status"] = json!("interrupted");
                run["finishedAt"] = json!(stamp);
                run["durationMs"] = json!(elapsed.max(run["durationMs"].as_u64().unwrap_or(0)));
                run["error"] = json!("任务因 Pisper 重启而中断。");
            }
        }
        let mut tasks = stored["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|task| normalize(task, &cwd))
            .collect::<Result<Vec<_>>>()?;
        for task in &mut tasks {
            let interrupted = runs.iter().rev().any(|run| {
                run["taskId"] == task["id"]
                    && run["status"] == "interrupted"
                    && (task["lastRunAt"].is_null() || run["startedAt"] == task["lastRunAt"])
            });
            if task["lastStatus"] == "running" {
                task["lastStatus"] = json!("interrupted");
                task["lastError"] = json!("任务因 Pisper 重启而中断。");
                task["updatedAt"] = json!(stamp);
            }
            if task["lastStatus"] == "interrupted" && interrupted && task["lastError"] == "" {
                task["lastError"] = json!("任务因 Pisper 重启而中断。");
            }
        }
        let state = json!({"version":1,"tasks":tasks,"runs":runs});
        store::save_json(&path, &state)?;
        let service = Arc::new(Self {
            path,
            cwd,
            workflows,
            inner: Mutex::new(ScheduleInner {
                state,
                executions: HashMap::new(),
                closed: false,
            }),
            shutdown: Arc::new(RunCancellation::default()),
            timer_finished: AtomicBool::new(false),
            timer_done: Notify::new(),
        });
        let owned = service.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(tick.max(Duration::from_millis(1)));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {_=owned.shutdown.cancelled()=>break,_=timer.tick()=>{if let Err(error)=owned.tick_at(Utc::now()).await{tracing::error!(code=%error.code,"Schedule tick failed");}}}
            }
            owned.timer_finished.store(true, Ordering::Release);
            owned.timer_done.notify_waiters();
        });
        Ok(service)
    }
    pub(crate) async fn state(&self) -> Value {
        self.inner.lock().await.state.clone()
    }
    pub(crate) async fn dashboard(&self) -> Result<Value> {
        let mut state = self.state().await;
        state["defaultCwd"] = json!(self.cwd);
        state["workflows"]=json!(self.workflows.list().await.into_iter().filter(|w|w["status"]=="published").map(|w|json!({"id":w["id"],"name":w["name"],"description":w["description"],"revision":w["revision"],"inputs":w["inputs"]})).collect::<Vec<_>>());
        let catalog = self.workflows.executor().catalog().await?;
        state["models"] = catalog["models"].clone();
        state["notificationTargets"] = catalog["notificationTargets"].clone();
        Ok(state)
    }
    async fn transaction<T>(
        &self,
        change: impl FnOnce(&mut ScheduleInner) -> Result<T>,
    ) -> Result<T> {
        let mut inner = self.inner.lock().await;
        let before = inner.state.clone();
        let value = match change(&mut inner) {
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
        Ok(value)
    }
    async fn normalize_input(&self, input: &Value, current: Option<&Value>) -> Result<Value> {
        let mut merged = current.cloned().unwrap_or(json!({}));
        let object = input
            .as_object()
            .ok_or_else(|| WorkflowError::invalid("定时任务输入必须是对象。"))?;
        for (key, value) in object {
            merged[key] = value.clone();
        }
        let name = model::text(&merged["name"], 120);
        if name.is_empty() {
            return Err(WorkflowError::invalid("任务名称不能为空。"));
        }
        let target = if merged["targetType"] == "workflow" {
            "workflow"
        } else {
            "prompt"
        };
        if target == "prompt" && model::text(&merged["prompt"], 100000).is_empty() {
            return Err(WorkflowError::invalid("任务 Prompt 不能为空。"));
        }
        if target == "workflow" {
            let id = merged["workflowId"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| WorkflowError::invalid("请选择要运行的工作流。"))?;
            let workflow = self
                .workflows
                .get_workflow(id)
                .await
                .ok_or_else(|| WorkflowError::invalid("选择的工作流不存在。"))?;
            if workflow["status"] != "published" {
                return Err(WorkflowError::invalid("定时任务只能调用已发布的工作流。"));
            }
            inputs::validate(workflow.get("inputs"), merged.get("workflowInputs"))?;
        }
        if target == "prompt" && object.contains_key("cwd") {
            let cwd = merged["cwd"]
                .as_str()
                .ok_or_else(|| WorkflowError::invalid("工作目录无效。"))?;
            let path = std::fs::canonicalize(cwd)
                .map_err(|_| WorkflowError::invalid("工作目录不存在。"))?;
            if !path.is_dir() {
                return Err(WorkflowError::invalid("工作目录不是目录。"));
            }
            merged["cwd"] = json!(path.to_string_lossy());
        }
        merged["name"] = json!(name);
        merged["updatedAt"] = json!(store::now());
        normalize(&merged, &self.cwd)
    }
    pub(crate) async fn create(&self, input: Value) -> Result<Value> {
        if !input.is_object() {
            return Err(WorkflowError::invalid("定时任务输入必须是对象。"));
        }
        let mut input = input;
        input["id"] = json!(store::id()?);
        input["createdAt"] = json!(store::now());
        let task = self.normalize_input(&input, None).await?;
        self.transaction(|inner| {
            inner.state["tasks"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("schedule state invalid"))?
                .insert(0, task.clone());
            Ok(task)
        })
        .await
    }
    pub(crate) async fn update(&self, id: &str, input: Value) -> Result<Option<Value>> {
        let current = self.inner.lock().await.state["tasks"]
            .as_array()
            .and_then(|tasks| tasks.iter().find(|t| t["id"] == id))
            .cloned();
        let Some(current) = current else {
            return Ok(None);
        };
        let mut task = self.normalize_input(&input, Some(&current)).await?;
        self.transaction(|inner| {
            let Some(existing) = find_task_mut(&mut inner.state, id) else {
                return Ok(None);
            };
            for key in [
                "id",
                "createdAt",
                "lastRunAt",
                "lastStatus",
                "lastSummary",
                "lastError",
                "lastNotificationError",
            ] {
                task[key] = existing[key].clone();
            }
            *existing = task.clone();
            Ok(Some(task))
        })
        .await
    }
    pub(crate) async fn remove(&self, id: &str) -> Result<bool> {
        self.transaction(|inner| {
            if inner.executions.contains_key(id) {
                return Err(WorkflowError::busy("任务正在运行，暂时不能删除。"));
            }
            let tasks = inner.state["tasks"]
                .as_array_mut()
                .ok_or_else(|| WorkflowError::io("schedule state invalid"))?;
            let before = tasks.len();
            tasks.retain(|t| t["id"] != id);
            let deleted = before != tasks.len();
            if deleted {
                if let Some(runs) = inner.state["runs"].as_array_mut() {
                    runs.retain(|r| r["taskId"] != id);
                }
            }
            Ok(deleted)
        })
        .await
    }
    pub(crate) async fn run_now(self: &Arc<Self>, id: &str) -> Result<Option<Value>> {
        self.start_run(id, "manual", Utc::now()).await
    }
    pub(crate) async fn tick_at(self: &Arc<Self>, now: DateTime<Utc>) -> Result<()> {
        let ids = {
            let inner = self.inner.lock().await;
            if inner.closed {
                return Ok(());
            }
            inner.state["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|task| {
                    task["enabled"] != false
                        && task["nextRunAt"]
                            .as_str()
                            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                            .is_some_and(|date| date <= now)
                        && !inner
                            .executions
                            .contains_key(task["id"].as_str().unwrap_or(""))
                })
                .filter_map(|task| task["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        for id in ids {
            match self.start_run(&id, "scheduled", now).await {
                Err(error) if error.status == StatusCode::CONFLICT => {}
                result => {
                    result?;
                }
            }
        }
        Ok(())
    }
    async fn start_run(
        self: &Arc<Self>,
        id: &str,
        trigger: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Value>> {
        let control = Arc::new(Execution {
            cancellation: Arc::new(RunCancellation::default()),
            session: Mutex::new(String::new()),
            storage_error: Mutex::new(None),
            finished: AtomicBool::new(false),
            done: Notify::new(),
        });
        let run_id = store::id()?;
        let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let task = {
            let mut inner = self.inner.lock().await;
            if inner.closed {
                return Err(WorkflowError::coded(
                    "SCHEDULE_INTERRUPTED",
                    "任务因 Pisper 关闭而中断。",
                ));
            }
            if inner.executions.contains_key(id) {
                return Err(WorkflowError::busy("任务已经在运行。"));
            }
            let before = inner.state.clone();
            let Some(task) = find_task_mut(&mut inner.state, id) else {
                return Ok(None);
            };
            task["lastRunAt"] = json!(stamp);
            task["lastStatus"] = json!("running");
            task["lastError"] = json!("");
            if trigger == "scheduled" {
                task["nextRunAt"] = json!(time::next_run(task, now)?);
            }
            let task = task.clone();
            inner.state["runs"].as_array_mut().ok_or_else(||WorkflowError::io("schedule runs invalid"))?.push(json!({"id":run_id,"taskId":id,"trigger":trigger,"status":"running","startedAt":stamp,"finishedAt":null,"durationMs":0,"summary":"","error":"","sessionId":"","workflowRunId":""}));
            if let Some(runs) = inner.state["runs"].as_array_mut() {
                if runs.len() > 200 {
                    runs.drain(..runs.len() - 200);
                }
            }
            if let Err(error) = store::save_json(&self.path, &inner.state) {
                inner.state = before;
                return Err(error);
            }
            inner.executions.insert(id.to_owned(), control.clone());
            task
        };
        let service = self.clone();
        let owned = task.clone();
        tokio::spawn(async move {
            service.execute(owned, run_id, control).await;
        });
        Ok(Some(task))
    }
    async fn execute(self: Arc<Self>, task: Value, run_id: String, control: Arc<Execution>) {
        let started = Instant::now();
        let id = task["id"].as_str().unwrap_or("");
        let result = if task["targetType"] == "workflow" {
            self.execute_workflow(&task, &run_id, &control).await
        } else {
            let ctl = control.clone();
            let observed_service = self.clone();
            let observed_run = run_id.clone();
            let observer = Arc::new(move |id: String| {
                let ctl = ctl.clone();
                let service = observed_service.clone();
                let run_id = observed_run.clone();
                Box::pin(async move {
                    *ctl.session.lock().await = id.clone();
                    if let Err(error) = service
                        .transaction(|inner| {
                            let run = find_run_mut(&mut inner.state, &run_id)
                                .ok_or_else(|| WorkflowError::io("schedule run missing"))?;
                            run["sessionId"] = json!(id);
                            Ok(())
                        })
                        .await
                    {
                        *ctl.storage_error.lock().await = Some(error);
                        ctl.cancellation.cancel();
                    }
                }) as BoxFuture<'static, ()>
            });
            let request = AgentRequest {
                session_id: String::new(),
                message: model::text(&task["prompt"], 100000),
                attachments: vec![],
                cwd: model::text(&task["cwd"], usize::MAX),
                title: format!("定时任务 · {}", model::text(&task["name"], 120)),
                model: task["model"].clone(),
                execution_mode: "full-access".into(),
                isolated_context: true,
                requested_tool_names: vec![],
                on_session: observer,
                cancellation: control.cancellation.clone(),
            };
            let executor = self.workflows.executor();
            let mut execution = executor.prompt(request);
            let result = tokio::select! {
                _ = control.cancellation.cancelled() => {
                    let session = control.session.lock().await.clone();
                    if session.is_empty() {
                        let _ = execution.await;
                    } else {
                        let _ = tokio::join!(execution, executor.abort(session));
                    }
                    Err(WorkflowError::coded("SCHEDULE_INTERRUPTED", "任务因 Pisper 关闭而中断。"))
                },
                result = &mut execution => result.map(|agent| (agent, String::new())),
            };
            if control.cancellation.is_cancelled() {
                Err(WorkflowError::coded(
                    "SCHEDULE_INTERRUPTED",
                    "任务因 Pisper 关闭而中断。",
                ))
            } else {
                result
            }
        };
        let result = match control.storage_error.lock().await.take() {
            Some(error) => Err(error),
            None => result,
        };
        let observed_session = control.session.lock().await.clone();
        let (status, summary, error, session, workflow_run) = match result {
            Ok((result, workflow_run)) => (
                "completed",
                if result.text.trim().is_empty() {
                    "任务已完成。".into()
                } else {
                    result.text.trim().chars().take(1200).collect::<String>()
                },
                String::new(),
                result.session_id,
                workflow_run,
            ),
            Err(error) => (
                if error.code == "SCHEDULE_INTERRUPTED" {
                    "interrupted"
                } else {
                    "failed"
                },
                String::new(),
                error.message,
                observed_session,
                String::new(),
            ),
        };
        let event = format!("schedule.{status}");
        let data = json!({"task":{"name":task["name"],"summary":summary,"error":error,"duration":duration_label(started.elapsed()),"nextRun":time::next_label(&task)}});
        let mut notification_error = None;
        if task["notifications"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
            && (status == "failed" || status == "completed" && task["notifyOn"] == "always")
        {
            notification_error = Some(
                self.workflows
                    .executor()
                    .notify(event, data, json!({"platforms":task["notifications"]}))
                    .await
                    .err()
                    .map(|error| error.message)
                    .unwrap_or_default(),
            );
        }
        let saved = self
            .transaction(|inner| {
                if let Some(run) = find_run_mut(&mut inner.state, &run_id) {
                    run["status"] = json!(status);
                    run["summary"] = json!(summary);
                    run["error"] = json!(error);
                    run["sessionId"] = json!(session);
                    if !workflow_run.is_empty() {
                        run["workflowRunId"] = json!(workflow_run);
                    }
                    run["finishedAt"] = json!(store::now());
                    run["durationMs"] = json!(started.elapsed().as_millis() as u64);
                    if let Some(error) = &notification_error {
                        run["notificationError"] = json!(error);
                    }
                }
                if let Some(task) = find_task_mut(&mut inner.state, id) {
                    task["lastStatus"] = json!(status);
                    if status == "completed" {
                        task["lastSummary"] = json!(summary);
                    }
                    task["lastError"] = json!(error);
                    task["updatedAt"] = json!(store::now());
                    if let Some(error) = &notification_error {
                        task["lastNotificationError"] = json!(error);
                    }
                }
                Ok(())
            })
            .await;
        if let Err(storage_error) = saved {
            let mut inner = self.inner.lock().await;
            inner.state["persistenceError"] =
                json!({"code":storage_error.code,"message":storage_error.message});
            if let Some(run) = find_run_mut(&mut inner.state, &run_id) {
                run["status"] = json!("failed");
                run["error"] = json!(storage_error.message);
                run["errorCode"] = json!(storage_error.code);
                run["finishedAt"] = json!(store::now());
            }
            if let Some(task) = find_task_mut(&mut inner.state, id) {
                task["lastStatus"] = json!("failed");
                task["lastError"] = json!(storage_error.message);
            }
            tracing::error!(run_id=%run_id,"Cannot persist terminal scheduled task state");
        }
        self.inner.lock().await.executions.remove(id);
        control.finished.store(true, Ordering::Release);
        control.done.notify_waiters();
    }
    async fn execute_workflow(
        &self,
        task: &Value,
        run_id: &str,
        control: &Execution,
    ) -> Result<(AgentResult, String)> {
        let id = task["workflowId"].as_str().unwrap_or("");
        let workflow = self
            .workflows
            .get_workflow(id)
            .await
            .ok_or_else(|| WorkflowError::invalid("选择的工作流不存在。"))?;
        if workflow["status"] != "published" {
            return Err(WorkflowError::invalid("定时任务只能调用已发布的工作流。"));
        }
        let run=self.workflows.run(id,json!({"trigger":"schedule","sourceMessage":task["name"],"inputs":task["workflowInputs"]})).await?.ok_or_else(||WorkflowError::invalid("选择的工作流不存在。"))?;
        let workflow_run = run["id"].as_str().unwrap_or("").to_owned();
        self.transaction(|inner| {
            if let Some(run) = find_run_mut(&mut inner.state, run_id) {
                run["workflowRunId"] = json!(workflow_run);
            }
            Ok(())
        })
        .await?;
        loop {
            let current = self
                .workflows
                .get_run(&workflow_run)
                .await
                .ok_or_else(|| WorkflowError::invalid("工作流运行记录不存在。"))?;
            if let Some(session) = current["sessionId"].as_str().filter(|id| !id.is_empty()) {
                let mut observed = control.session.lock().await;
                if *observed != session {
                    *observed = session.to_owned();
                    self.transaction(|inner| {
                        let run = find_run_mut(&mut inner.state, run_id)
                            .ok_or_else(|| WorkflowError::io("schedule run missing"))?;
                        run["sessionId"] = json!(session);
                        Ok(())
                    })
                    .await?;
                }
            }
            let status = current["status"].as_str().unwrap_or("");
            if ["completed", "failed", "cancelled", "interrupted"].contains(&status) {
                if status != "completed" {
                    return Err(WorkflowError::invalid(
                        current["error"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("工作流运行{status}。")),
                    ));
                }
                return Ok((
                    AgentResult {
                        text: current["summary"]
                            .as_str()
                            .unwrap_or("工作流已完成。")
                            .into(),
                        session_id: current["sessionId"].as_str().unwrap_or("").into(),
                        assets: vec![],
                    },
                    workflow_run,
                ));
            }
            tokio::select! {_=control.cancellation.cancelled()=>return Err(WorkflowError::coded("SCHEDULE_INTERRUPTED","任务因 Pisper 关闭而中断。")),_=tokio::time::sleep(Duration::from_millis(250))=>{}}
        }
    }
    pub(crate) async fn dispose(&self) -> Result<()> {
        self.shutdown.cancel();
        let controls = {
            let mut inner = self.inner.lock().await;
            inner.closed = true;
            inner.executions.values().cloned().collect::<Vec<_>>()
        };
        for control in &controls {
            control.cancellation.cancel();
        }
        for control in controls {
            let notified = control.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !control.finished.load(Ordering::Acquire) {
                notified.await;
            }
        }
        let timer_done = self.timer_done.notified();
        tokio::pin!(timer_done);
        timer_done.as_mut().enable();
        if !self.timer_finished.load(Ordering::Acquire) {
            timer_done.await;
        }
        Ok(())
    }
}
fn normalize(raw: &Value, cwd: &str) -> Result<Value> {
    let stamp = store::now();
    let frequency = raw["frequency"]
        .as_str()
        .filter(|f| ["interval", "daily", "weekly", "monthly"].contains(f))
        .unwrap_or("daily");
    let interval = raw["intervalValue"]
        .as_f64()
        .or_else(|| raw["intervalValue"].as_str().and_then(|s| s.parse().ok()))
        .filter(|v| v.is_finite() && *v != 0.0)
        .unwrap_or(1.0)
        .clamp(1.0, 10000.0);
    let day = raw["dayOfWeek"]
        .as_f64()
        .filter(|v| v.fract() == 0.0)
        .unwrap_or(1.0)
        .clamp(0.0, 6.0);
    let month = raw["dayOfMonth"]
        .as_f64()
        .filter(|v| v.is_finite() && *v != 0.0)
        .unwrap_or(1.0)
        .clamp(1.0, 28.0);
    let time = raw["time"]
        .as_str()
        .filter(|s| {
            let bytes = s.as_bytes();
            bytes.len() == 5
                && bytes[2] == b':'
                && bytes[0..2].iter().all(u8::is_ascii_digit)
                && bytes[3..5].iter().all(u8::is_ascii_digit)
                && s[0..2].parse::<u32>().is_ok_and(|h| h < 24)
                && s[3..5].parse::<u32>().is_ok_and(|m| m < 60)
        })
        .unwrap_or("09:00");
    let notifications = model::strings(&raw["notifications"], usize::MAX)
        .into_iter()
        .filter(|s| ["browser", "feishu", "weixin", "qq", "telegram"].contains(&s.as_str()))
        .collect::<Vec<_>>();
    let workflow_inputs = raw["workflowInputs"]
        .as_object()
        .map(|o| {
            o.iter()
                .take(100)
                .map(|(key, value)| (key.chars().take(120).collect::<String>(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    let mut task = json!({"id":raw["id"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).map(Ok).unwrap_or_else(store::id)?,"name":raw["name"].as_str().filter(|s|!s.is_empty()).unwrap_or("未命名任务").chars().take(120).collect::<String>(),"targetType":if raw["targetType"]=="workflow"{"workflow"}else{"prompt"},"prompt":raw["prompt"].as_str().unwrap_or("").chars().take(100000).collect::<String>(),"workflowId":raw["workflowId"].as_str().unwrap_or("").chars().take(200).collect::<String>(),"workflowInputs":workflow_inputs,"enabled":raw["enabled"]!=false,"frequency":frequency,"intervalValue":interval,"intervalUnit":raw["intervalUnit"].as_str().filter(|s|["minutes","hours","days"].contains(s)).unwrap_or("hours"),"time":time,"timezone":raw["timezone"].as_str().filter(|s|!s.is_empty()).unwrap_or("Asia/Hong_Kong"),"dayOfWeek":day,"dayOfMonth":month,"cwd":raw["cwd"].as_str().filter(|s|!s.is_empty()).unwrap_or(cwd),"executionMode":"full-access","model":model::model(&raw["model"]),"notifications":notifications,"notifyOn":if raw["notifyOn"]=="failure"{"failure"}else{"always"},"createdAt":raw.get("createdAt").cloned().unwrap_or(json!(stamp)),"updatedAt":raw.get("updatedAt").cloned().unwrap_or(json!(stamp)),"nextRunAt":Value::Null,"lastRunAt":raw.get("lastRunAt").cloned().unwrap_or(Value::Null),"lastStatus":raw.get("lastStatus").cloned().unwrap_or(json!("idle")),"lastSummary":model::text(&raw["lastSummary"],1200),"lastError":model::text(&raw["lastError"],1200),"lastNotificationError":model::text(&raw["lastNotificationError"],1200)});
    time::validate_timezone(&task)?;
    if task["enabled"] != false {
        task["nextRunAt"] = json!(time::next_run(&task, Utc::now())?);
    }
    Ok(task)
}
fn find_task_mut<'a>(state: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    state["tasks"]
        .as_array_mut()?
        .iter_mut()
        .find(|task| task["id"] == id)
}
fn find_run_mut<'a>(state: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    state["runs"]
        .as_array_mut()?
        .iter_mut()
        .find(|run| run["id"] == id)
}
fn missing() -> WorkflowError {
    WorkflowError {
        status: StatusCode::NOT_FOUND,
        code: "schedule_not_found".into(),
        message: "定时任务不存在。".into(),
        partial_output: None,
    }
}
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(service: Arc<ScheduleService>) -> Router<S> {
    Router::new()
        .route("/api/schedules", get(get_schedules).post(create_schedule))
        .route(
            "/api/schedules/{id}",
            patch(update_schedule).delete(delete_schedule),
        )
        .route("/api/schedules/{id}/run", post(run_schedule))
        .with_state(service)
}
async fn get_schedules(State(service): State<Arc<ScheduleService>>) -> Result<Json<Value>> {
    Ok(Json(service.dashboard().await?))
}
async fn create_schedule(
    State(service): State<Arc<ScheduleService>>,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let task = service.create(input).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"task":task,"state":service.dashboard().await?})),
    ))
}
async fn update_schedule(
    State(service): State<Arc<ScheduleService>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let task = service.update(&id, input).await?.ok_or_else(missing)?;
    Ok(Json(
        json!({"task":task,"state":service.dashboard().await?}),
    ))
}
async fn delete_schedule(
    State(service): State<Arc<ScheduleService>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    if !service.remove(&id).await? {
        return Err(missing());
    }
    Ok(Json(json!({"deleted":true})))
}
async fn run_schedule(
    State(service): State<Arc<ScheduleService>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    let task = service.run_now(&id).await?.ok_or_else(missing)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"started":true,"task":task})),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::{self, Executor, TempDirectory};
    async fn schedule(
        directory: &TempDirectory,
        executor: Arc<Executor>,
    ) -> (Arc<ScheduleService>, Arc<WorkflowService>) {
        let workflows = test_support::service(directory, executor).await;
        let service = ScheduleService::open(
            directory.path.join("schedules.json"),
            directory.path.to_string_lossy().into_owned(),
            workflows.clone(),
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        (service, workflows)
    }
    async fn wait_status(service: &ScheduleService, status: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let state = service.state().await;
                if state["runs"]
                    .as_array()
                    .is_some_and(|runs| runs.iter().any(|run| run["status"] == status))
                {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test(start_paused = true)]
    async fn owned_native_timer_executes_due_task_and_dispose_joins_timer() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let workflows = test_support::service(&directory, executor.clone()).await;
        let service = ScheduleService::open(
            directory.path.join("schedules.json"),
            directory.path.to_string_lossy().into_owned(),
            workflows.clone(),
            Duration::from_secs(15),
        )
        .await
        .unwrap();
        tokio::task::yield_now().await;
        let task=service.create(json!({"name":"real timer","prompt":"due","frequency":"interval","intervalValue":1,"intervalUnit":"minutes"})).await.unwrap();
        service
            .transaction(|inner| {
                find_task_mut(&mut inner.state, task["id"].as_str().unwrap()).unwrap()
                    ["nextRunAt"] = json!((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
                Ok(())
            })
            .await
            .unwrap();
        tokio::time::advance(Duration::from_secs(15)).await;
        for _ in 0..100 {
            if !executor.prompts.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(executor.prompts.lock().unwrap().len(), 1);
        for _ in 0..100 {
            if service.state().await["runs"][0]["status"] == "completed" {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(service.state().await["runs"][0]["trigger"], "scheduled");
        service.dispose().await.unwrap();
        assert!(service.timer_finished.load(Ordering::Acquire));
        assert!(service.inner.lock().await.executions.is_empty());
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn prompt_schedule_persists_selected_model_and_full_access_and_multiple_notifications() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let (service, workflows) = schedule(&directory, executor.clone()).await;
        let task=service.create(json!({"name":"daily","prompt":"test","frequency":"daily","time":"09:00","timezone":"UTC","executionMode":"read-only","model":{"provider":"fixture","model":"model"},"notifications":["browser","feishu"]})).await.unwrap();
        let next = task["nextRunAt"].clone();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        let state = wait_status(&service, "completed").await;
        assert_eq!(state["tasks"][0]["nextRunAt"], next);
        assert_eq!(state["runs"][0]["trigger"], "manual");
        assert_eq!(
            executor.prompts.lock().unwrap()[0]["model"],
            json!({"provider":"fixture","model":"model"})
        );
        assert_eq!(
            executor.prompts.lock().unwrap()[0]["executionMode"],
            "full-access"
        );
        assert_eq!(executor.prompts.lock().unwrap()[0]["isolatedContext"], true);
        assert_eq!(
            executor.notifications.lock().unwrap()[0]["options"]["platforms"],
            json!(["browser", "feishu"])
        );
        service.dispose().await.unwrap();
        let restored = ScheduleService::open(
            directory.path.join("schedules.json"),
            directory.path.to_string_lossy().into_owned(),
            workflows.clone(),
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        assert_eq!(restored.state().await["runs"][0]["status"], "completed");
        restored.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn due_schedule_ticks_once_without_overlap_and_cannot_delete_while_running() {
        let directory = TempDirectory::new();
        let releases = Arc::new(tokio::sync::Semaphore::new(0));
        let fixture_releases = releases.clone();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(move |request| {
                let releases = fixture_releases.clone();
                Box::pin(async move {
                    (request.on_session)("tick-session".into()).await;
                    let permit = releases.acquire().await.map_err(WorkflowError::io)?;
                    permit.forget();
                    Ok(AgentResult {
                        text: "tick done".into(),
                        session_id: "tick-session".into(),
                        assets: vec![],
                    })
                })
            })),
            ..Executor::default()
        });
        let (service, workflows) = schedule(&directory, executor.clone()).await;
        let task=service.create(json!({"name":"interval","prompt":"test","frequency":"interval","intervalUnit":"minutes","intervalValue":1})).await.unwrap();
        let id = task["id"].as_str().unwrap();
        let due = task["nextRunAt"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap();
        service.tick_at(due).await.unwrap();
        service.tick_at(due).await.unwrap();
        assert_eq!(
            service.run_now(id).await.unwrap_err().status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            service.remove(id).await.unwrap_err().status,
            StatusCode::CONFLICT
        );
        assert_eq!(service.state().await["runs"].as_array().unwrap().len(), 1);
        assert_eq!(service.state().await["runs"][0]["trigger"], "scheduled");
        assert!(
            service.state().await["tasks"][0]["nextRunAt"]
                .as_str()
                .unwrap()
                .parse::<DateTime<Utc>>()
                .unwrap()
                > due
        );
        releases.add_permits(1);
        wait_status(&service, "completed").await;
        assert!(service.remove(id).await.unwrap());
        assert!(service.state().await["runs"].as_array().unwrap().is_empty());
        service.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn notification_delivery_failure_is_recorded_without_changing_execution_success() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor {
            notification_failure: true,
            ..Executor::default()
        });
        let (service, workflows) = schedule(&directory, executor).await;
        let task = service
            .create(
                json!({"name":"notify","prompt":"test","enabled":false,"notifications":["weixin"]}),
            )
            .await
            .unwrap();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        let state = wait_status(&service, "completed").await;
        assert!(state["runs"][0]["notificationError"]
            .as_str()
            .unwrap()
            .contains("weixin"));
        assert_eq!(
            state["tasks"][0]["lastNotificationError"],
            state["runs"][0]["notificationError"]
        );
        assert_eq!(state["tasks"][0]["lastStatus"], "completed");
        service.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn failure_only_suppresses_success_notification_and_delivers_failed_template() {
        let directory = TempDirectory::new();
        let fail = Arc::new(AtomicBool::new(false));
        let fixture_fail = fail.clone();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(move |_| {
                let fail = fixture_fail.load(Ordering::SeqCst);
                Box::pin(async move {
                    if fail {
                        Err(WorkflowError::invalid("model failed"))
                    } else {
                        Ok(AgentResult {
                            text: "success".into(),
                            ..AgentResult::default()
                        })
                    }
                })
            })),
            ..Executor::default()
        });
        let (service, workflows) = schedule(&directory, executor.clone()).await;
        let task=service.create(json!({"name":"failure-only","prompt":"test","enabled":false,"notifications":["browser"],"notifyOn":"failure"})).await.unwrap();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        wait_status(&service, "completed").await;
        assert!(executor.notifications.lock().unwrap().is_empty());
        fail.store(true, Ordering::SeqCst);
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        let state = wait_status(&service, "failed").await;
        assert_eq!(state["tasks"][0]["lastError"], "model failed");
        assert_eq!(
            executor.notifications.lock().unwrap()[0]["event"],
            "schedule.failed"
        );
        assert_eq!(
            executor.notifications.lock().unwrap()[0]["data"]["task"]["error"],
            "model failed"
        );
        service.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn workflow_schedule_checks_publish_and_inputs_then_waits_for_actual_completion() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor::default());
        let (service, workflows) = schedule(&directory, executor).await;
        let workflow=workflows.create(json!({"name":"workflow","status":"published","inputs":[{"name":"environment","required":true}],"nodes":[{"id":"task","prompt":"{{inputs.environment}}"}]})).await.unwrap();
        let id = workflow["id"].as_str().unwrap();
        assert!(service.create(json!({"name":"invalid","targetType":"workflow","workflowId":id,"workflowInputs":{}})).await.is_err());
        let task=service.create(json!({"name":"workflow target","targetType":"workflow","workflowId":id,"workflowInputs":{"environment":"production"},"enabled":false})).await.unwrap();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        let state = wait_status(&service, "completed").await;
        let run_id = state["runs"][0]["workflowRunId"].as_str().unwrap();
        let run = workflows.get_run(run_id).await.unwrap();
        assert_eq!(run["trigger"], "schedule");
        assert_eq!(run["inputs"], json!({"environment":"production"}));
        assert_eq!(run["status"], "completed");
        assert_eq!(state["runs"][0]["summary"], run["summary"]);
        workflows
            .update(id, json!({"status":"draft"}))
            .await
            .unwrap();
        assert!(service.create(json!({"name":"draft","targetType":"workflow","workflowId":id,"workflowInputs":{"environment":"p"}})).await.is_err());
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        let state = wait_status(&service, "failed").await;
        assert!(state["tasks"][0]["lastError"]
            .as_str()
            .unwrap()
            .contains("已发布"));
        service.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn shutdown_interrupts_workflow_poll_and_owned_prompt_without_success_notification() {
        let directory = TempDirectory::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(Arc::new(|request| {
                Box::pin(async move {
                    (request.on_session)("schedule-wait".into()).await;
                    request.cancellation.cancelled().await;
                    Err(WorkflowError::cancelled())
                })
            })),
            ..Executor::default()
        });
        let (service, workflows) = schedule(&directory, executor.clone()).await;
        let workflow = workflows
            .create(
                json!({"name":"wait","status":"published","nodes":[{"id":"wait","prompt":"等待"}]}),
            )
            .await
            .unwrap();
        let task=service.create(json!({"name":"workflow wait","targetType":"workflow","workflowId":workflow["id"],"enabled":false,"notifications":["browser"]})).await.unwrap();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while service.state().await["runs"][0]["workflowRunId"] == "" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        service.dispose().await.unwrap();
        assert_eq!(service.state().await["runs"][0]["status"], "interrupted");
        assert!(executor.notifications.lock().unwrap().is_empty());
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn restart_marks_running_runs_and_tasks_interrupted_and_retains_history() {
        let directory = TempDirectory::new();
        let start = "2026-01-01T00:00:00.000Z";
        store::save_json(&directory.path.join("schedules.json"),&json!({"version":1,"tasks":[{"id":"task","name":"restart","prompt":"test","enabled":false,"lastStatus":"running","lastRunAt":start}],"runs":[{"id":"run","taskId":"task","status":"running","startedAt":start,"durationMs":0}]})).unwrap();
        let (service, workflows) = schedule(&directory, Arc::new(Executor::default())).await;
        let state = service.state().await;
        assert_eq!(state["tasks"][0]["lastStatus"], "interrupted");
        assert_eq!(state["runs"][0]["status"], "interrupted");
        assert!(state["runs"][0]["durationMs"].as_u64().unwrap() > 0);
        assert_eq!(state["tasks"][0]["nextRunAt"], Value::Null);
        let stored = store::read_json(&directory.path.join("schedules.json"), Value::Null).unwrap();
        assert_eq!(stored["runs"][0]["status"], "interrupted");
        service.dispose().await.unwrap();
        workflows.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn cooperative_schedule_shutdown_joins_prompt_finally_before_terminal() {
        let directory = TempDirectory::new();
        let lifecycle = test_support::PromptLifecycle::new();
        let executor = Arc::new(Executor {
            prompt_fixture: Some(lifecycle.fixture()),
            ..Executor::default()
        });
        let (service, workflows) = schedule(&directory, executor).await;
        let task = service
            .create(json!({"name":"owned shutdown","prompt":"run","enabled":false}))
            .await
            .unwrap();
        service.run_now(task["id"].as_str().unwrap()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), lifecycle.started.notified())
            .await
            .unwrap();
        assert_eq!(
            service.state().await["runs"][0]["sessionId"],
            "cooperative-session"
        );
        let closing = tokio::spawn({
            let service = service.clone();
            async move { service.dispose().await }
        });
        tokio::time::timeout(Duration::from_secs(3), lifecycle.cancelling.notified())
            .await
            .unwrap();
        assert!(!closing.is_finished());
        assert!(!lifecycle.dropped.load(Ordering::SeqCst));
        assert_eq!(service.state().await["runs"][0]["status"], "running");
        lifecycle.finish.add_permits(1);
        tokio::time::timeout(Duration::from_secs(3), closing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(service.state().await["runs"][0]["status"], "interrupted");
        assert_eq!(
            service.state().await["runs"][0]["sessionId"],
            "cooperative-session"
        );
        assert!(lifecycle.finally_finished.load(Ordering::SeqCst));
        assert!(lifecycle.dropped.load(Ordering::SeqCst));
        assert!(!lifecycle.dropped_before_finally.load(Ordering::SeqCst));
        workflows.dispose().await.unwrap();
    }
    #[tokio::test]
    async fn normalized_weekday_and_month_day_are_effective_after_roundtrip() {
        let raw = json!({"frequency":"weekly","dayOfWeek":4,"time":"09:00","timezone":"UTC"});
        let normalized = normalize(&raw, ".").unwrap();
        assert_eq!(
            time::next_run(&normalized, "2026-07-18T10:00:00Z".parse().unwrap()).unwrap(),
            "2026-07-23T09:00:00.000Z"
        );
        let monthly = normalize(
            &json!({"frequency":"monthly","dayOfMonth":28,"time":"09:00","timezone":"UTC"}),
            ".",
        )
        .unwrap();
        assert_eq!(
            time::next_run(&monthly, "2026-07-18T10:00:00Z".parse().unwrap()).unwrap(),
            "2026-07-28T09:00:00.000Z"
        );
        assert!(normalize(&json!({"timezone":"Mars/Nowhere"}), ".").is_err());
    }
}
