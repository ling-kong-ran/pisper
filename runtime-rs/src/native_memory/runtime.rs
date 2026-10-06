//! 记忆后台任务持有独立模型请求；生命周期与前台 Agent 的 prompt 相互独立。
use super::MemoryStore;
use anyhow::{Context as _, Result};
use chrono::{Local, TimeZone};
use futures::future::BoxFuture;
use pi_rust::{
    ai::{
        models::ModelsSimpleStreamOptions,
        types::{
            AssistantBlock, Message, Model, SimpleStreamOptions, StopReason, StreamOptions,
            StringOrBlocks, ThinkingLevel, UserMessage,
        },
        Context,
    },
    coding_agent::{
        core::{model_runtime::ModelRuntime, resource_loader::InlineExtension},
        session_manager::SessionManager,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{oneshot, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const SUMMARY_SYSTEM: &str = "You expand a Pisper memory entry into semantic retrieval keywords so a later full-text search can find it even when the query uses synonyms, related concepts, or different wording.\nFor each memory, output one line of comma-separated keywords, aliases, related technologies, and a short paraphrase.\nKeep it dense and lowercase where natural. Do not invent facts that are not implied by the memory.\nRedact any secrets. Output exactly one line per memory, in the memory language. No numbering, no explanations, no JSON.";
const EXTRACT_SYSTEM: &str = "You propose memory candidates for Pisper. Candidates are reviewed by the user before becoming trusted memory.\nKeep only reusable and relatively stable information explicitly supported by the source conversation.\nAllowed: lasting user preferences, explicit constraints, confirmed project architecture or technical decisions, and recurring risks.\nDo not treat the assistant claiming that work is complete, tests passed, or a fact is true as verified evidence.\nNever store temporary questions, small talk, speculation, plans in progress, API keys, passwords, tokens, private keys, or other secrets.\nOutput only a JSON array with at most 3 items. Output [] when there is nothing worth proposing.\nEvery item must include an exact short evidence quote copied verbatim from either the user or assistant message.\nUse a narrow stable topic key. A topic groups the same individual fact; do not use broad topics such as project.architecture.\nUse the source conversation language for title and content.\nItem format: {\"title\":\"short title\",\"content\":\"self-contained candidate fact\",\"topic\":\"narrow stable topic key\",\"type\":\"preference|decision|fact|risk|task\",\"scope\":\"global|project\",\"importance\":0.1 to 1,\"confidence\":0 to 1,\"evidence\":\"exact source quote\"}";

#[derive(Clone)]
pub struct CompletionRequest {
    pub system: String,
    pub user: String,
    pub max_tokens: u64,
}
#[derive(Clone)]
pub struct Completion {
    pub text: String,
    pub usage: Value,
    pub timestamp: i64,
    pub error_code: Option<&'static str>,
}
pub trait MemoryModel: Send + Sync {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Completion>;
}
#[derive(Clone)]
pub struct PiMemoryModel {
    runtime: ModelRuntime,
    model: Model,
}
impl PiMemoryModel {
    pub fn new(runtime: ModelRuntime, model: Model) -> Self {
        Self { runtime, model }
    }
}
impl MemoryModel for PiMemoryModel {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Completion> {
        let runtime = self.runtime.clone();
        let model = self.model.clone();
        Box::pin(async move {
            let context = Context {
                system_prompt: Some(request.system),
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text(request.user),
                    timestamp: pi_rust::ai::now_ms(),
                })],
                tools: None,
            };
            let options = ModelsSimpleStreamOptions {
                simple: SimpleStreamOptions {
                    stream: StreamOptions {
                        signal: Some(cancel.clone()),
                        temperature: if model.reasoning { None } else { Some(0.1) },
                        max_tokens: Some(request.max_tokens.min(if model.max_tokens > 0 {
                            model.max_tokens
                        } else {
                            u64::MAX
                        })),
                        ..Default::default()
                    },
                    reasoning: if model.reasoning {
                        Some(ThinkingLevel::Low)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            let response = runtime
                .complete_simple(&model, &context, Some(options))
                .await;
            let error_code = if cancel.is_cancelled() || response.stop_reason == StopReason::Aborted
            {
                Some("aborted")
            } else if response.error_message.is_some() || response.stop_reason == StopReason::Error
            {
                Some("model_error")
            } else if response.stop_reason == StopReason::Length {
                Some("token_limit")
            } else {
                None
            };
            let text = response
                .content
                .iter()
                .filter_map(|part| match part {
                    AssistantBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            Completion {
                text,
                usage: serde_json::to_value(response.usage).unwrap_or(Value::Null),
                timestamp: response.timestamp,
                error_code,
            }
        })
    }
}
pub struct CaptureInput {
    pub session_id: String,
    pub cwd: PathBuf,
    pub model: Model,
    pub user: String,
    pub assistant: String,
    pub source_timestamp: String,
}
#[derive(Clone)]
pub struct UsageRecord {
    pub day: String,
    pub key: String,
    pub usage: Value,
}
pub type UsageRecorder =
    Arc<dyn Fn(UsageRecord) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
struct OwnedTask {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}
struct SemanticModel {
    model: Arc<dyn MemoryModel>,
    generation: u64,
    cancel: CancellationToken,
}

pub struct MemoryRuntime {
    pub store: Arc<Mutex<MemoryStore>>,
    stop: CancellationToken,
    tasks: Mutex<HashMap<u64, OwnedTask>>,
    next_task: AtomicU64,
    closed: AtomicBool,
    raw_users: Mutex<HashMap<String, String>>,
    semantic: Mutex<Option<SemanticModel>>,
    semantic_generation: AtomicU64,
    semantic_started: AtomicBool,
    semantic_notify: Arc<Notify>,
    semantic_worker: Mutex<Option<JoinHandle<()>>>,
    usage_recorder: Mutex<Option<UsageRecorder>>,
    capture_timeout: Duration,
}
impl MemoryRuntime {
    pub fn new(store: Arc<Mutex<MemoryStore>>, agent_dir: PathBuf) -> Result<Arc<Self>> {
        let config = match std::fs::read(agent_dir.join("pisper.json")) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .context("Pisper preferences contain invalid JSON")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.into()),
        };
        let confidence = normalize_confidence(config.get("memoryAutoApproveConfidence"));
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("Memory store lock failed"))?
            .auto_approve_confidence = confidence / 100.0;
        Ok(Arc::new(Self {
            store,
            stop: CancellationToken::new(),
            tasks: Mutex::new(HashMap::new()),
            next_task: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            raw_users: Mutex::new(HashMap::new()),
            semantic: Mutex::new(None),
            semantic_generation: AtomicU64::new(0),
            semantic_started: AtomicBool::new(false),
            semantic_notify: Arc::new(Notify::new()),
            semantic_worker: Mutex::new(None),
            usage_recorder: Mutex::new(None),
            capture_timeout: Duration::from_secs(120),
        }))
    }
    pub fn create_extension(self: &Arc<Self>) -> InlineExtension {
        super::tools::create_extension(self.clone())
    }
    pub fn set_user_message(&self, session_id: &str, user: &str) {
        if let Ok(mut users) = self.raw_users.lock() {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            users.insert(session_id.to_owned(), user.to_owned());
        }
    }
    pub fn forget_session(&self, session_id: &str) {
        if let Ok(mut users) = self.raw_users.lock() {
            users.remove(session_id);
        }
    }
    pub fn current_user_message(&self, session_id: &str) -> String {
        self.raw_users
            .lock()
            .ok()
            .and_then(|u| u.get(session_id).cloned())
            .unwrap_or_default()
    }
    pub fn set_usage_recorder(&self, recorder: UsageRecorder) {
        if let Ok(mut slot) = self.usage_recorder.lock() {
            *slot = Some(recorder);
        }
    }
    pub fn context_for(
        &self,
        user: &str,
        cwd: &Path,
        enabled: &[String],
        isolated: bool,
    ) -> Result<String> {
        if isolated || !enabled.iter().any(|tool| tool == "memory_search") {
            return Ok(String::new());
        }
        let result = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Memory store lock failed"))?
            .relevant_context(user, cwd, 3)?;
        Ok(result["text"].as_str().unwrap_or("").to_owned())
    }
    pub fn set_semantic_model(self: &Arc<Self>, model: Option<Arc<dyn MemoryModel>>) {
        // 与 shutdown 取走 join handle 使用同一屏障，杜绝停机后才登记的 worker。
        let Ok(mut worker) = self.semantic_worker.lock() else {
            return;
        };
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let generation = self.semantic_generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut slot) = self.semantic.lock() {
            if let Some(old) = slot.take() {
                old.cancel.cancel();
            }
            *slot = model.map(|model| SemanticModel {
                model,
                generation,
                cancel: self.stop.child_token(),
            });
        }
        if let Ok(mut store) = self.store.lock() {
            store.set_semantic_enabled(self.semantic.lock().map(|s| s.is_some()).unwrap_or(false));
        }
        if !self.semantic_started.swap(true, Ordering::AcqRel) {
            let weak = Arc::downgrade(self);
            let stop = self.stop.clone();
            let notify = self.semantic_notify.clone();
            let join = tokio::spawn(async move {
                loop {
                    tokio::select! {_ =stop.cancelled()=>break,_ =notify.notified()=>{}}
                    let Some(runtime) = weak.upgrade() else { break };
                    runtime.run_semantic().await;
                }
            });
            *worker = Some(join);
        }
        drop(worker);
        self.schedule_semantic();
    }
    pub fn schedule_semantic(&self) {
        if !self.closed.load(Ordering::Acquire) {
            self.semantic_notify.notify_one();
        }
    }
    async fn run_semantic(&self) {
        loop {
            if self.stop.is_cancelled() {
                break;
            }
            let selected = self.semantic.lock().ok().and_then(|s| {
                s.as_ref()
                    .map(|s| (s.model.clone(), s.generation, s.cancel.clone()))
            });
            let Some((model, generation, generation_cancel)) = selected else {
                break;
            };
            let cancel = generation_cancel.child_token();
            let batch = match self.store.lock() {
                Ok(mut store) => match store.semantic_batch() {
                    Ok(batch) => batch,
                    Err(_) => break,
                },
                Err(_) => break,
            };
            if batch.is_empty() {
                break;
            }
            let blocks = batch
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    format!(
                        "{}. {}\n{}",
                        index + 1,
                        truncate(row["title"].as_str().unwrap_or(""), 140),
                        truncate(row["content"].as_str().unwrap_or(""), 1000)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let request = CompletionRequest {
                system: SUMMARY_SYSTEM.to_owned(),
                user: format!("Expand each memory into one semantic keyword line:\n\n{blocks}"),
                max_tokens: (batch.len() as u64 * 120).clamp(400, 1200),
            };
            let outcome = tokio::select! {_ =cancel.cancelled()=>None,result=tokio::time::timeout(Duration::from_secs(120),model.complete(request,cancel.clone()))=>Some(result)};
            if generation != self.semantic_generation.load(Ordering::Acquire)
                || self.stop.is_cancelled()
            {
                break;
            }
            match outcome {
                Some(Ok(response)) if response.error_code.is_none() => {
                    let numbering = regex::Regex::new(r"^\s*\d+\.\s*").ok();
                    let summaries = response
                        .text
                        .lines()
                        .map(|line| {
                            numbering
                                .as_ref()
                                .map(|r| r.replace(line, "").to_string())
                                .unwrap_or_else(|| line.to_owned())
                        })
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>();
                    if let Ok(mut store) = self.store.lock() {
                        if store.complete_semantic_batch(&batch, &summaries).is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(response)) => {
                    if let Ok(mut store) = self.store.lock() {
                        let _ = store.fail_semantic_batch(
                            &batch,
                            response.error_code.unwrap_or("model_error"),
                        );
                    }
                    break;
                }
                Some(Err(_)) => {
                    cancel.cancel();
                    if let Ok(mut store) = self.store.lock() {
                        let _ = store.fail_semantic_batch(&batch, "timeout");
                    }
                    break;
                }
                None => break,
            }
        }
        if let Ok(mut store) = self.store.lock() {
            store.set_semantic_running(false);
        }
    }
    pub fn capture(self: &Arc<Self>, runtime: ModelRuntime, input: CaptureInput) -> Option<u64> {
        let model = Arc::new(PiMemoryModel::new(runtime, input.model.clone()));
        self.capture_with_model(model, input)
    }
    fn capture_with_model(
        self: &Arc<Self>,
        model: Arc<dyn MemoryModel>,
        input: CaptureInput,
    ) -> Option<u64> {
        if self.closed.load(Ordering::Acquire) || !should_extract(&input.user) {
            return None;
        }
        let cancel = self.stop.child_token();
        let task_id = self.next_task.fetch_add(1, Ordering::Relaxed);
        let worker_cancel = cancel.clone();
        let weak = Arc::downgrade(self);
        let (start_tx, start_rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            if start_rx.await.is_err() {
                return;
            }
            let Some(service) = weak.upgrade() else {
                return;
            };
            let session = input.session_id.clone();
            let result = tokio::select! {_ =worker_cancel.cancelled()=>Ok(vec![]),result=tokio::time::timeout(service.capture_timeout,service.run_capture(model,input,worker_cancel.clone()))=>match result{Ok(result)=>result,Err(_)=>{worker_cancel.cancel();Err("timeout")}}};
            if let Err(code) = result {
                if !service.stop.is_cancelled() {
                    diagnose(code, &session)
                }
            }
            if let Ok(mut tasks) = service.tasks.lock() {
                tasks.remove(&task_id);
            };
        });
        if let Ok(mut tasks) = self.tasks.lock() {
            if self.closed.load(Ordering::Acquire) {
                join.abort();
                return None;
            }
            tasks.insert(task_id, OwnedTask { cancel, join });
        } else {
            join.abort();
            return None;
        }
        let _ = start_tx.send(());
        Some(task_id)
    }
    async fn run_capture(
        &self,
        model: Arc<dyn MemoryModel>,
        input: CaptureInput,
        cancel: CancellationToken,
    ) -> std::result::Result<Vec<Value>, &'static str> {
        let user = truncate(&crate::security::redact_secret_text(&input.user), 2400);
        let assistant = truncate(&crate::security::redact_secret_text(&input.assistant), 3600);
        let response = model
            .complete(
                CompletionRequest {
                    system: EXTRACT_SYSTEM.to_owned(),
                    user: format!("用户消息：\n{user}\n\nAgent 回复：\n{assistant}"),
                    max_tokens: if input.model.reasoning { 8192 } else { 2048 },
                },
                cancel.clone(),
            )
            .await;
        if cancel.is_cancelled() {
            return Ok(vec![]);
        }
        let recorder = self.usage_recorder.lock().ok().and_then(|r| r.clone());
        if let Some(recorder) = recorder {
            if !response.usage.is_null() {
                let timestamp = if response.timestamp > 0 {
                    response.timestamp
                } else {
                    pi_rust::ai::now_ms()
                };
                let day = Local
                    .timestamp_millis_opt(timestamp)
                    .single()
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                let source = if input.source_timestamp.is_empty() {
                    timestamp.to_string()
                } else {
                    input.source_timestamp.clone()
                };
                if recorder(UsageRecord {
                    day,
                    key: format!("memory:{}:{source}", input.session_id),
                    usage: response.usage.clone(),
                })
                .await
                .is_err()
                {
                    diagnose("usage_write_failed", &input.session_id);
                }
            }
        }
        if cancel.is_cancelled() {
            return Ok(vec![]);
        }
        if let Some(code) = response.error_code {
            if code == "aborted" {
                return Ok(vec![]);
            }
            return Err(code);
        }
        if response.text.trim().is_empty() {
            return Err("empty_response");
        }
        let candidates =
            parse_candidates(&response.text, &user, &assistant).ok_or("invalid_response")?;
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        let mut store = self.store.lock().map_err(|_| "capture_failed")?;
        let project = store
            .ensure_workspace(&input.cwd)
            .map_err(|_| "capture_failed")?;
        let mut result = Vec::new();
        for (index, mut candidate) in candidates.into_iter().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            candidate["spaceId"] = json!(if candidate["scope"] == "global" {
                "global"
            } else {
                &project
            });
            candidate["cwd"] = json!(input.cwd);
            candidate["sessionId"] = json!(input.session_id);
            let timestamp = if response.timestamp > 0 {
                response.timestamp
            } else {
                pi_rust::ai::now_ms()
            };
            let source = if input.source_timestamp.is_empty() {
                timestamp.to_string()
            } else {
                input.source_timestamp.clone()
            };
            candidate["sourceId"] = json!(format!("{}:{source}:{index}", input.session_id));
            candidate["sourceTimestamp"] = json!(if input.source_timestamp.is_empty() {
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                    .unwrap_or_default()
            } else {
                input.source_timestamp.clone()
            });
            candidate["sourceType"] = json!("conversation");
            result.push(store.propose(&candidate).map_err(|_| "capture_failed")?);
        }
        drop(store);
        self.schedule_semantic();
        Ok(result)
    }
    pub async fn drain(&self) {
        loop {
            let done = self.tasks.lock().map(|t| t.is_empty()).unwrap_or(true);
            if done {
                break;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    pub async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.stop.cancel();
        if let Ok(slot) = self.semantic.lock() {
            if let Some(model) = slot.as_ref() {
                model.cancel.cancel();
            }
        }
        let tasks = self
            .tasks
            .lock()
            .map(|mut tasks| tasks.drain().map(|(_, t)| t).collect::<Vec<_>>())
            .unwrap_or_default();
        for task in tasks {
            task.cancel.cancel();
            let _ = task.join.await;
        }
        let worker = self.semantic_worker.lock().ok().and_then(|mut w| w.take());
        if let Some(worker) = worker {
            let _ = worker.await;
        }
        if let Ok(mut store) = self.store.lock() {
            store.set_semantic_running(false);
        }
        if let Ok(mut users) = self.raw_users.lock() {
            users.clear();
        }
    }
}
fn diagnose(code: &str, session: &str) {
    let session = Sha256::digest(session)
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    tracing::warn!(code, session, "memory capture failed");
}
fn truncate(value: &str, limit: usize) -> String {
    let mut length = 0;
    value
        .chars()
        .take_while(|c| {
            length += c.len_utf16();
            length <= limit
        })
        .collect()
}
pub fn should_extract(user: &str) -> bool {
    let user = user.trim().to_lowercase();
    if user.encode_utf16().count() < 8 {
        return false;
    }
    [
        "记住",
        "记下来",
        "请记下",
        "以后",
        "从今以后",
        "今后",
        "长期",
        "偏好",
        "习惯",
        "约定",
        "决定",
        "确认采用",
        "确认使用",
        "最终方案",
        "暂时不引入",
        "不再使用",
        "改用",
        "改为",
        "不要再",
        "remember",
        "preference",
        "from now on",
        "we decided",
        "use instead",
    ]
    .iter()
    .any(|word| user.contains(word))
}
pub fn normalize_confidence(value: Option<&Value>) -> f64 {
    let n = value
        .and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => {
                if s.trim().is_empty() {
                    Some(0.0)
                } else {
                    s.trim().parse().ok()
                }
            }
            Value::Null => Some(0.0),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        })
        .filter(|n| n.is_finite())
        .unwrap_or(60.0);
    n.clamp(0.0, 100.0).round()
}
pub fn parse_candidates(text: &str, user: &str, assistant: &str) -> Option<Vec<Value>> {
    let text = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    let items = serde_json::from_str::<Value>(&text[start..=end]).ok()?;
    let mut result = Vec::new();
    for item in items.as_array()?.iter().take(3) {
        let title = truncate(item["title"].as_str().unwrap_or("").trim(), 140);
        let content = truncate(item["content"].as_str().unwrap_or("").trim(), 4000);
        let evidence = truncate(item["evidence"].as_str().unwrap_or("").trim(), 1000);
        if title.is_empty()
            || content.is_empty()
            || evidence.is_empty()
            || evidence.contains("[REDACTED SECRET]")
            || (!user.contains(&evidence) && !assistant.contains(&evidence))
        {
            continue;
        }
        let type_name = item["type"]
            .as_str()
            .filter(|t| ["preference", "decision", "fact", "risk", "task"].contains(t))
            .unwrap_or("fact");
        let importance = item["importance"]
            .as_f64()
            .filter(|n| n.is_finite() && *n != 0.0)
            .unwrap_or(0.5)
            .clamp(0.1, 1.0);
        let confidence = item["confidence"]
            .as_f64()
            .filter(|n| n.is_finite() && *n != 0.0)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        result.push(json!({"title":crate::security::redact_secret_text(&title),"content":crate::security::redact_secret_text(&content),"topic":truncate(item["topic"].as_str().unwrap_or("").trim(),180),"type":type_name,"scope":if item["scope"]=="global"{"global"}else{"project"},"importance":importance,"confidence":confidence,"evidence":evidence}));
    }
    Some(result)
}
pub fn session_id(
    manager: pi_rust::coding_agent::extensions::types::SessionManagerHandle,
) -> Result<String, String> {
    let manager = manager
        .downcast::<Mutex<SessionManager>>()
        .map_err(|_| "Memory tool session manager unavailable".to_owned())?;
    let result = manager
        .lock()
        .map_err(|_| "Session manager lock failed".to_owned())?
        .get_session_id()
        .to_owned();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_rust::coding_agent::core::model_runtime::CreateModelRuntimeOptions;
    use std::sync::atomic::AtomicUsize;

    fn fixture() -> (PathBuf, Arc<MemoryRuntime>) {
        let root = std::env::temp_dir().join(format!(
            "pisper-memory-runtime-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Arc::new(Mutex::new(
            MemoryStore::open(root.join("pisper-memory.sqlite"), &root).unwrap(),
        ));
        let runtime = MemoryRuntime::new(store, root.clone()).unwrap();
        (root, runtime)
    }
    fn model() -> Model {
        serde_json::from_value(json!({"id":"memory-fixture","name":"Memory fixture","api":"openai-completions","provider":"memory-fixture","baseUrl":"http://127.0.0.1/v1","reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":2048})).unwrap()
    }
    fn input(root: &Path) -> CaptureInput {
        CaptureInput {
            session_id: "synthetic-session".to_owned(),
            cwd: root.to_owned(),
            model: model(),
            user: "请记住以后用 Rust 开发服务".to_owned(),
            assistant: "好的，后续服务使用 Rust。".to_owned(),
            source_timestamp: "2026-10-05T10:00:00.000Z".to_owned(),
        }
    }
    struct FixedModel {
        requests: Arc<Mutex<Vec<CompletionRequest>>>,
        response: String,
    }
    impl MemoryModel for FixedModel {
        fn complete(
            &self,
            request: CompletionRequest,
            _: CancellationToken,
        ) -> BoxFuture<'static, Completion> {
            self.requests.lock().unwrap().push(request);
            let text = self.response.clone();
            Box::pin(async move {
                Completion {
                    text,
                    usage: json!({"input":12,"output":4,"totalTokens":16}),
                    timestamp: 1791180000000,
                    error_code: None,
                }
            })
        }
    }
    #[test]
    fn extraction_matches_release_evidence_and_secret_boundaries() {
        assert!(!should_extract("记住"));
        assert!(!should_extract("今天的天气怎么样？"));
        assert!(should_extract("from now on please use rust"));
        let source = "请记住以后用 Rust 开发服务";
        let result = parse_candidates(&json!([
            {"title":"服务语言","content":"服务采用 Rust","evidence":source,"confidence":0.4,"scope":"project"},
            {"title":"编造","content":"没有来源的内容","evidence":"根本不在原文"},
            {"title":"泄密","content":"token=[REDACTED SECRET]","evidence":"[REDACTED SECRET]"}
        ]).to_string(), source, "").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["confidence"], 0.4);
        assert_eq!(normalize_confidence(None), 60.0);
        assert_eq!(normalize_confidence(Some(&json!("40.5"))), 41.0);
        assert_eq!(normalize_confidence(Some(&Value::Null)), 0.0);
    }
    #[tokio::test]
    async fn capture_records_usage_and_persists_supported_candidates() {
        let (root, service) = fixture();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let received = records.clone();
        service.set_usage_recorder(Arc::new(move |record| {
            received.lock().unwrap().push(record);
            Box::pin(async { Ok(()) })
        }));
        let model = Arc::new(FixedModel { requests: requests.clone(), response: json!([{"title":"服务语言","content":"服务采用 Rust","topic":"runtime.language","confidence":0.4,"evidence":"请记住以后用 Rust 开发服务"}]).to_string() });
        service.capture_with_model(model, input(&root)).unwrap();
        service.drain().await;
        let inbox = service.store.lock().unwrap().candidate_inbox(5).unwrap();
        assert_eq!(inbox["count"], 1);
        assert_eq!(inbox["candidates"][0]["sourceType"], "conversation");
        assert_eq!(
            inbox["candidates"][0]["sourceId"],
            "synthetic-session:2026-10-05T10:00:00.000Z:0"
        );
        assert_eq!(
            records.lock().unwrap()[0].key,
            "memory:synthetic-session:2026-10-05T10:00:00.000Z"
        );
        assert!(requests.lock().unwrap()[0]
            .system
            .contains("Do not treat the assistant claiming"));
        service.shutdown().await;
        drop(service);
        let restarted = MemoryStore::open(root.join("pisper-memory.sqlite"), &root).unwrap();
        assert_eq!(restarted.candidate_inbox(5).unwrap()["count"], 1);
        drop(restarted);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn stale_semantic_result_does_not_overwrite_user_edit() {
        let (root, service) = fixture();
        let mut store = service.store.lock().unwrap();
        let memory = store
            .remember(&json!({"spaceId":"global","title":"原始事实","content":"原始内容"}))
            .unwrap();
        let batch = store.semantic_batch().unwrap();
        store
            .update_memory(
                memory["id"].as_str().unwrap(),
                &json!({"content":"更新后的内容"}),
            )
            .unwrap();
        store
            .complete_semantic_batch(&batch, &["old semantic keywords".to_owned()])
            .unwrap();
        let updated = store
            .get_memory(memory["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(updated["semanticStatus"], "pending");
        assert_eq!(updated["semanticText"], "");
        drop(store);
        service.shutdown().await;
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    struct DropCount(Arc<AtomicUsize>);
    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    struct PendingModel {
        started: Arc<Notify>,
        dropped: Arc<AtomicUsize>,
    }
    impl MemoryModel for PendingModel {
        fn complete(
            &self,
            _: CompletionRequest,
            cancel: CancellationToken,
        ) -> BoxFuture<'static, Completion> {
            let started = self.started.clone();
            let dropped = self.dropped.clone();
            Box::pin(async move {
                let _owned = DropCount(dropped);
                started.notify_one();
                cancel.cancelled().await;
                Completion {
                    text: String::new(),
                    usage: Value::Null,
                    timestamp: 0,
                    error_code: Some("aborted"),
                }
            })
        }
    }
    #[tokio::test]
    async fn shutdown_cancels_and_joins_live_capture_without_writing_candidates() {
        let (root, service) = fixture();
        let started = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicUsize::new(0));
        service
            .capture_with_model(
                Arc::new(PendingModel {
                    started: started.clone(),
                    dropped: dropped.clone(),
                }),
                input(&root),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), service.shutdown())
            .await
            .unwrap();
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(service.tasks.lock().unwrap().is_empty());
        assert_eq!(
            service.store.lock().unwrap().candidate_inbox(5).unwrap()["count"],
            0
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn pi_provider_http_drives_semantic_summary_and_conversation_capture() {
        use axum::{
            extract::State, http::header, response::IntoResponse, routing::post, Json, Router,
        };
        let (root, service) = fixture();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        async fn completion(
            State(requests): State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> impl IntoResponse {
            requests.lock().unwrap().push(body.clone());
            let response = if body
                .to_string()
                .contains("You expand a Pisper memory entry")
            {
                "rust cargo compiler preferences".to_owned()
            } else {
                json!([{"title":"服务语言","content":"服务采用 Rust","topic":"runtime.language","confidence":0.4,"evidence":"请记住以后用 Rust 开发服务"}]).to_string()
            };
            let chunks = [
                json!({"id":"fixture","object":"chat.completion.chunk","model":"memory-fixture","choices":[{"index":0,"delta":{"role":"assistant","content":response},"finish_reason":null}]}),
                json!({"id":"fixture","object":"chat.completion.chunk","model":"memory-fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16}}),
            ];
            let body = chunks
                .iter()
                .map(|chunk| format!("data: {chunk}\n\n"))
                .collect::<String>()
                + "data: [DONE]\n\n";
            ([(header::CONTENT_TYPE, "text/event-stream")], body)
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/v1/chat/completions", post(completion))
            .with_state(requests.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let config = json!({"providers":{"memory-fixture":{"baseUrl":format!("http://{address}/v1"),"api":"openai-completions","apiKey":"synthetic-memory-fixture-key","models":[{"id":"memory-fixture","name":"Memory fixture","reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":2048}]}}});
        std::fs::write(
            root.join("models.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            auth_path: Some(root.join("auth.json").to_string_lossy().to_string()),
            models_path: Some(Some(root.join("models.json").to_string_lossy().to_string())),
            models_store_path: Some(root.join("models-store.json").to_string_lossy().to_string()),
            allow_model_network: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let fixture_model = runtime
            .get_model_sync("memory-fixture", "memory-fixture")
            .unwrap();
        service.store.lock().unwrap().remember(&json!({"spaceId":"global","title":"Rust preference","content":"rust compiler cargo"})).unwrap();
        service.set_semantic_model(Some(Arc::new(PiMemoryModel::new(
            runtime.clone(),
            fixture_model.clone(),
        ))));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if service.store.lock().unwrap().semantic_status().unwrap()["ready"] == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            service
                .store
                .lock()
                .unwrap()
                .dashboard("global", "")
                .unwrap()["nodes"][0]["semanticText"],
            "rust cargo compiler preferences"
        );
        let mut capture = input(&root);
        capture.model = fixture_model;
        service.capture(runtime, capture).unwrap();
        service.drain().await;
        assert_eq!(
            service.store.lock().unwrap().candidate_inbox(5).unwrap()["count"],
            1
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(requests.lock().unwrap()[0]["temperature"], 0.1);
        service.shutdown().await;
        server.abort();
        let _ = server.await;
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
}
