//! 微信二维码/数字配对码登录；每个轮询任务只持有作业数据，不反向持有服务或句柄。
use super::weixin_protocol::{error, pause, string, text, truthy, WeixinProtocol};
use super::{CompletedSink, Onboarding, Result};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

struct JobState {
    id: String,
    status: String,
    qrcode: String,
    qr_url: String,
    qr_data_url: String,
    expire_at: String,
    deadline: Instant,
    error: String,
    base_url: String,
    verify_code: String,
    verify_version: u64,
}
struct Job {
    state: Mutex<JobState>,
    cancellation: CancellationToken,
}
struct Entry {
    job: Arc<Job>,
    task: Option<JoinHandle<()>>,
}
pub(crate) struct WeixinOnboardingService {
    protocol: Arc<WeixinProtocol>,
    completed: CompletedSink,
    jobs: Mutex<HashMap<String, Entry>>,
    action: AsyncMutex<()>,
    cancellation: CancellationToken,
    closed: AtomicBool,
}
fn expiry() -> (String, Instant) {
    (
        (chrono::Utc::now() + chrono::Duration::minutes(5))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        Instant::now() + Duration::from_secs(300),
    )
}
fn public(job: &Job) -> Value {
    let job = job.state.lock().unwrap();
    json!({"id":job.id,"platform":"weixin","status":job.status,"qrUrl":job.qr_url,"qrDataUrl":job.qr_data_url,"expireAt":job.expire_at,"error":job.error,"needsVerifyCode":job.status=="verification_required"})
}
impl WeixinOnboardingService {
    pub(crate) fn new(completed: CompletedSink) -> Arc<Self> {
        Self::with_protocol(completed, Arc::new(WeixinProtocol::new()))
    }
    pub(crate) fn with_protocol(
        completed: CompletedSink,
        protocol: Arc<WeixinProtocol>,
    ) -> Arc<Self> {
        Arc::new(Self {
            protocol,
            completed,
            jobs: Mutex::new(HashMap::new()),
            action: AsyncMutex::new(()),
            cancellation: CancellationToken::new(),
            closed: AtomicBool::new(false),
        })
    }
    async fn poll(
        job: Arc<Job>,
        protocol: Arc<WeixinProtocol>,
        completed: CompletedSink,
    ) -> Result<()> {
        let mut refreshes = 0;
        loop {
            if job.cancellation.is_cancelled() {
                return Ok(());
            }
            let (qrcode, base, code, deadline) = {
                let state = job.state.lock().unwrap();
                (
                    state.qrcode.clone(),
                    state.base_url.clone(),
                    state.verify_code.clone(),
                    state.deadline,
                )
            };
            if Instant::now() >= deadline {
                return Err(error("微信扫码登录已超时。"));
            }
            let result = protocol
                .poll_qr(&qrcode, &base, &code, &job.cancellation)
                .await?;
            if job.cancellation.is_cancelled() {
                return Ok(());
            }
            match result["status"].as_str().unwrap_or("") {
                "wait" => job.state.lock().unwrap().status = "waiting".into(),
                "scaned" => {
                    let mut state = job.state.lock().unwrap();
                    state.status = "scanned".into();
                    state.verify_code.clear();
                }
                "need_verifycode" => {
                    let version = {
                        let mut state = job.state.lock().unwrap();
                        state.status = "verification_required".into();
                        state.verify_version
                    };
                    while !job.cancellation.is_cancelled()
                        && job.state.lock().unwrap().verify_version == version
                    {
                        // 配对码等待也受同一个作业期限约束；不能因服务端状态停留而遗留无限任务。
                        if Instant::now() >= job.state.lock().unwrap().deadline {
                            return Err(error("微信扫码登录已超时。"));
                        }
                        pause(&job.cancellation, 500).await;
                    }
                    continue;
                }
                "verify_code_blocked" => return Err(error("配对码多次输入错误，请重新扫码。")),
                "scaned_but_redirect" => {
                    let mut state = job.state.lock().unwrap();
                    if truthy(&result["redirect_host"]) {
                        state.base_url = format!("https://{}", text(&result, "redirect_host"));
                    }
                    state.status = "scanned".into();
                }
                "binded_redirect" => {
                    return Err(error("该微信已绑定当前机器人，请先解除旧连接后重试。"))
                }
                "expired" => {
                    refreshes += 1;
                    if refreshes > 2 {
                        return Err(error("微信二维码已多次过期，请重新开始。"));
                    }
                    let next = protocol.start_qr(json!([]), &job.cancellation).await?;
                    let qrcode = text(&next, "qrcode");
                    let qr_url = text(&next, "qrcode_img_content");
                    let qr_data = super::qr::data_url(&qr_url, 248, 2)?;
                    let (expire_at, deadline) = expiry();
                    let mut state = job.state.lock().unwrap();
                    state.qrcode = qrcode;
                    state.qr_url = qr_url;
                    state.qr_data_url = qr_data;
                    state.expire_at = expire_at;
                    state.deadline = deadline;
                    state.status = "waiting".into();
                }
                "confirmed" => {
                    if !truthy(&result["bot_token"]) || !truthy(&result["ilink_bot_id"]) {
                        return Err(error("微信确认成功，但登录凭据不完整。"));
                    }
                    job.state.lock().unwrap().status = "connecting".into();
                    let base = if truthy(&result["baseurl"]) {
                        result["baseurl"].clone()
                    } else {
                        json!(job.state.lock().unwrap().base_url.clone())
                    };
                    let future = completed(
                        json!({"accountId":result["ilink_bot_id"],"token":result["bot_token"],"ownerUserId":if truthy(&result["ilink_user_id"]){result["ilink_user_id"].clone()}else{json!("")},"baseUrl":base,"cdnBaseUrl":protocol.cdn_base()}),
                    );
                    tokio::select! {biased;_=job.cancellation.cancelled()=>return Ok(()), result=future=>{result?;}}
                    if !job.cancellation.is_cancelled() {
                        job.state.lock().unwrap().status = "completed".into();
                    }
                    return Ok(());
                }
                _ => {}
            }
            pause(&job.cancellation, 800).await;
        }
    }
    fn pending(&self, all: bool) -> Vec<JoinHandle<()>> {
        let mut tasks = Vec::new();
        let mut jobs = self.jobs.lock().unwrap();
        for entry in jobs.values_mut() {
            let terminal = matches!(
                entry.job.state.lock().unwrap().status.as_str(),
                "completed" | "failed" | "cancelled"
            );
            if all || !terminal {
                entry.job.cancellation.cancel();
                if let Some(task) = entry.task.take() {
                    tasks.push(task)
                }
            }
        }
        tasks
    }
}
impl Onboarding for WeixinOnboardingService {
    fn start(&self, options: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let _action = self.action.lock().await;
            if self.closed.load(Ordering::Acquire) {
                return Err(error("微信接入服务已关闭。"));
            }
            for task in self.pending(false) {
                task.await.map_err(|e| error(e.to_string()))?;
            }
            let response = self
                .protocol
                .start_qr(options["localTokens"].clone(), &self.cancellation)
                .await?;
            if !truthy(&response["qrcode"]) || !truthy(&response["qrcode_img_content"]) {
                return Err(error("微信登录服务未返回二维码。"));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let qr_url = text(&response, "qrcode_img_content");
            let qr_data_url = super::qr::data_url(&qr_url, 248, 2)?;
            let (expire_at, deadline) = expiry();
            if self.cancellation.is_cancelled() {
                return Err(error("微信接入服务已关闭。"));
            }
            let job = Arc::new(Job {
                state: Mutex::new(JobState {
                    id: id.clone(),
                    status: "waiting".into(),
                    qrcode: text(&response, "qrcode"),
                    qr_url,
                    qr_data_url,
                    expire_at,
                    deadline,
                    error: String::new(),
                    base_url: self.protocol.api_base().into(),
                    verify_code: String::new(),
                    verify_version: 0,
                }),
                cancellation: self.cancellation.child_token(),
            });
            let initial = public(&job);
            let task_job = job.clone();
            let protocol = self.protocol.clone();
            let completed = self.completed.clone();
            let task = tokio::spawn(async move {
                let result = Self::poll(task_job.clone(), protocol, completed).await;
                let mut state = task_job.state.lock().unwrap();
                if task_job.cancellation.is_cancelled() {
                    state.status = "cancelled".into();
                } else if let Err(e) = result {
                    state.status = "failed".into();
                    state.error = e.message;
                }
            });
            self.jobs.lock().unwrap().insert(
                id,
                Entry {
                    job,
                    task: Some(task),
                },
            );
            Ok(initial)
        })
    }
    fn get(&self, id: &str) -> Option<Value> {
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .map(|entry| public(&entry.job))
    }
    fn cancel(&self, id: &str) -> bool {
        let jobs = self.jobs.lock().unwrap();
        let Some(entry) = jobs.get(id) else {
            return false;
        };
        entry.job.cancellation.cancel();
        entry.job.state.lock().unwrap().status = "cancelled".into();
        true
    }
    fn verify(&self, id: &str, code: Value) -> Result<Option<Value>> {
        let jobs = self.jobs.lock().unwrap();
        let Some(entry) = jobs.get(id) else {
            return Ok(None);
        };
        let value = if truthy(&code) {
            string(&code).trim().to_owned()
        } else {
            String::new()
        };
        if !(4..=8).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(error("请输入微信显示的数字配对码。"));
        }
        {
            let mut state = entry.job.state.lock().unwrap();
            state.verify_code = value;
            state.verify_version += 1;
            state.status = "scanned".into();
        }
        Ok(Some(public(&entry.job)))
    }
    fn dispose(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.closed.store(true, Ordering::Release);
            self.cancellation.cancel();
            let _action = self.action.lock().await;
            let mut failures = Vec::new();
            for task in self.pending(true) {
                if let Err(e) = task.await {
                    failures.push(e.to_string());
                }
            }
            self.jobs.lock().unwrap().clear();
            if failures.is_empty() {
                Ok(())
            } else {
                Err(error(failures.join("; ")))
            }
        })
    }
}
