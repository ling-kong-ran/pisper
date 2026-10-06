//! 持久 Team 任务图。这里只认领任务、检查证据；实际执行由 AgentService 拥有。
use super::{
    agents::{task_name, AgentService, TaskGraph},
    persistence::{now, read, write},
    EventSink, SessionScope,
};
use crate::goal_api::{GoalService, TeamCoordinator};
use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

const LEASE_MS: i64 = 600_000;
pub(crate) const TEAM_EXECUTION_MARKER: &str = "[Pisper internal team execution]";
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Task {
    id: String,
    task_name: String,
    role: String,
    message: String,
    files: Vec<String>,
    depends_on: Vec<String>,
    agent_id: String,
    lease_id: String,
    claimed_at: Option<String>,
    lease_expires_at: Option<String>,
    blocked_reason: String,
    status: String,
    output: String,
    error: String,
    error_kind: String,
    attempts: u64,
    next_retry_at: Option<String>,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
    auto_start: bool,
    workflow_fingerprint: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Team {
    id: String,
    session_id: String,
    goal_id: String,
    objective: String,
    status: String,
    token_budget: Option<u64>,
    token_budget_explicit: bool,
    tasks: BTreeMap<String, Task>,
    conflicts: Vec<Value>,
    summary_text: String,
    script_path: String,
    communications: Vec<Value>,
    started_at: String,
    created_at: String,
    updated_at: String,
    last_progress_at: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Document {
    version: u32,
    teams: BTreeMap<String, Team>,
}
pub(crate) struct TeamService {
    path: PathBuf,
    document: Mutex<Document>,
    goals: Arc<GoalService>,
    events: EventSink,
    agents: Mutex<Weak<AgentService>>,
}
fn limited(s: &str, max: usize) -> String {
    let mut count = 0;
    s.chars()
        .take_while(|c| {
            count += c.len_utf16();
            count <= max
        })
        .collect()
}
fn terminal(s: &str) -> bool {
    matches!(s, "completed" | "failed" | "interrupted")
}
fn timestamp(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}
fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn dependencies_complete(team: &Team, task: &Task) -> bool {
    task.depends_on.iter().all(|dep| {
        team.tasks
            .values()
            .any(|t| t.task_name == *dep && t.status == "completed")
    })
}
fn unblock(team: &mut Team) {
    let names = team
        .tasks
        .values()
        .filter(|t| t.status == "completed")
        .map(|t| t.task_name.clone())
        .collect::<HashSet<_>>();
    for task in team.tasks.values_mut() {
        if matches!(task.status.as_str(), "queued" | "blocked") {
            task.status = if task.depends_on.iter().all(|d| names.contains(d)) {
                "queued"
            } else {
                "blocked"
            }
            .into();
            task.blocked_reason = if task.status == "blocked" {
                "等待依赖完成后自动认领"
            } else {
                ""
            }
            .into();
        }
    }
}
fn clear_lease(task: &mut Task) {
    task.lease_id.clear();
    task.claimed_at = None;
    task.lease_expires_at = None;
}
fn stop_tasks(team: &mut Team, reason: &str) {
    let at = now();
    for task in team.tasks.values_mut().filter(|t| !terminal(&t.status)) {
        task.status = "interrupted".into();
        task.error = limited(reason, 1000);
        task.error_kind = "user_cancelled".into();
        task.next_retry_at = None;
        task.completed_at = Some(at.clone());
        task.updated_at = at.clone();
        clear_lease(task);
    }
    team.updated_at = at.clone();
    team.last_progress_at = at;
}
fn error_kind(error: &str, status: &str) -> String {
    let text = error.to_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|t| text.contains(t));
    if has(&[
        "team_workflow_communication_interrupted",
        "workflow worker",
        "communication stream",
        "worker exited",
    ]) {
        "agent_communication"
    } else if has(&[
        "401",
        "403",
        "invalid api key",
        "invalid token",
        "authentication",
        "unauthorized",
        "forbidden",
    ]) {
        "authentication"
    } else if has(&[
        "stream_read_error",
        "econnreset",
        "connection reset",
        "connection closed",
        "connection lost",
        "premature",
        "incomplete",
        "too many pending",
        "rate limit",
        "retry",
        "upstream closed",
    ]) {
        "upstream_stream"
    } else if status == "interrupted" || has(&["abort", "cancel", "取消", "中断"]) {
        "user_cancelled"
    } else if error.trim().is_empty() {
        ""
    } else {
        "execution"
    }
    .into()
}
fn retry(task: &mut Task, at: i64) {
    task.next_retry_at = if terminal(&task.status)
        && task.status != "completed"
        && matches!(
            task.error_kind.as_str(),
            "upstream_stream" | "agent_communication"
        ) {
        Some(iso(at
            + (30_000i64.saturating_mul(
                1i64.checked_shl(task.attempts.saturating_sub(1).min(20) as u32)
                    .unwrap_or(i64::MAX),
            ))
            .min(600_000)))
    } else {
        None
    };
}
fn strings(value: &Value, max: usize, chars: usize, files: bool) -> Result<Vec<String>> {
    if value.is_null() {
        return Ok(vec![]);
    }
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("Team files and dependencies must be arrays."))?;
    if values.len() > max {
        bail!("Team list exceeds its {max} item limit.")
    }
    let mut result = vec![];
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| anyhow!("Team list entries must be strings."))?;
        let item = if files {
            limited(
                raw.trim()
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .trim_end_matches('/'),
                chars,
            )
        } else {
            task_name(raw)
        };
        if !item.is_empty() && !result.contains(&item) {
            result.push(item)
        }
    }
    Ok(result)
}
fn overlaps(a: &[String], b: &[String]) -> bool {
    a.iter().any(|a| {
        b.iter()
            .any(|b| a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/")))
    })
}
fn validate_graph(team: &Team, candidate: &Task) -> Result<()> {
    if team
        .tasks
        .values()
        .any(|t| t.id != candidate.id && t.task_name == candidate.task_name)
    {
        bail!("Team task already exists: {}.", candidate.task_name)
    }
    for dep in &candidate.depends_on {
        if dep == &candidate.task_name {
            bail!("Team task cannot depend on itself.")
        }
        if !team
            .tasks
            .values()
            .any(|t| t.id != candidate.id && t.task_name == *dep)
        {
            bail!("Unknown Team task dependency: {dep}.")
        }
    }
    let mut graph = team
        .tasks
        .values()
        .map(|t| (t.task_name.clone(), t.depends_on.clone()))
        .collect::<BTreeMap<_, _>>();
    graph.insert(candidate.task_name.clone(), candidate.depends_on.clone());
    fn visit(
        name: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut HashSet<String>,
        done: &mut HashSet<String>,
    ) -> bool {
        if done.contains(name) {
            return false;
        }
        if !visiting.insert(name.into()) {
            return true;
        }
        if graph
            .get(name)
            .is_some_and(|deps| deps.iter().any(|d| visit(d, graph, visiting, done)))
        {
            return true;
        }
        visiting.remove(name);
        done.insert(name.into());
        false
    }
    if visit(
        &candidate.task_name,
        &graph,
        &mut HashSet::new(),
        &mut HashSet::new(),
    ) {
        bail!("Team task dependency cycle detected.")
    }
    if let Some(other) = team.tasks.values().find(|t| {
        t.id != candidate.id
            && !terminal(&t.status)
            && !candidate.depends_on.contains(&t.task_name)
            && overlaps(&candidate.files, &t.files)
    }) {
        bail!("Team file ownership conflict with {}.", other.task_name)
    }
    Ok(())
}
fn public(team: &Team, goal: Option<crate::goal_api::Goal>) -> Value {
    let tasks = team
        .tasks
        .values()
        .map(|t| serde_json::to_value(t).expect("team task"))
        .collect::<Vec<_>>();
    let completed = team
        .tasks
        .values()
        .filter(|t| t.status == "completed")
        .collect::<Vec<_>>();
    let blockers=team.tasks.values().filter(|t|t.status!="completed").map(|t|json!({"taskName":t.task_name,"status":t.status,"reason":if !t.error.is_empty(){t.error.as_str()}else if !t.blocked_reason.is_empty(){t.blocked_reason.as_str()}else{"工作流尚未完成"}})).collect::<Vec<_>>();
    let mut value = serde_json::to_value(team).expect("team JSON");
    value["tasks"] = json!(tasks);
    value["taskCount"] = json!(team.tasks.len());
    value["completedTaskCount"] = json!(completed.len());
    value["activeTaskCount"] = json!(team.tasks.values().filter(|t| !terminal(&t.status)).count());
    value["elapsedMs"] = json!(millis().saturating_sub(timestamp(&team.started_at)).max(0));
    value["stalled"] = json!(team.status == "stalled");
    value["blockers"] = json!(blockers);
    value["summary"] = json!({"text":team.summary_text,"completed":completed.into_iter().map(|t|json!({"taskName":t.task_name,"role":t.role,"output":t.output})).collect::<Vec<_>>()});
    if let Some(goal) = goal {
        value["tokenUsed"] = json!(goal.tokens_used);
        value["tokenBudget"] = json!(goal.team_token_budget);
    }
    if let Some(obj) = value.as_object_mut() {
        obj.remove("summaryText");
    }
    value
}
impl TeamService {
    pub(crate) fn new(
        path: PathBuf,
        goals: Arc<GoalService>,
        events: EventSink,
        pause_active: bool,
    ) -> Result<Arc<Self>> {
        let mut document = read(&path)?
            .map(serde_json::from_value::<Document>)
            .transpose()?
            .unwrap_or_default();
        document.version = 1;
        document
            .teams
            .retain(|id, t| !id.trim().is_empty() && !t.id.trim().is_empty());
        let mut changed = false;
        for (id, team) in document.teams.iter_mut() {
            team.session_id = id.clone();
            if !team.token_budget_explicit {
                team.token_budget = None;
                if team.status == "budget_limited" {
                    team.status = "paused".into();
                    changed = true;
                }
            }
            if pause_active && team.status == "active" {
                team.status = "paused".into();
                stop_tasks(team, "Runtime restarted; explicit Team resume is required.");
                changed = true;
            }
            for task in team.tasks.values_mut() {
                task.task_name = task_name(&task.task_name);
                task.message = limited(&task.message, 12000);
                task.output = limited(&task.output, 12000);
            }
        }
        if changed {
            write(&path, &serde_json::to_value(&document)?)?;
        }
        Ok(Arc::new(Self {
            path,
            document: Mutex::new(document),
            goals,
            events,
            agents: Mutex::new(Weak::new()),
        }))
    }
    pub(crate) fn attach_agents(&self, agents: Weak<AgentService>) {
        *self.agents.lock().expect("Team executor") = agents;
    }
    fn agents(&self) -> Result<Arc<AgentService>> {
        self.agents
            .lock()
            .expect("Team executor")
            .upgrade()
            .ok_or_else(|| anyhow!("Team Agent executor is unavailable."))
    }
    fn commit<T>(&self, id: &str, operation: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
        let mut document = self.document.lock().expect("Team state");
        let mut candidate = document.clone();
        let result = operation(&mut candidate)?;
        write(&self.path, &serde_json::to_value(&candidate)?)?;
        *document = candidate;
        let team = document.teams.get(id).cloned();
        drop(document);
        if let Some(team) = team {
            (self.events)(
                "team_update",
                &json!({"sessionId":id,"team":public(&team,self.goals.get(id))}),
            );
        }
        Ok(result)
    }
    pub(crate) fn get(&self, id: &str) -> Option<Value> {
        self.document
            .lock()
            .expect("Team state")
            .teams
            .get(id)
            .map(|t| public(t, self.goals.get(id)))
    }
    pub(crate) fn projection(&self, parent: &str) -> Option<Value> {
        self.goals
            .get(parent)
            .filter(|g| g.mode == "team")
            .and_then(|_| self.get(parent))
    }
    pub(crate) fn task(&self, id: &str, target: &str) -> Option<Value> {
        self.document
            .lock()
            .expect("Team state")
            .teams
            .get(id)
            .and_then(|t| {
                t.tasks
                    .values()
                    .find(|t| t.id == target || t.task_name == target)
            })
            .map(|t| serde_json::to_value(t).expect("Team task"))
    }
    pub(crate) fn ensure(&self, id: &str) -> Result<Value> {
        let goal = self
            .goals
            .get(id)
            .filter(|g| g.mode == "team" && g.status == "active")
            .ok_or_else(|| anyhow!("No active Team goal is available."))?;
        self.commit(id, |doc| {
            let existing = doc
                .teams
                .get(id)
                .filter(|t| {
                    t.goal_id == goal.id
                        && matches!(
                            t.status.as_str(),
                            "active" | "paused" | "stalled" | "budget_limited"
                        )
                })
                .cloned();
            let mut team = existing.unwrap_or_else(|| {
                let at = now();
                Team {
                    id: crate::product::new_id(),
                    session_id: id.into(),
                    goal_id: goal.id.clone(),
                    objective: goal.objective.clone(),
                    status: "active".into(),
                    created_at: at.clone(),
                    updated_at: at.clone(),
                    last_progress_at: at.clone(),
                    started_at: at,
                    ..Default::default()
                }
            });
            team.objective = goal.objective.clone();
            team.token_budget = goal.team_token_budget;
            team.token_budget_explicit = goal.team_token_budget_explicit;
            let resuming = team.status != "active";
            team.status = "active".into();
            if resuming {
                for task in team
                    .tasks
                    .values_mut()
                    .filter(|t| t.status == "interrupted")
                {
                    task.status = "queued".into();
                    task.agent_id.clear();
                    task.error.clear();
                    task.error_kind.clear();
                    task.completed_at = None;
                    clear_lease(task);
                    task.next_retry_at = None;
                }
            }
            unblock(&mut team);
            if resuming {
                let mut tasks = team
                    .tasks
                    .values()
                    .filter(|t| !terminal(&t.status))
                    .collect::<Vec<_>>();
                tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                for (index, task) in tasks.iter().enumerate() {
                    if let Some(prior) = tasks[..index].iter().find(|prior| {
                        !task.depends_on.contains(&prior.task_name)
                            && overlaps(&task.files, &prior.files)
                    }) {
                        bail!(
                            "Team file ownership conflict while resuming {} with {}.",
                            task.task_name,
                            prior.task_name
                        );
                    }
                }
            }
            team.updated_at = now();
            let value = public(&team, Some(goal));
            doc.teams.insert(id.into(), team);
            Ok(value)
        })
    }
    pub(crate) fn check_complete(&self, id: &str) -> Result<()> {
        let document = self.document.lock().expect("Team state");
        let team = document
            .teams
            .get(id)
            .filter(|t| t.status == "active")
            .ok_or_else(|| anyhow!("No active Team is available."))?;
        let incomplete = team
            .tasks
            .values()
            .filter(|t| t.status != "completed")
            .map(|t| format!("{} ({})", t.task_name, t.status))
            .collect::<Vec<_>>();
        if !incomplete.is_empty() {
            bail!("Team still has unfinished work: {}. Use followup_task or restart the same taskName before completing the goal.",incomplete.join(", "))
        }
        if let Some(task) = team.tasks.values().find(|t| t.output.trim().is_empty()) {
            bail!(
                "Team completed task lacks verifiable output: {}.",
                task.task_name
            )
        }
        Ok(())
    }
    pub(crate) fn mark_complete(&self, id: &str) -> Result<()> {
        self.check_complete(id)?;
        self.commit(id, |d| {
            let t = d
                .teams
                .get_mut(id)
                .ok_or_else(|| anyhow!("No Team available."))?;
            t.status = "complete".into();
            t.updated_at = now();
            Ok(())
        })
    }
    pub(crate) fn stop_state(&self, id: &str, reason: &str) -> Result<()> {
        let budget = self
            .goals
            .get(id)
            .is_some_and(|g| g.status == "budget_limited");
        self.commit(id, |d| {
            if let Some(team) = d.teams.get_mut(id) {
                team.status = if budget { "budget_limited" } else { "paused" }.into();
                stop_tasks(team, reason);
            }
            Ok(())
        })
    }
    pub(crate) fn set_script_path(&self, id: &str, path: &str) -> Result<()> {
        self.commit(id, |d| {
            let team = d
                .teams
                .get_mut(id)
                .ok_or_else(|| anyhow!("No Team available."))?;
            team.script_path = limited(path, 240);
            team.updated_at = now();
            Ok(())
        })
    }
    pub(crate) fn set_summary(&self, id: &str, summary: &str) -> Result<()> {
        self.commit(id, |d| {
            let team = d
                .teams
                .get_mut(id)
                .ok_or_else(|| anyhow!("No Team available."))?;
            team.summary_text = limited(summary.trim(), 12000);
            team.updated_at = now();
            Ok(())
        })
    }
    pub(crate) fn resolve(
        &self,
        id: &str,
        target: &str,
        output: &str,
        status: &str,
    ) -> Result<Value> {
        if !terminal(status) {
            bail!("Team task can only be resolved to a terminal status.")
        }
        self.commit(id, |d| {
            let team = d
                .teams
                .get_mut(id)
                .ok_or_else(|| anyhow!("No Team available."))?;
            let task = team
                .tasks
                .values_mut()
                .find(|t| t.id == target || t.task_name == target)
                .ok_or_else(|| anyhow!("Unknown Team task."))?;
            if matches!(task.status.as_str(), "starting" | "running") {
                bail!("Interrupt the actual Agent before resolving a running Team task.")
            }
            task.status = status.into();
            task.agent_id.clear();
            clear_lease(task);
            task.next_retry_at = None;
            if !output.is_empty() {
                task.output = limited(output, 12000);
            }
            task.error = if status == "completed" {
                String::new()
            } else {
                limited(output, 1000)
            };
            task.error_kind = error_kind(&task.error, status);
            task.completed_at = Some(now());
            task.updated_at = now();
            let value = serde_json::to_value(&*task)?;
            unblock(team);
            Ok(value)
        })
    }
    /// 真实延续/恢复时调度。不会因读取状态或启动进程而隐式恢复用户暂停的 Team。
    pub(crate) async fn schedule(self: &Arc<Self>, id: &str) -> Result<()> {
        self.ensure(id)?;
        let agents = self.agents()?;
        let expired = self.commit(id, |d| {
            let team = d
                .teams
                .get_mut(id)
                .ok_or_else(|| anyhow!("No Team available."))?;
            let at = millis();
            let mut expired = vec![];
            for task in team.tasks.values_mut() {
                let lease_expired = matches!(task.status.as_str(), "starting" | "running")
                    && task
                        .lease_expires_at
                        .as_ref()
                        .is_some_and(|s| timestamp(s) <= at);
                let retry_due = matches!(task.status.as_str(), "failed" | "interrupted")
                    && matches!(
                        task.error_kind.as_str(),
                        "upstream_stream" | "agent_communication"
                    )
                    && task
                        .next_retry_at
                        .as_ref()
                        .is_none_or(|s| timestamp(s) <= at);
                if lease_expired || retry_due {
                    if lease_expired && !task.agent_id.is_empty() {
                        expired.push(task.agent_id.clone());
                    }
                    task.status = "queued".into();
                    task.agent_id.clear();
                    clear_lease(task);
                    task.next_retry_at = None;
                    task.error.clear();
                    task.error_kind.clear();
                    task.completed_at = None;
                    task.updated_at = now();
                }
            }
            unblock(team);
            Ok(expired)
        })?;
        for agent in expired {
            agents
                .interrupt(id, &agent, "Team task lease expired.")
                .await?;
        }
        let ready = {
            let d = self.document.lock().expect("Team state");
            let t = d.teams.get(id).expect("ensured Team");
            t.tasks
                .values()
                .filter(|task| {
                    task.auto_start && task.status == "queued" && task.agent_id.is_empty()
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for task in ready {
            agents.spawn(id.into(),json!({"teamTaskId":task.id,"taskName":task.task_name,"role":task.role,"message":task.message,"files":task.files,"dependsOn":task.depends_on,"autoStart":true,"workflowFingerprint":task.workflow_fingerprint})).await?;
        }
        agents.pump()?;
        Ok(())
    }
    pub(crate) fn execution_prompt(&self, id: &str) -> Result<String> {
        let goal = self
            .goals
            .get(id)
            .ok_or_else(|| anyhow!("No Team goal available."))?;
        let state = self.get(id).unwrap_or(Value::Null);
        Ok(format!("{TEAM_EXECUTION_MARKER}\nYou are the lead of a dynamic team. The objective is a mission requiring concrete implementation and evidence, not a one-turn answer.\n<team_objective>\n{}\n</team_objective>\nMaintain a concrete Plan; delegate bounded tasks only where useful. Supply files and dependencies so the runtime enforces ownership. Integrate and verify member results. Do not mark the goal complete while tasks are unfinished or evidence is missing. Member communication tools support direct handoffs. The parent owns final decisions and the completion audit. For visible fan-out and aggregation, run a workspace JavaScript workflow through run_team_workflow.\nCurrent task graph: {}",goal.objective,serde_json::to_string(&state["tasks"])?))
    }
    fn claim(&self, parent: &str, target: &str, agent: Option<&str>) -> Result<bool> {
        self.commit(parent, |d| {
            let Some(team) = d.teams.get_mut(parent).filter(|t| t.status == "active") else {
                return Ok(false);
            };
            let Some(task) = team.tasks.get(target).cloned() else {
                return Ok(false);
            };
            if !matches!(task.status.as_str(), "queued" | "blocked")
                || !dependencies_complete(team, &task)
                || agent.is_some_and(|agent| task.agent_id != agent)
            {
                return Ok(false);
            }
            if let Some(other) = team.tasks.values().find(|other| {
                other.id != task.id
                    && matches!(other.status.as_str(), "starting" | "running")
                    && overlaps(&task.files, &other.files)
            }) {
                bail!(
                    "Team file ownership conflict with running task {}.",
                    other.task_name
                );
            }
            let task = team.tasks.get_mut(target).expect("Team task");
            task.status = "starting".into();
            task.lease_id = crate::product::new_id();
            let at = millis();
            task.claimed_at = Some(iso(at));
            task.lease_expires_at = Some(iso(at + LEASE_MS));
            task.blocked_reason.clear();
            task.updated_at = iso(at);
            Ok(true)
        })
    }
}
impl TaskGraph for TeamService {
    fn register(&self, parent: &SessionScope, input: &Value) -> Result<String> {
        self.ensure(&parent.session_id)?;
        let id = parent.session_id.as_str();
        self.commit(id, |d| {
            let team = d.teams.get_mut(id).expect("ensured Team");
            if let Some(target) = input["teamTaskId"].as_str().filter(|s| !s.is_empty()) {
                let task = team
                    .tasks
                    .get(target)
                    .ok_or_else(|| anyhow!("Unknown persisted Team task."))?;
                if !matches!(task.status.as_str(), "queued" | "blocked")
                    || !task.agent_id.is_empty()
                {
                    bail!("Team task is already claimed or not ready.")
                }
                return Ok(target.into());
            }
            let name = task_name(input["taskName"].as_str().unwrap_or_default());
            if name.is_empty() {
                bail!("Team taskName cannot be empty.")
            }
            if team.tasks.len() >= 64 {
                bail!("Team task limit reached (64).")
            }
            let at = now();
            let mut task = Task {
                id: crate::product::new_id(),
                task_name: name,
                role: limited(input["role"].as_str().unwrap_or_default().trim(), 80),
                message: limited(input["message"].as_str().unwrap_or_default().trim(), 12000),
                files: strings(&input["files"], 96, 240, true)?,
                depends_on: strings(&input["dependsOn"], 32, 48, false)?,
                created_at: at.clone(),
                updated_at: at.clone(),
                status: "queued".into(),
                auto_start: input["autoStart"].as_bool().unwrap_or(true),
                workflow_fingerprint: limited(
                    input["workflowFingerprint"].as_str().unwrap_or_default(),
                    64,
                ),
                ..Default::default()
            };
            validate_graph(team, &task)?;
            if !dependencies_complete(team, &task) {
                task.status = "blocked".into();
                task.blocked_reason = "等待依赖完成后自动认领".into();
            }
            let task_id = task.id.clone();
            team.tasks.insert(task_id.clone(), task);
            team.updated_at = at.clone();
            team.last_progress_at = at;
            Ok(task_id)
        })
    }
    fn ready(&self, parent: &str, target: &str) -> Result<bool> {
        self.claim(parent, target, None)
    }
    fn ready_agent(&self, parent: &str, target: &str, agent: &str) -> Result<bool> {
        self.claim(parent, target, Some(agent))
    }
    fn bind(&self, parent: &str, target: &str, agent: &Value) -> Result<()> {
        self.commit(parent, |d| {
            let team = d
                .teams
                .get_mut(parent)
                .ok_or_else(|| anyhow!("No Team available."))?;
            let task = team
                .tasks
                .get_mut(target)
                .ok_or_else(|| anyhow!("No Team task available."))?;
            if !task.agent_id.is_empty()
                && task.agent_id != agent["id"].as_str().unwrap_or_default()
            {
                bail!("Team task has already been bound to another actual Agent.")
            }
            task.agent_id = agent["id"].as_str().unwrap_or_default().into();
            task.attempts += 1;
            task.updated_at = now();
            Ok(())
        })
    }
    fn update_agent(&self, parent: &str, agent: &Value) -> Result<()> {
        self.commit(parent, |d| {
            let Some(team) = d.teams.get_mut(parent) else {
                return Ok(());
            };
            let Some(task) = team.tasks.values_mut().find(|t| t.agent_id == agent["id"]) else {
                return Ok(());
            };
            // 停止或租约换代后，旧 Agent 的迟到输出不得覆盖新任务状态。
            if team.status != "active"
                || task.lease_id.is_empty() && matches!(task.status.as_str(), "queued" | "blocked")
            {
                return Ok(());
            }
            let status = agent["status"].as_str().unwrap_or(&task.status);
            if matches!(
                status,
                "queued" | "starting" | "running" | "completed" | "failed" | "interrupted"
            ) {
                task.status = status.into();
            }
            task.output = limited(agent["output"].as_str().unwrap_or_default(), 12000);
            task.error = limited(agent["error"].as_str().unwrap_or_default(), 1000);
            task.error_kind = error_kind(&task.error, &task.status);
            let at = millis();
            task.updated_at = iso(at);
            if terminal(&task.status) {
                task.completed_at = Some(iso(at));
                clear_lease(task);
            } else if matches!(task.status.as_str(), "starting" | "running")
                && !task.lease_id.is_empty()
            {
                task.lease_expires_at = Some(iso(at + LEASE_MS));
            }
            retry(task, at);
            if task.status == "running" {
                for entry in team.communications.iter_mut().filter(|entry| {
                    entry["toAgentId"] == agent["id"] && entry["status"] == "queued"
                }) {
                    entry["status"] = json!("delivered");
                    entry["deliveredAt"] = json!(iso(at));
                }
            }
            team.updated_at = iso(at);
            team.last_progress_at = iso(at);
            unblock(team);
            Ok(())
        })
    }
    fn update_task(&self, parent: &str, target: &str, input: &Value) -> Result<Value> {
        self.commit(parent, |d| {
            let team = d
                .teams
                .get_mut(parent)
                .filter(|t| t.status == "active")
                .ok_or_else(|| anyhow!("No active Team task available."))?;
            let mut task = team
                .tasks
                .values()
                .find(|t| t.id == target || t.task_name == target)
                .cloned()
                .ok_or_else(|| anyhow!("Unknown Team task."))?;
            if matches!(task.status.as_str(), "starting" | "running") {
                bail!("Running Team tasks must be interrupted before changing their graph entry.")
            }
            let old_name = task.task_name.clone();
            if let Some(name) = input["taskName"].as_str() {
                task.task_name = task_name(name);
                if task.task_name.is_empty() {
                    bail!("Team taskName cannot be empty.")
                }
            }
            if input.get("files").is_some() {
                task.files = strings(&input["files"], 96, 240, true)?
            }
            if input.get("dependsOn").is_some() {
                task.depends_on = strings(&input["dependsOn"], 32, 48, false)?
            }
            let mut graph = team.clone();
            if task.task_name != old_name {
                graph.tasks.remove(&task.id);
                for other in graph.tasks.values_mut() {
                    for dep in &mut other.depends_on {
                        if *dep == old_name {
                            *dep = task.task_name.clone();
                        }
                    }
                }
            }
            validate_graph(&graph, &task)?;
            if let Some(v) = input["role"].as_str() {
                task.role = limited(v.trim(), 80)
            }
            if let Some(v) = input["message"].as_str() {
                task.message = limited(v.trim(), 12000)
            }
            if let Some(v) = input["workflowFingerprint"].as_str() {
                task.workflow_fingerprint = limited(v, 64);
            }
            if let Some(v) = input["autoStart"].as_bool() {
                task.auto_start = v;
            }
            task.agent_id.clear();
            clear_lease(&mut task);
            task.next_retry_at = None;
            task.output.clear();
            task.error.clear();
            task.error_kind.clear();
            task.completed_at = None;
            task.status = "queued".into();
            task.updated_at = now();
            if old_name != task.task_name {
                for other in team.tasks.values_mut() {
                    for dep in &mut other.depends_on {
                        if *dep == old_name {
                            *dep = task.task_name.clone()
                        }
                    }
                }
            }
            let task_id = task.id.clone();
            team.tasks.insert(task_id.clone(), task);
            unblock(team);
            team.updated_at = now();
            Ok(serde_json::to_value(&team.tasks[&task_id])?)
        })
    }
    fn projection(&self, parent: &str) -> Option<Value> {
        TeamService::projection(self, parent)
    }
    fn communication(&self, parent: &str, sender: &str, target: &str, text: &str) -> Result<()> {
        self.commit(parent,|d|{let team=d.teams.get_mut(parent).filter(|t|t.status=="active").ok_or_else(||anyhow!("No active Team available."))?;
        let from=team.tasks.values().find(|t|t.agent_id==sender).ok_or_else(||anyhow!("Sender is not an active Team member."))?;let to=team.tasks.values().find(|t|t.agent_id==target).ok_or_else(||anyhow!("Recipient is not a Team member."))?;
        let entry=json!({"id":crate::product::new_id(),"fromAgentId":sender,"fromTaskName":from.task_name,"toAgentId":target,"toTaskName":to.task_name,"message":limited(text,12000),"status":if matches!(to.status.as_str(),"queued"|"blocked"|"starting"){"queued"}else{"delivered"},"sentAt":now()});team.communications.push(entry);if team.communications.len()>128{team.communications.drain(..team.communications.len()-128);}team.updated_at=now();Ok(())})
    }
    fn registration_failed(&self, parent: &str, target: &str, reason: &str) -> Result<()> {
        self.commit(parent, |d| {
            if let Some(task) = d
                .teams
                .get_mut(parent)
                .and_then(|t| t.tasks.get_mut(target))
            {
                if task.agent_id.is_empty() {
                    task.status = "failed".into();
                    task.error = limited(reason, 1000);
                    task.error_kind = error_kind(reason, "failed");
                    task.completed_at = Some(now());
                    task.updated_at = now();
                    clear_lease(task);
                    retry(task, millis());
                }
            }
            Ok(())
        })
    }
}
/// Arc wrapper 让异步协调能持有服务，服务内部只弱引用实际 AgentService，避免循环。
pub(crate) struct Coordinator(pub(crate) Arc<TeamService>);
impl TeamCoordinator for Coordinator {
    fn continuation(&self, session: String) -> BoxFuture<'static, Result<String>> {
        let team = self.0.clone();
        Box::pin(async move {
            team.schedule(&session).await?;
            team.execution_prompt(&session)
        })
    }
    fn check_complete(&self, session: String) -> BoxFuture<'static, Result<()>> {
        let team = self.0.clone();
        Box::pin(async move { team.check_complete(&session) })
    }
    fn mark_complete(&self, session: String) -> BoxFuture<'static, Result<()>> {
        let team = self.0.clone();
        Box::pin(async move { team.mark_complete(&session) })
    }
    fn stop(&self, session: String, reason: String) -> BoxFuture<'static, Result<()>> {
        let team = self.0.clone();
        Box::pin(async move {
            team.stop_state(&session, &reason)?;
            team.agents()?
                .abort_parent(&session, &reason)
                .await
                .map(|_| ())
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::session_workers::{
        ChildRequest, InputKind, PromptRequest, RunOutcome, SessionExecutor,
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use tokio::sync::{watch, Semaphore};
    pub(crate) struct Fixture {
        pub(crate) permits: Arc<Semaphore>,
        pub(crate) starts: Arc<Mutex<Vec<String>>>,
        pub(crate) contexts: Arc<Mutex<Vec<ChildRequest>>>,
        inputs: Arc<Mutex<Vec<(String, String)>>>,
        signals: Arc<Mutex<BTreeMap<String, watch::Sender<bool>>>>,
        disposed: Arc<AtomicUsize>,
    }
    impl Fixture {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                permits: Arc::new(Semaphore::new(0)),
                starts: Arc::new(Mutex::new(vec![])),
                contexts: Arc::new(Mutex::new(vec![])),
                inputs: Arc::new(Mutex::new(vec![])),
                signals: Arc::new(Mutex::new(BTreeMap::new())),
                disposed: Arc::new(AtomicUsize::new(0)),
            })
        }
    }
    impl SessionExecutor for Fixture {
        fn scope(&self, id: String) -> BoxFuture<'static, Result<SessionScope>> {
            Box::pin(async move {
                Ok(SessionScope {
                    session_id: id,
                    cwd: std::env::temp_dir().to_string_lossy().into(),
                    model: "fixture/model".into(),
                    tool_names: vec!["read".into(), "write".into()],
                    ..Default::default()
                })
            })
        }
        fn create_child(&self, r: ChildRequest) -> BoxFuture<'static, Result<SessionScope>> {
            let signals = self.signals.clone();
            let contexts = self.contexts.clone();
            Box::pin(async move {
                let (tx, _) = watch::channel(false);
                signals.lock().unwrap().insert(r.id.clone(), tx);
                contexts.lock().unwrap().push(r.clone());
                Ok(SessionScope {
                    session_id: r.id,
                    parent_session_id: Some(r.parent.session_id),
                    ..Default::default()
                })
            })
        }
        fn prompt(
            &self,
            r: PromptRequest,
            events: EventSink,
        ) -> BoxFuture<'static, Result<RunOutcome>> {
            let starts = self.starts.clone();
            let permits = self.permits.clone();
            let mut signal = self.signals.lock().unwrap()[&r.session_id].subscribe();
            Box::pin(async move {
                starts.lock().unwrap().push(r.text.clone());
                events("turn_start", &json!({}));
                tokio::select! {permit=permits.acquire_owned()=>{permit.unwrap().forget();events("turn_end",&json!({"usage":{"input":2,"output":1,"totalTokens":3}}));Ok(RunOutcome{output:format!("evidence: {}",r.text),usage:json!({"input":2,"output":1,"totalTokens":3}),..Default::default()})},_=signal.changed()=>Ok(RunOutcome{aborted:true,..Default::default()})}
            })
        }
        fn enqueue(
            &self,
            id: String,
            text: String,
            _: InputKind,
        ) -> BoxFuture<'static, Result<()>> {
            let inputs = self.inputs.clone();
            Box::pin(async move {
                inputs.lock().unwrap().push((id, text));
                Ok(())
            })
        }
        fn abort(&self, id: String) -> BoxFuture<'static, Result<()>> {
            let signals = self.signals.clone();
            Box::pin(async move {
                if let Some(tx) = signals.lock().unwrap().get(&id) {
                    let _ = tx.send(true);
                }
                Ok(())
            })
        }
        fn dispose(&self, _: String) -> BoxFuture<'static, Result<()>> {
            let disposed = self.disposed.clone();
            Box::pin(async move {
                disposed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }
    fn sandbox() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pisper-native-team-{}", crate::product::new_id()));
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
    async fn updated_queue_followup_and_same_name_retry_execute_current_graph_inputs() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start("parent", "verify updated work", &Value::Null, "team")
            .unwrap();
        let team = TeamService::new(
            dir.join("teams.json"),
            goals.clone(),
            Arc::new(|_, _| {}),
            true,
        )
        .unwrap();
        let agents = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals,
            Arc::new(|_, _| {}),
        )
        .unwrap();
        team.attach_agents(Arc::downgrade(&agents));
        agents.set_graph(team.clone());
        agents
            .spawn(
                "parent".into(),
                json!({"taskName":"build","message":"build"}),
            )
            .await
            .unwrap();
        let old = agents.spawn("parent".into(), json!({"taskName":"verify","message":"old input","files":["src/old"],"dependsOn":["build"]})).await.unwrap();
        until(|| fixture.starts.lock().unwrap().len() == 1).await;
        agents
            .update_task(
                "parent",
                "verify",
                &json!({"message":"current input","files":["src/current"]}),
            )
            .await
            .unwrap();
        assert_eq!(
            agents.find("parent", old["id"].as_str().unwrap()).unwrap()["status"],
            "interrupted"
        );
        let replacement = agents.find("parent", "verify").unwrap();
        assert_ne!(replacement["id"], old["id"]);
        assert_eq!(fixture.starts.lock().unwrap().len(), 1);
        fixture.permits.add_permits(1);
        until(|| fixture.starts.lock().unwrap().len() == 2).await;
        assert_eq!(fixture.starts.lock().unwrap()[1], "current input");
        assert_eq!(
            fixture.contexts.lock().unwrap()[1].owned_files,
            vec!["src/current"]
        );
        fixture.permits.add_permits(1);
        until(|| !agents.has_active("parent")).await;
        let context_count = fixture.contexts.lock().unwrap().len();
        agents
            .followup(
                "parent",
                replacement["id"].as_str().unwrap(),
                "verify followup",
            )
            .await
            .unwrap();
        until(|| fixture.starts.lock().unwrap().len() == 3).await;
        assert_eq!(fixture.starts.lock().unwrap()[2], "verify followup");
        assert_eq!(fixture.contexts.lock().unwrap().len(), context_count);
        fixture.permits.add_permits(1);
        until(|| !agents.has_active("parent")).await;
        assert_eq!(
            team.task("parent", "verify").unwrap()["status"],
            "completed"
        );
        let retry = agents
            .spawn(
                "parent".into(),
                json!({"taskName":"retry","message":"first retry"}),
            )
            .await
            .unwrap();
        until(|| fixture.starts.lock().unwrap().len() == 4).await;
        agents
            .interrupt(
                "parent",
                retry["id"].as_str().unwrap(),
                "explicit task stop",
            )
            .await
            .unwrap();
        let restarted = agents
            .spawn(
                "parent".into(),
                json!({"taskName":"retry","message":"retried current input"}),
            )
            .await
            .unwrap();
        assert_eq!(restarted["teamTaskId"], retry["teamTaskId"]);
        assert_ne!(restarted["id"], retry["id"]);
        until(|| fixture.starts.lock().unwrap().len() == 5).await;
        assert_eq!(fixture.starts.lock().unwrap()[4], "retried current input");
        fixture.permits.add_permits(1);
        until(|| !agents.has_active("parent")).await;
        team.check_complete("parent").unwrap();
        agents.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn dependency_graph_runs_actual_children_and_requires_evidence() {
        let dir = sandbox();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start("parent", "implement and verify", &Value::Null, "team")
            .unwrap();
        let team = TeamService::new(
            dir.join("teams.json"),
            goals.clone(),
            Arc::new(|_, _| {}),
            true,
        )
        .unwrap();
        let agents = AgentService::new(
            dir.join("agents.json"),
            fixture.clone(),
            goals.clone(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        team.attach_agents(Arc::downgrade(&agents));
        agents.set_graph(team.clone());
        let first=agents.spawn("parent".into(),json!({"taskName":"build","message":"first implementation","files":["src/shared"],"autoStart":true})).await.unwrap();
        assert!(agents
            .spawn(
                "parent".into(),
                json!({"taskName":"conflict","message":"overlap","files":["src/shared/file.rs"]})
            )
            .await
            .is_err());
        let second=agents.spawn("parent".into(),json!({"taskName":"verify","message":"dependent verification","files":["src/shared/file.rs"],"dependsOn":["build"]})).await.unwrap();
        until(|| fixture.starts.lock().unwrap().len() == 1).await;
        agents
            .send_from_agent(
                "parent",
                first["id"].as_str().unwrap(),
                second["canonicalName"].as_str().unwrap(),
                "queued dependency handoff",
            )
            .await
            .unwrap();
        assert_eq!(
            team.get("parent").unwrap()["communications"][0]["status"],
            "queued"
        );
        assert_eq!(
            agents
                .find("parent", second["id"].as_str().unwrap())
                .unwrap()["status"],
            "queued"
        );
        assert!(team.check_complete("parent").is_err());
        fixture.permits.add_permits(1);
        until(|| fixture.starts.lock().unwrap().len() == 2).await;
        assert!(fixture.inputs.lock().unwrap().iter().any(|(id, text)| {
            id == second["id"].as_str().unwrap() && text == "queued dependency handoff"
        }));
        assert_eq!(
            team.get("parent").unwrap()["communications"][0]["status"],
            "delivered"
        );
        assert_eq!(
            agents
                .find("parent", first["id"].as_str().unwrap())
                .unwrap()["status"],
            "completed"
        );
        fixture.permits.add_permits(1);
        until(|| !agents.has_active("parent")).await;
        assert_eq!(team.get("parent").unwrap()["completedTaskCount"], 2);
        team.check_complete("parent").unwrap();
        assert_eq!(goals.get("parent").unwrap().tokens_used, 6);
        team.mark_complete("parent").unwrap();
        assert_eq!(team.get("parent").unwrap()["status"], "complete");
        agents.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn restart_graph_pauses_without_execution_and_recovers_lease_on_explicit_resume() {
        let dir = sandbox();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start("parent", "synthetic restart", &Value::Null, "team")
            .unwrap();
        let path = dir.join("teams.json");
        let team =
            TeamService::new(path.clone(), goals.clone(), Arc::new(|_, _| {}), true).unwrap();
        let scope = SessionScope {
            session_id: "parent".into(),
            ..Default::default()
        };
        let task = team
            .register(
                &scope,
                &json!({"taskName":"one","message":"persisted actual task","files":["src/one"]}),
            )
            .unwrap();
        team.bind("parent", &task, &json!({"id":"old-agent"}))
            .unwrap();
        assert!(team.ready("parent", &task).unwrap());
        let restarted = TeamService::new(path, goals.clone(), Arc::new(|_, _| {}), true).unwrap();
        assert_eq!(restarted.get("parent").unwrap()["status"], "paused");
        assert_eq!(
            restarted.task("parent", &task).unwrap()["status"],
            "interrupted"
        );
        assert!(!restarted.ready("parent", &task).unwrap());
        restarted.ensure("parent").unwrap();
        assert_eq!(restarted.task("parent", &task).unwrap()["status"], "queued");
        assert_eq!(restarted.task("parent", &task).unwrap()["agentId"], "");
        restarted
            .bind("parent", &task, &json!({"id":"new-agent"}))
            .unwrap();
        assert!(restarted.ready("parent", &task).unwrap());
        restarted
            .update_agent(
                "parent",
                &json!({"id":"old-agent","status":"completed","output":"stale"}),
            )
            .unwrap();
        assert_eq!(
            restarted.task("parent", &task).unwrap()["status"],
            "starting"
        );
        restarted
            .update_agent(
                "parent",
                &json!({"id":"new-agent","status":"completed","output":""}),
            )
            .unwrap();
        assert!(restarted.check_complete("parent").is_err());
        restarted
            .resolve("parent", &task, "verified result", "completed")
            .unwrap();
        restarted.check_complete("parent").unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn graph_rejects_cycle_and_classifies_bounded_retry() {
        let dir = sandbox();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start("parent", "test graph", &Value::Null, "team")
            .unwrap();
        let team =
            TeamService::new(dir.join("teams.json"), goals, Arc::new(|_, _| {}), true).unwrap();
        let scope = SessionScope {
            session_id: "parent".into(),
            ..Default::default()
        };
        let first = team
            .register(&scope, &json!({"taskName":"first","message":"one"}))
            .unwrap();
        team.register(
            &scope,
            &json!({"taskName":"second","message":"two","dependsOn":["first"]}),
        )
        .unwrap();
        assert!(team
            .update_task("parent", &first, &json!({"dependsOn":["second"]}))
            .is_err());
        assert!(team
            .register(
                &scope,
                &json!({"taskName":"missing","message":"one","dependsOn":["nonexistent"]})
            )
            .is_err());
        let mut task = Task {
            status: "failed".into(),
            error_kind: error_kind("upstream stream_read_error", "failed"),
            attempts: 1,
            ..Default::default()
        };
        retry(&mut task, 1000);
        assert_eq!(timestamp(task.next_retry_at.as_ref().unwrap()), 31_000);
        task.attempts = 100;
        retry(&mut task, 1000);
        assert_eq!(timestamp(task.next_retry_at.as_ref().unwrap()), 601_000);
        task.error_kind = "authentication".into();
        retry(&mut task, 1000);
        assert!(task.next_retry_at.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
