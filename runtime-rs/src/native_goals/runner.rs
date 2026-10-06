//! 连续轮执行器：每轮调用真实 SessionExecutor，不以 goal 元数据充当执行成功。
use super::{budget_prompt, continuation, Goal, GoalService};
use crate::session_workers::{EventSink, PromptRequest, RunOutcome, SessionExecutor};
use anyhow::{anyhow, bail, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub(crate) trait TeamCoordinator: Send + Sync {
    fn continuation(&self, session: String) -> BoxFuture<'static, Result<String>>;
    fn check_complete(&self, session: String) -> BoxFuture<'static, Result<()>>;
    fn mark_complete(&self, session: String) -> BoxFuture<'static, Result<()>>;
    fn stop(&self, session: String, reason: String) -> BoxFuture<'static, Result<()>>;
}
struct Active {
    generation: String,
    cancel: watch::Sender<bool>,
}
pub(crate) struct GoalRunner {
    pub(crate) goals: Arc<GoalService>,
    executor: Arc<dyn SessionExecutor>,
    active: Mutex<HashMap<String, Active>>,
    team: Mutex<Option<Arc<dyn TeamCoordinator>>>,
}
struct Lease {
    runner: Weak<GoalRunner>,
    session: String,
    generation: String,
    finished: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let Some(runner) = self.runner.upgrade() else {
            return;
        };
        let mut active = runner.active.lock().expect("goal runners");
        if !active
            .get(&self.session)
            .is_some_and(|a| a.generation == self.generation)
        {
            return;
        }
        active.remove(&self.session);
        drop(active);
        if !self.finished {
            let _ = runner.goals.pause(&self.session);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let session = self.session.clone();
                handle.spawn(async move {
                    let _ = runner.executor.abort(session.clone()).await;
                    if let Some(team) = runner.team() {
                        let _ = team
                            .stop(session, "Goal execution was cancelled.".into())
                            .await;
                    }
                });
            }
        }
    }
}
impl GoalRunner {
    pub(crate) fn new(goals: Arc<GoalService>, executor: Arc<dyn SessionExecutor>) -> Arc<Self> {
        Arc::new(Self {
            goals,
            executor,
            active: Mutex::new(HashMap::new()),
            team: Mutex::new(None),
        })
    }
    pub(crate) fn set_team(&self, team: Arc<dyn TeamCoordinator>) {
        *self.team.lock().expect("goal team coordinator") = Some(team);
    }
    fn team(&self) -> Option<Arc<dyn TeamCoordinator>> {
        self.team.lock().expect("goal team coordinator").clone()
    }
    pub(crate) fn busy(&self, session: &str) -> bool {
        self.active
            .lock()
            .expect("goal runners")
            .contains_key(session)
    }
    pub(crate) async fn complete(&self, session: &str) -> Result<Goal> {
        let current = self
            .goals
            .get(session)
            .ok_or_else(|| anyhow!("No active goal is available to complete."))?;
        let team = if current.mode == "team" {
            Some(
                self.team()
                    .ok_or_else(|| anyhow!("Team execution coordinator has not been installed."))?,
            )
        } else {
            None
        };
        if let Some(team) = &team {
            team.check_complete(session.into()).await?;
        }
        let goal = self.goals.complete(session)?;
        if let Some(team) = team {
            if let Err(error) = team.mark_complete(session.into()).await {
                self.goals.reopen(session, &current.id)?;
                return Err(error);
            }
        }
        Ok(goal)
    }
    pub(crate) async fn pause(&self, session: &str) -> Result<Option<Goal>> {
        let goal = self.goals.pause(session)?;
        if goal.as_ref().is_some_and(|g| g.mode == "team") {
            self.team()
                .ok_or_else(|| anyhow!("Team execution coordinator has not been installed."))?
                .stop(
                    session.into(),
                    "Team goal was paused; active members were stopped.".into(),
                )
                .await?;
        }
        Ok(goal)
    }
    pub(crate) async fn set_budget(&self, session: &str, budget: &Value) -> Result<Goal> {
        let goal = self.goals.set_budget(session, budget)?;
        if goal.mode == "team" && goal.status == "budget_limited" {
            self.team()
                .ok_or_else(|| anyhow!("Team execution coordinator has not been installed."))?
                .stop(
                    session.into(),
                    "Team token budget was reached; remaining members were stopped.".into(),
                )
                .await?;
        }
        Ok(goal)
    }
    pub(crate) async fn cancel(&self, session: &str) -> Result<()> {
        if let Some(active) = self.active.lock().expect("goal runners").get(session) {
            let _ = active.cancel.send(true);
        }
        self.goals.pause(session)?;
        self.executor.abort(session.into()).await?;
        if let Some(team) = self.team() {
            team.stop(session.into(), "Parent session stopped.".into())
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        let ids = self
            .active
            .lock()
            .expect("goal runners")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel(&id).await?;
        }
        self.goals.pause_all()
    }
    /// 初始 prompt 也沿用这一执行链，root 在流式运行中负责消息排队和 SSE record。
    pub(crate) async fn run(
        self: &Arc<Self>,
        session: String,
        initial: String,
        events: EventSink,
    ) -> Result<RunOutcome> {
        self.run_prepared(
            session.clone(),
            PromptRequest {
                session_id: session,
                text: initial,
                ..Default::default()
            },
            events,
        )
        .await
    }
    pub(crate) async fn run_with_images(
        self: &Arc<Self>,
        session: String,
        initial: String,
        images: Vec<pi_rust::ai::types::ImageContent>,
        events: EventSink,
    ) -> Result<RunOutcome> {
        self.run_prepared(
            session.clone(),
            PromptRequest {
                session_id: session,
                text: initial,
                images,
                ..Default::default()
            },
            events,
        )
        .await
    }
    pub(crate) async fn run_prepared(
        self: &Arc<Self>,
        session: String,
        initial: PromptRequest,
        events: EventSink,
    ) -> Result<RunOutcome> {
        if initial.session_id != session {
            bail!("Initial Goal prompt belongs to a different session.")
        }
        let (cancel, mut cancellation) = watch::channel(false);
        let generation = crate::product::new_id();
        {
            let mut active = self.active.lock().expect("goal runners");
            if active.contains_key(&session) {
                bail!("Session already has an active goal runner.")
            }
            active.insert(
                session.clone(),
                Active {
                    generation: generation.clone(),
                    cancel,
                },
            );
        }
        let mut lease = Lease {
            runner: Arc::downgrade(self),
            session: session.clone(),
            generation,
            finished: false,
        };
        let result = self
            .run_loop(&session, initial, events, &mut cancellation)
            .await;
        if result.is_err() {
            self.goals.pause(&session)?;
            if let Some(team) = self.team() {
                team.stop(session.clone(), "Goal execution failed.".into())
                    .await?;
            }
        }
        lease.finished = true;
        result
    }
    async fn run_loop(
        &self,
        session: &str,
        mut initial: PromptRequest,
        events: EventSink,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<RunOutcome> {
        let mut text = initial.text.clone();
        if let Some(goal) = self
            .goals
            .get(session)
            .filter(|goal| goal.status == "active")
        {
            let mut context = continuation(&goal);
            if goal.mode == "team" {
                // Release schedules ready members before the lead's first
                // model request, including when a paused Team is resumed.
                let team = self
                    .team()
                    .ok_or_else(|| anyhow!("Team execution coordinator has not been installed."))?;
                context.push_str("\n\n");
                context.push_str(&team.continuation(session.into()).await?);
            }
            // Keep the public initial message and its existing attachment /
            // memory context intact. Release hides the active objective and
            // Team graph behind the same attachment-context boundary.
            if !initial.isolated {
                text.push_str(crate::session_api::ATTACHMENT_MARKER);
                text.push_str(&context);
            }
        }
        let mut internal = initial.internal;
        let mut first_input = true;
        let mut summary = false;
        loop {
            if *cancellation.borrow() {
                return Ok(RunOutcome {
                    aborted: true,
                    ..Default::default()
                });
            }
            let turn = Arc::new(Mutex::new(None::<(String, Instant)>));
            let error = Arc::new(Mutex::new(None::<String>));
            let own_turn = turn.clone();
            let own_error = error.clone();
            let goals = self.goals.clone();
            let id = session.to_string();
            let forward = events.clone();
            let executor = self.executor.clone();
            let sink: EventSink = Arc::new(move |event, data| {
                if event == "turn_start" {
                    *own_turn.lock().expect("goal turn") = goals
                        .get(&id)
                        .filter(|g| g.status == "active")
                        .map(|g| (g.id, Instant::now()));
                }
                if event == "turn_end" {
                    if let Some((goal_id, started)) = own_turn.lock().expect("goal turn").take() {
                        let usage = data.get("usage").unwrap_or(&data["message"]["usage"]);
                        let elapsed = data["elapsedSeconds"]
                            .as_f64()
                            .unwrap_or_else(|| started.elapsed().as_secs_f64());
                        if let Err(error) = goals.account(&id, &goal_id, usage, elapsed) {
                            *own_error.lock().expect("goal accounting error") =
                                Some(error.to_string());
                            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                                let executor = executor.clone();
                                let id = id.clone();
                                handle.spawn(async move {
                                    let _ = executor.abort(id).await;
                                });
                            }
                        }
                    }
                }
                forward(event, data);
            });
            let prompt = self.executor.prompt(
                PromptRequest {
                    session_id: session.into(),
                    text,
                    internal,
                    images: std::mem::take(&mut initial.images),
                    context_prepared: first_input && initial.context_prepared,
                    isolated: initial.isolated,
                },
                sink,
            );
            first_input = false;
            tokio::pin!(prompt);
            let outcome = tokio::select! {biased;
                changed=cancellation.changed()=>{
                    if changed.is_ok()&&*cancellation.borrow(){
                        let (aborted, _) = tokio::time::timeout(Duration::from_secs(10), async {
                            tokio::join!(self.executor.abort(session.into()), &mut prompt)
                        }).await.map_err(|_|anyhow!("Goal cancellation did not settle within 10 seconds."))?;
                        aborted?;return Ok(RunOutcome{aborted:true,..Default::default()})
                    }
                    prompt.await?
                },
                outcome=&mut prompt=>outcome?,
            };
            if let Some(error) = error.lock().expect("goal accounting error").take() {
                bail!("Goal usage persistence failed: {error}")
            }
            if outcome.aborted {
                self.goals.pause(session)?;
                return Ok(outcome);
            }
            if let Some(error) = &outcome.error {
                bail!("{error}")
            }
            let Some(goal) = self.goals.get(session) else {
                return Ok(outcome);
            };
            if summary || !matches!(goal.status.as_str(), "active" | "budget_limited") {
                return Ok(outcome);
            }
            if goal.status == "budget_limited" {
                text = budget_prompt(&goal);
                summary = true;
                if goal.mode == "team" {
                    self.team()
                        .ok_or_else(|| {
                            anyhow!("Team execution coordinator has not been installed.")
                        })?
                        .stop(
                            session.into(),
                            "Team token budget was reached; remaining members were stopped.".into(),
                        )
                        .await?;
                }
            } else {
                text = continuation(&goal);
                if goal.mode == "team" {
                    let team = self.team().ok_or_else(|| {
                        anyhow!("Team execution coordinator has not been installed.")
                    })?;
                    text.push_str("\n\n");
                    text.push_str(&team.continuation(session.into()).await?);
                }
            }
            internal = true;
            events(
                "goal_continuation",
                &json!({"sessionId":session,"goalId":goal.id,"budgetSummary":summary}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_workers::{ChildRequest, InputKind, SessionScope};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fixture {
        goals: Arc<GoalService>,
        runs: Arc<AtomicUsize>,
        texts: Arc<Mutex<Vec<String>>>,
        requests: Arc<Mutex<Vec<PromptRequest>>>,
        finish: usize,
    }
    impl SessionExecutor for Fixture {
        fn scope(&self, id: String) -> BoxFuture<'static, Result<SessionScope>> {
            Box::pin(async move {
                Ok(SessionScope {
                    session_id: id,
                    ..Default::default()
                })
            })
        }
        fn create_child(&self, _: ChildRequest) -> BoxFuture<'static, Result<SessionScope>> {
            Box::pin(async { bail!("not used") })
        }
        fn enqueue(&self, _: String, _: String, _: InputKind) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn abort(&self, _: String) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn dispose(&self, _: String) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn prompt(
            &self,
            request: PromptRequest,
            events: EventSink,
        ) -> BoxFuture<'static, Result<RunOutcome>> {
            let goals = self.goals.clone();
            let runs = self.runs.clone();
            let texts = self.texts.clone();
            let requests = self.requests.clone();
            let finish = self.finish;
            Box::pin(async move {
                let run = runs.fetch_add(1, Ordering::SeqCst) + 1;
                texts.lock().unwrap().push(request.text.clone());
                requests.lock().unwrap().push(request.clone());
                events("turn_start", &json!({}));
                if run == finish {
                    goals.complete(&request.session_id)?;
                }
                events(
                    "turn_end",
                    &json!({"usage":{"totalTokens":10},"elapsedSeconds":1}),
                );
                Ok(RunOutcome {
                    output: format!("verified turn {run}"),
                    usage: json!({"totalTokens":10}),
                    ..Default::default()
                })
            })
        }
    }
    #[tokio::test]
    async fn actual_executor_is_called_until_completion_and_budget_queues_one_summary() {
        for budget in [Value::Null, json!(10)] {
            let dir = super::super::tests::sandbox();
            let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
            goals
                .start("s", "verify all requirements", &budget, "goal")
                .unwrap();
            let runs = Arc::new(AtomicUsize::new(0));
            let texts = Arc::new(Mutex::new(vec![]));
            let executor = Arc::new(Fixture {
                goals: goals.clone(),
                runs: runs.clone(),
                texts: texts.clone(),
                requests: Arc::new(Mutex::new(vec![])),
                finish: if budget.is_null() { 3 } else { 99 },
            });
            let runner = GoalRunner::new(goals.clone(), executor);
            let result = runner
                .run("s".into(), "initial user task".into(), Arc::new(|_, _| {}))
                .await
                .unwrap();
            assert!(!result.aborted);
            assert_eq!(
                runs.load(Ordering::SeqCst),
                if budget.is_null() { 3 } else { 2 }
            );
            assert_eq!(
                goals.get("s").unwrap().status,
                if budget.is_null() {
                    "complete"
                } else {
                    "budget_limited"
                }
            );
            assert!(texts.lock().unwrap()[1].starts_with(super::super::CONTINUATION_MARKER));
            assert!(!runner.busy("s"));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn prepared_images_and_context_are_consumed_only_on_initial_turn() {
        let dir = super::super::tests::sandbox();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        goals
            .start("s", "verify the attached image", &Value::Null, "goal")
            .unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let executor = Arc::new(Fixture {
            goals: goals.clone(),
            runs: Arc::new(AtomicUsize::new(0)),
            texts: Arc::new(Mutex::new(vec![])),
            requests: requests.clone(),
            finish: 2,
        });
        let runner = GoalRunner::new(goals, executor);
        runner
            .run_prepared(
                "s".into(),
                PromptRequest {
                    session_id: "s".into(),
                    text: "prepared attachment and memory context".into(),
                    images: vec![pi_rust::ai::types::ImageContent {
                        data: "c3ludGhldGljLWZpeHR1cmU=".into(),
                        mime_type: "image/png".into(),
                    }],
                    context_prepared: true,
                    isolated: true,
                    ..Default::default()
                },
                Arc::new(|_, _| {}),
            )
            .await
            .unwrap();
        let observed = requests.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].images.len(), 1);
        assert_eq!(observed[0].images[0].data, "c3ludGhldGljLWZpeHR1cmU=");
        assert!(observed[0].context_prepared && observed[0].isolated);
        assert!(!observed[0].internal);
        assert!(observed[1].images.is_empty());
        assert!(!observed[1].context_prepared);
        assert!(observed[1].isolated && observed[1].internal);
        drop(observed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn paused_goal_resume_injects_original_objective_into_first_real_request() {
        let dir = super::super::tests::sandbox();
        let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
        let original = goals
            .start("s", "实施并验证原始需求", &json!(100), "goal")
            .unwrap();
        goals.pause("s").unwrap();
        let resumed = goals.resume("s", &json!({"mode":"goal"})).unwrap();
        assert_eq!(resumed.id, original.id);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let runner = GoalRunner::new(
            goals.clone(),
            Arc::new(Fixture {
                goals: goals.clone(),
                runs: Arc::new(AtomicUsize::new(0)),
                texts: Arc::new(Mutex::new(Vec::new())),
                requests: requests.clone(),
                finish: 1,
            }),
        );
        let prepared = format!(
            "继续{}existing attachment and memory evidence",
            crate::session_api::ATTACHMENT_MARKER
        );
        runner
            .run_prepared(
                "s".into(),
                PromptRequest {
                    session_id: "s".into(),
                    text: prepared.clone(),
                    context_prepared: true,
                    images: vec![pi_rust::ai::types::ImageContent {
                        data: "c3ludGhldGlj".into(),
                        mime_type: "image/png".into(),
                    }],
                    ..Default::default()
                },
                Arc::new(|_, _| {}),
            )
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].text,
            format!(
                "{prepared}{}{}",
                crate::session_api::ATTACHMENT_MARKER,
                continuation(&resumed)
            )
        );
        assert_eq!(
            requests[0]
                .text
                .split(crate::session_api::ATTACHMENT_MARKER)
                .next(),
            Some("继续")
        );
        assert_eq!(requests[0].images[0].data, "c3ludGhldGlj");
        assert!(requests[0].context_prepared);
        assert!(!requests[0].internal);
        drop(requests);
        assert_eq!(goals.get("s").unwrap().id, original.id);
        assert!(!runner.busy("s"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct TracedExecutor {
        inner: Arc<Fixture>,
        order: Arc<Mutex<Vec<String>>>,
    }
    impl SessionExecutor for TracedExecutor {
        fn scope(&self, id: String) -> BoxFuture<'static, Result<SessionScope>> {
            self.inner.scope(id)
        }
        fn create_child(&self, request: ChildRequest) -> BoxFuture<'static, Result<SessionScope>> {
            self.inner.create_child(request)
        }
        fn enqueue(
            &self,
            id: String,
            text: String,
            kind: InputKind,
        ) -> BoxFuture<'static, Result<()>> {
            self.inner.enqueue(id, text, kind)
        }
        fn abort(&self, id: String) -> BoxFuture<'static, Result<()>> {
            self.inner.abort(id)
        }
        fn dispose(&self, id: String) -> BoxFuture<'static, Result<()>> {
            self.inner.dispose(id)
        }
        fn prompt(
            &self,
            request: PromptRequest,
            events: EventSink,
        ) -> BoxFuture<'static, Result<RunOutcome>> {
            let order = self.order.clone();
            let prompt = self.inner.prompt(request, events);
            Box::pin(async move {
                order.lock().unwrap().push("lead-model-request".into());
                prompt.await
            })
        }
    }
    struct PreparingTeam {
        order: Arc<Mutex<Vec<String>>>,
    }
    impl TeamCoordinator for PreparingTeam {
        fn continuation(&self, session: String) -> BoxFuture<'static, Result<String>> {
            let order = self.order.clone();
            Box::pin(async move {
                assert_eq!(session, "s");
                order.lock().unwrap().push("schedule-ready-members".into());
                Ok("[Pisper internal team execution]\nCurrent task graph: [{\"taskName\":\"ready\"}]".into())
            })
        }
        fn check_complete(&self, _: String) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn mark_complete(&self, _: String) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn stop(&self, _: String, _: String) -> BoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }
    #[tokio::test]
    async fn team_first_lead_request_follows_ready_scheduling_and_preserves_isolated_boundary() {
        for isolated in [false, true] {
            let dir = super::super::tests::sandbox();
            let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
            let original = goals
                .start("s", "original paused Team mission", &Value::Null, "team")
                .unwrap();
            goals.pause("s").unwrap();
            goals.resume("s", &json!({"mode":"team"})).unwrap();
            let order = Arc::new(Mutex::new(Vec::new()));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let runner = GoalRunner::new(
                goals.clone(),
                Arc::new(TracedExecutor {
                    inner: Arc::new(Fixture {
                        goals: goals.clone(),
                        runs: Arc::new(AtomicUsize::new(0)),
                        texts: Arc::new(Mutex::new(Vec::new())),
                        requests: requests.clone(),
                        finish: 1,
                    }),
                    order: order.clone(),
                }),
            );
            runner.set_team(Arc::new(PreparingTeam {
                order: order.clone(),
            }));
            runner
                .run_prepared(
                    "s".into(),
                    PromptRequest {
                        session_id: "s".into(),
                        text: "继续".into(),
                        isolated,
                        ..Default::default()
                    },
                    Arc::new(|_, _| {}),
                )
                .await
                .unwrap();
            assert_eq!(
                *order.lock().unwrap(),
                ["schedule-ready-members", "lead-model-request"]
            );
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            if isolated {
                assert_eq!(requests[0].text, "继续");
            } else {
                assert_eq!(
                    requests[0]
                        .text
                        .split(crate::session_api::ATTACHMENT_MARKER)
                        .next(),
                    Some("继续")
                );
                assert!(requests[0].text.contains(&original.objective));
                assert!(requests[0]
                    .text
                    .contains("[Pisper internal team execution]"));
                assert!(requests[0].text.contains("\"taskName\":\"ready\""));
            }
            drop(requests);
            assert!(!runner.busy("s"));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn paused_or_absent_goal_does_not_inject_first_turn_or_prepare_team() {
        for paused in [false, true] {
            let dir = super::super::tests::sandbox();
            let goals = GoalService::new(dir.join("goals.json"), true).unwrap();
            if paused {
                goals
                    .start("s", "inactive objective", &Value::Null, "team")
                    .unwrap();
                goals.pause("s").unwrap();
            }
            let order = Arc::new(Mutex::new(Vec::new()));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let runner = GoalRunner::new(
                goals.clone(),
                Arc::new(TracedExecutor {
                    inner: Arc::new(Fixture {
                        goals,
                        runs: Arc::new(AtomicUsize::new(0)),
                        texts: Arc::new(Mutex::new(Vec::new())),
                        requests: requests.clone(),
                        finish: usize::MAX,
                    }),
                    order: order.clone(),
                }),
            );
            runner.set_team(Arc::new(PreparingTeam {
                order: order.clone(),
            }));
            runner
                .run(
                    "s".into(),
                    "ordinary user message".into(),
                    Arc::new(|_, _| {}),
                )
                .await
                .unwrap();
            assert_eq!(*order.lock().unwrap(), ["lead-model-request"]);
            assert_eq!(requests.lock().unwrap()[0].text, "ordinary user message");
            assert!(!runner.busy("s"));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
