//! 把受限 JS 的 agent() Promise 接到真实 Team 图及独立 Pi 子会话。
use super::{
    agents::{AgentService, TaskGraph, WorkflowRunner},
    team::TeamService,
    team_js, SessionExecutor,
};
use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
#[derive(Clone)]
pub(crate) struct TeamWorkflowService {
    team: Arc<TeamService>,
    agents: Weak<AgentService>,
    executor: Arc<dyn SessionExecutor>,
    active: Arc<Mutex<HashSet<String>>>,
}
struct Lease {
    active: Arc<Mutex<HashSet<String>>>,
    parent: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.active
            .lock()
            .expect("Team workflow runs")
            .remove(&self.parent);
    }
}
#[derive(Default)]
struct Requests {
    number: usize,
    aliases: BTreeMap<String, String>,
    tasks: HashSet<String>,
}
fn fingerprint(value: &Value) -> Result<String> {
    use sha2::Digest;
    Ok(sha2::Sha256::digest(serde_json::to_vec(value)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn structured(output: &str, schema: &Value) -> Value {
    if schema.is_null() {
        return json!(output);
    }
    let text = output.trim();
    let text = if text.starts_with("```") && text.ends_with("```") {
        text.trim_start_matches("```")
            .strip_prefix("json")
            .unwrap_or(text.trim_start_matches("```"))
            .trim_end_matches("```")
            .trim()
    } else {
        text
    };
    serde_json::from_str(text).unwrap_or(Value::Null)
}
impl TeamWorkflowService {
    pub(crate) fn new(
        team: Arc<TeamService>,
        agents: Weak<AgentService>,
        executor: Arc<dyn SessionExecutor>,
    ) -> Arc<Self> {
        Arc::new(Self {
            team,
            agents,
            executor,
            active: Arc::new(Mutex::new(HashSet::new())),
        })
    }
    async fn run_script(&self, parent: String, path: String, args: Value) -> Result<Value> {
        let scope = self.executor.scope(parent.clone()).await?;
        if scope.parent_session_id.is_some() {
            bail!("Subagents cannot run Team workflows.")
        }
        if path.trim().is_empty() || path.encode_utf16().count() > 240 {
            bail!("Workflow path must contain 1–240 characters.")
        }
        if !args.is_null() && !args.is_object() {
            bail!("Workflow args must be an object.")
        }
        self.team.ensure(&parent)?;
        {
            let mut active = self.active.lock().expect("Team workflow runs");
            if !active.insert(parent.clone()) {
                bail!("A Team workflow is already running for this session.")
            }
        }
        let _lease = Lease {
            active: self.active.clone(),
            parent: parent.clone(),
        };
        let script = team_js::read_script(Path::new(&scope.cwd), &path)?;
        self.team.set_script_path(&parent, &script.path)?;
        let agents = self
            .agents
            .upgrade()
            .ok_or_else(|| anyhow!("Team Agent executor unavailable."))?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let own_requests = requests.clone();
        let team = self.team.clone();
        let owner = parent.clone();
        let script_hash = script.fingerprint.clone();
        let args_json = serde_json::to_string(&args)?;
        let handler = Arc::new(move |input: Value| -> BoxFuture<'static, Result<Value>> {
            let agents = agents.clone();
            let team = team.clone();
            let requests = own_requests.clone();
            let owner = owner.clone();
            let script_hash = script_hash.clone();
            let args_json = args_json.clone();
            Box::pin(async move {
                if team.get(&owner).is_none_or(|t| t["status"] != "active") {
                    bail!("Team workflow is no longer active.")
                }
                let options = &input["options"];
                let schema = options["schema"].clone();
                let (name, dependencies) = {
                    let mut requests = requests.lock().expect("workflow request order");
                    requests.number += 1;
                    if requests.number > 64 {
                        bail!("A Team workflow cannot start more than 64 agents.")
                    }
                    let label = options["label"]
                        .as_str()
                        .or_else(|| options["taskName"].as_str())
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("agent_{}", requests.number));
                    let phase = options["__pisperPhase"].as_str().unwrap_or_default();
                    let name = super::agents::task_name(&format!(
                        "{}{}_{}",
                        if phase.is_empty() {
                            String::new()
                        } else {
                            format!("{phase}_")
                        },
                        label,
                        requests.number
                    ));
                    if name.is_empty() {
                        bail!("Workflow Agent label requires usable characters.")
                    }
                    let dependencies = options["dependsOn"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|dependency| {
                            let key = super::agents::task_name(dependency);
                            requests.aliases.get(&key).cloned().unwrap_or(key)
                        })
                        .collect::<Vec<_>>();
                    requests
                        .aliases
                        .insert(super::agents::task_name(&label), name.clone());
                    (name, dependencies)
                };
                let prompt = input["prompt"].as_str().unwrap_or_default().trim();
                let message = if schema.is_null() {
                    prompt.into()
                } else {
                    format!(
                        "{prompt}\n\nReturn only a JSON value matching this schema:\n{}",
                        serde_json::to_string(&schema)?
                    )
                };
                let hash = fingerprint(
                    &json!({"scriptFingerprint":script_hash,"args":args_json,"taskName":name,"message":message,"role":options.get("role").unwrap_or(&json!("")),"files":options.get("files").unwrap_or(&json!([])),"dependsOn":dependencies,"schema":if schema.is_null(){String::new()}else{serde_json::to_string(&schema)?}}),
                )?;
                let previous = team.task(&owner, &name);
                if let Some(task) = &previous {
                    requests
                        .lock()
                        .expect("workflow task ids")
                        .tasks
                        .insert(task["id"].as_str().unwrap_or_default().into());
                    if task["status"] == "completed" && task["workflowFingerprint"] == hash {
                        return Ok(structured(
                            task["output"].as_str().unwrap_or_default(),
                            &schema,
                        ));
                    }
                    if matches!(task["status"].as_str(), Some("starting" | "running"))
                        && task["workflowFingerprint"] != hash
                    {
                        bail!("Workflow task {name} is already running with different inputs.")
                    }
                }
                let mut params = json!({"taskName":name,"message":message,"role":options["role"],"files":options["files"],"dependsOn":dependencies,"autoStart":true,"workflowFingerprint":hash});
                let waiting = previous.as_ref().is_some_and(|t| {
                    t["workflowFingerprint"] == hash
                        && !t["agentId"].as_str().unwrap_or_default().is_empty()
                        && matches!(
                            t["status"].as_str(),
                            Some("queued" | "blocked" | "starting" | "running")
                        )
                });
                let task_id = if waiting {
                    previous.as_ref().expect("existing workflow task")["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                } else {
                    if let Some(previous) = previous {
                        if let Some(agent) = previous["agentId"].as_str().filter(|s| !s.is_empty())
                        {
                            if agents.find(&owner, agent).is_some_and(|a| {
                                matches!(
                                    a["status"].as_str(),
                                    Some("queued" | "starting" | "running")
                                )
                            }) {
                                agents
                                    .interrupt(&owner, agent, "Workflow task inputs changed.")
                                    .await?;
                            }
                        }
                        let id = previous["id"]
                            .as_str()
                            .ok_or_else(|| anyhow!("Workflow task has no id."))?;
                        team.update_task(&owner, id, &params)?;
                        params["teamTaskId"] = json!(id);
                    }
                    let spawned = agents.spawn(owner.clone(), params).await?;
                    spawned["teamTaskId"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow!("Spawned workflow Agent has no persisted Team task.")
                        })?
                        .to_string()
                };
                requests
                    .lock()
                    .expect("workflow task ids")
                    .tasks
                    .insert(task_id.clone());
                loop {
                    if team.get(&owner).is_none_or(|t| t["status"] != "active") {
                        bail!("Team workflow is no longer active.")
                    }
                    let task = team
                        .task(&owner, &task_id)
                        .ok_or_else(|| anyhow!("Workflow task disappeared while waiting."))?;
                    match task["status"].as_str() {
                        Some("completed") => {
                            return Ok(structured(
                                task["output"].as_str().unwrap_or_default(),
                                &schema,
                            ))
                        }
                        Some("failed" | "interrupted") => bail!(
                            "{}",
                            task["error"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("Workflow Agent did not complete.")
                        ),
                        _ => {}
                    }
                    team.schedule(&owner).await?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        });
        let result = team_js::execute(&script, args, handler).await?;
        let team = self
            .team
            .get(&parent)
            .ok_or_else(|| anyhow!("Workflow Team disappeared."))?;
        let task_count = requests.lock().expect("workflow task ids").tasks.len();
        Ok(
            json!({"scriptPath":script.path,"meta":script.meta,"logs":result["logs"],"result":result["result"],"taskCount":task_count,"tasks":team["tasks"],"team":team}),
        )
    }
}
impl WorkflowRunner for TeamWorkflowService {
    fn run(&self, parent: String, path: String, args: Value) -> BoxFuture<'static, Result<Value>> {
        let service = self.clone();
        Box::pin(async move { service.run_script(parent, path, args).await })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{goal_api::GoalService, session_workers::team::tests::Fixture};
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
    async fn workspace_script_runs_real_dependency_children_and_reuses_verified_results() {
        let dir =
            std::env::temp_dir().join(format!("pisper-team-workflow-{}", crate::product::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("workflow.js");
        std::fs::write(&file,"export const meta={name:'fixture',description:'real delegated dependency'}; phase('verify'); log('start'); return await parallel([()=>agent('first evidence',{label:'first',files:['src/proof']}),()=>agent('dependent evidence',{label:'second',files:['src/proof'],dependsOn:['first']})]);").unwrap();
        let fixture = Fixture::new();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start(
                "parent",
                "implement and verify dependencies",
                &Value::Null,
                "team",
            )
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
        let workflow =
            TeamWorkflowService::new(team.clone(), Arc::downgrade(&agents), fixture.clone());
        agents.set_workflow(workflow.clone());
        let run = tokio::spawn(workflow.run(
            "parent".into(),
            file.to_string_lossy().into(),
            json!({"fixture":true}),
        ));
        until(|| {
            fixture.starts.lock().unwrap().len() == 1
                && team.get("parent").is_some_and(|value| {
                    value["tasks"]
                        .as_array()
                        .is_some_and(|tasks| tasks.len() == 2)
                })
        })
        .await;
        assert_eq!(
            team.get("parent").unwrap()["tasks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(fixture.starts.lock().unwrap()[0], "first evidence");
        fixture.permits.add_permits(1);
        until(|| fixture.starts.lock().unwrap().len() == 2).await;
        fixture.permits.add_permits(1);
        let result = run.await.unwrap().unwrap();
        assert_eq!(result["taskCount"], 2);
        assert_eq!(
            result["result"],
            json!(["evidence: first evidence", "evidence: dependent evidence"])
        );
        assert_eq!(result["logs"], json!(["start"]));
        team.check_complete("parent").unwrap();
        let repeated = workflow
            .run(
                "parent".into(),
                file.to_string_lossy().into(),
                json!({"fixture":true}),
            )
            .await
            .unwrap();
        assert_eq!(repeated["result"], result["result"]);
        assert_eq!(fixture.starts.lock().unwrap().len(),2,"Identical completed task fingerprint must reuse verified output without another model call");
        agents.shutdown().await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
