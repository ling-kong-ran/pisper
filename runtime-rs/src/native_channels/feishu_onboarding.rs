//! 飞书 Node SDK 1.72.0 的 PersonalAgent 设备授权流程，保持创建参数与域切换。
#[path = "feishu_qq/common.rs"]
mod common;
use super::{ChannelError, CompletedSink, Onboarding, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct Endpoints {
    pub(crate) feishu: String,
    pub(crate) lark: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            feishu: "https://accounts.feishu.cn".into(),
            lark: "https://accounts.larksuite.com".into(),
        }
    }
}
struct Job {
    public: Mutex<Value>,
    token: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Job {
    fn snapshot(&self) -> Value {
        self.public
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn patch(&self, patch: Value) {
        let mut public = self
            .public
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let (Some(public), Some(patch)) = (public.as_object_mut(), patch.as_object()) {
            public.extend(patch.clone())
        }
    }
}
struct Service {
    completed: CompletedSink,
    endpoints: Endpoints,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}
pub(crate) fn new(completed: CompletedSink) -> Arc<dyn Onboarding> {
    new_with_endpoints(completed, Endpoints::default())
}
pub(crate) fn new_with_endpoints(
    completed: CompletedSink,
    endpoints: Endpoints,
) -> Arc<dyn Onboarding> {
    Arc::new(Service {
        completed,
        endpoints,
        jobs: Mutex::new(HashMap::new()),
    })
}
fn addons() -> Result<String> {
    let value = json!({"preset":false,"scopes":{"tenant":["im:message:send_as_bot","im:message.p2p_msg:readonly","im:message.group_at_msg:readonly","im:resource"]},"events":{"items":{"tenant":["im.message.receive_v1"]}}});
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(value.to_string().as_bytes())
        .map_err(|error| ChannelError::new(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(
        gzip.finish()
            .map_err(|error| ChannelError::new(error.to_string()))?,
    ))
}
async fn registration(
    client: &reqwest::Client,
    base: &str,
    parameters: &[(&str, &str)],
    token: &CancellationToken,
) -> Result<Value> {
    let response = common::response(
        client
            .post(format!(
                "{}/oauth/v1/app/registration",
                base.trim_end_matches('/')
            ))
            .form(parameters),
        token,
    )
    .await?;
    // RFC8628 的待授权/降频等状态使用 HTTP400；先保留响应体再按 error 分类。
    tokio::select! {biased;_=token.cancelled()=>Err(common::cancelled()),value=response.json()=>value.map_err(|_|ChannelError::new("飞书注册接口返回无效 JSON。"))}
}
async fn run(
    job: Arc<Job>,
    completed: CompletedSink,
    endpoints: Endpoints,
    ready: tokio::sync::oneshot::Sender<Result<Value>>,
) {
    let mut ready = Some(ready);
    let result=async{
        let client=common::client();
        let begin=registration(&client,&endpoints.feishu,&[("action","begin"),("archetype","PersonalAgent"),("auth_method","client_secret"),("request_user_info","open_id")],&job.token).await?;
        let mut qr=reqwest::Url::parse(begin["verification_uri_complete"].as_str().unwrap_or("")).map_err(|_|ChannelError::new("飞书注册响应缺少扫码地址。"))?;
        qr.query_pairs_mut().append_pair("from","sdk").append_pair("source","node-sdk/pisper").append_pair("tp","sdk").append_pair("name","Pisper Agent").append_pair("desc","通过飞书与本机 Pisper Agent 进行双向对话").append_pair("addons",&addons()?).append_pair("createOnly","true");
        let expires=begin["expires_in"].as_u64().unwrap_or(600);let mut interval=Duration::from_secs(begin["interval"].as_u64().unwrap_or(5));
        let device=begin["device_code"].as_str().filter(|value|!value.is_empty()).ok_or_else(||ChannelError::new("飞书注册响应缺少 device_code。"))?.to_owned();
        job.patch(json!({"qrUrl":qr.to_string(),"expireAt":(chrono::Utc::now()+chrono::Duration::seconds(expires.min(i64::MAX as u64) as i64)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"status":"waiting"}));
        let png=super::qr::data_url(qr.as_str(),248,2)?;job.patch(json!({"qrDataUrl":png}));
        if let Some(ready)=ready.take(){let _=ready.send(Ok(job.snapshot()));}
        let expiry=tokio::time::Instant::now()+Duration::from_secs(expires);let mut base=endpoints.feishu;let mut switched=false;
        loop{
            let poll_parameters = [("action","poll"),("device_code",&device)];
            let value=tokio::select!{biased;_=job.token.cancelled()=>return Err(common::cancelled()),_=tokio::time::sleep_until(expiry)=>return Err(ChannelError::new("Polling timed out")),value=registration(&client,&base,&poll_parameters,&job.token)=>value?};
            if value["user_info"]["tenant_brand"]=="lark"&&!switched{base=endpoints.lark.clone();switched=true;job.patch(json!({"status":"authorizing"}));continue}
            let id=value["client_id"].as_str().unwrap_or("");let secret=value["client_secret"].as_str().unwrap_or("");
            if !id.is_empty()&&!secret.is_empty(){
                job.patch(json!({"status":"connecting"}));
                completed(json!({"appId":id,"appSecret":secret,"ownerOpenId":value["user_info"]["open_id"].as_str().unwrap_or(""),"domain":if value["user_info"]["tenant_brand"]=="lark"{"lark"}else{"feishu"}})).await?;
                job.patch(json!({"status":"completed"}));return Ok(())
            }
            match value["error"].as_str(){
                Some("authorization_pending")=>{},Some("slow_down")=>interval+=Duration::from_secs(5),
                Some(_)=>return Err(ChannelError::new(value["error_description"].as_str().unwrap_or("Unknown error"))),None=>{}
            }
            tokio::select!{biased;_=job.token.cancelled()=>return Err(common::cancelled()),_=tokio::time::sleep_until(expiry)=>return Err(ChannelError::new("Polling timed out")),_=tokio::time::sleep(interval)=>{}}
        }
    }.await;
    if let Err(error) = result {
        if job.token.is_cancelled() {
            job.patch(json!({"status":"cancelled"}))
        } else {
            job.patch(json!({"status":"failed","error":error.message}))
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(Err(error));
        }
    }
}
impl Onboarding for Service {
    fn start(&self, _options: Value) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            {
                let jobs = self
                    .jobs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for job in jobs.values() {
                    if matches!(
                        job.snapshot()["status"].as_str(),
                        Some("starting" | "waiting" | "authorizing" | "connecting")
                    ) {
                        job.token.cancel();
                        job.patch(json!({"status":"cancelled"}));
                    }
                }
            }
            let id = uuid::Uuid::new_v4().to_string();
            let job = Arc::new(Job {
                public: Mutex::new(
                    json!({"id":id,"status":"starting","qrUrl":"","qrDataUrl":"","userCode":"","expireAt":null,"error":""}),
                ),
                token: CancellationToken::new(),
                task: Mutex::new(None),
            });
            self.jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, job.clone());
            let (send, receive) = tokio::sync::oneshot::channel();
            let work = job.clone();
            let completed = self.completed.clone();
            let endpoints = self.endpoints.clone();
            *job.task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(tokio::spawn(async move {
                    run(work, completed, endpoints, send).await
                }));
            match tokio::time::timeout(Duration::from_secs(15), receive).await {
                Ok(Ok(value)) => value,
                Ok(Err(_)) => Err(ChannelError::new("飞书扫码地址生成失败。")),
                Err(_) => Err(ChannelError::new("飞书扫码地址生成超时。")),
            }
        })
    }
    fn get(&self, id: &str) -> Option<Value> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .map(|job| job.snapshot())
    }
    fn cancel(&self, id: &str) -> bool {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job) = jobs.get(id) else {
            return false;
        };
        job.token.cancel();
        job.patch(json!({"status":"cancelled"}));
        true
    }
    fn verify(&self, _id: &str, _code: Value) -> Result<Option<Value>> {
        Ok(None)
    }
    fn dispose(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let jobs = {
                let mut jobs = self
                    .jobs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                jobs.drain().map(|(_, job)| job).collect::<Vec<_>>()
            };
            for job in &jobs {
                job.token.cancel()
            }
            for job in jobs {
                let task = job
                    .task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(task) = task {
                    let _ = task.await;
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "feishu_qq/feishu_onboarding_tests.rs"]
mod tests;
