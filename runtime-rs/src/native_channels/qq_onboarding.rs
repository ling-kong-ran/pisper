//! 腾讯 QQ Bot Connector 1.2.0 的绑定协议；AES-GCM 校验失败不能交付凭据。
#[path = "feishu_qq/common.rs"]
mod common;
use super::{ChannelError, CompletedSink, Onboarding, Result};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct Endpoints {
    pub(crate) api: String,
    pub(crate) page: String,
    pub(crate) poll_interval: Duration,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: "https://q.qq.com".into(),
            page: "https://q.qq.com/qqbot/openclaw/connect.html".into(),
            poll_interval: Duration::from_secs(2),
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

pub(crate) fn decrypt_secret(encoded: &str, key: &[u8]) -> Result<String> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ChannelError::new("QQ 绑定凭据编码无效。"))?;
    if key.len() != 32 || bytes.len() < 28 {
        return Err(ChannelError::new("QQ 绑定凭据长度无效。"));
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| ChannelError::new("QQ 绑定密钥无效。"))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
        .map_err(|_| ChannelError::new("QQ 绑定凭据认证失败。"))?;
    String::from_utf8(plain).map_err(|_| ChannelError::new("QQ 绑定凭据不是有效 UTF-8。"))
}
async fn connector_request(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    path: &str,
    body: Value,
    token: &CancellationToken,
) -> Result<Value> {
    let response = common::response(
        client
            .post(format!("{}{}", endpoints.api.trim_end_matches('/'), path))
            .json(&body),
        token,
    )
    .await?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(ChannelError::new(format!(
            "HTTP {} from QQ binding endpoint",
            response.status().as_u16()
        )));
    }
    let value: Value = tokio::select! {biased;_=token.cancelled()=>return Err(common::cancelled()),value=response.json()=>value.map_err(|_|ChannelError::new("QQ 绑定接口返回无效 JSON。"))?};
    if value["retcode"].as_i64() != Some(0) {
        return Err(ChannelError::new(
            value["msg"].as_str().unwrap_or("QQ 绑定接口失败。"),
        ));
    }
    Ok(value)
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
        loop{
            if job.token.is_cancelled(){return Err(common::cancelled())}
            let mut key=[0u8;32];getrandom::getrandom(&mut key).map_err(|error|ChannelError::new(error.to_string()))?;
            let task=connector_request(&client,&endpoints,"/lite/create_bind_task",json!({"key":STANDARD.encode(key)}),&job.token).await.map_err(|error|ChannelError::new(format!("获取绑定任务失败: {}",error.message)))?;
            let task_id=task["data"]["task_id"].as_str().filter(|value|!value.is_empty()).ok_or_else(||ChannelError::new("create_bind_task: missing task_id"))?;
            let mut url=reqwest::Url::parse(&endpoints.page).map_err(|_|ChannelError::new("QQ 二维码地址无效。"))?;
            url.query_pairs_mut().append_pair("task_id",task_id).append_pair("source","pisper").append_pair("_wv","2");
            let qr=super::qr::data_url(url.as_str(),248,2)?;
            job.patch(json!({"qrUrl":url.to_string(),"qrDataUrl":qr,"expireAt":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"status":"waiting"}));
            if let Some(ready)=ready.take(){let _=ready.send(Ok(job.snapshot()));}
            loop{
                let value=connector_request(&client,&endpoints,"/lite/poll_bind_result",json!({"task_id":task_id}),&job.token).await;
                match value{
                    Ok(value)=>match value["data"]["status"].as_i64().unwrap_or(0){
                        2=>{
                            let id=common::text(&value["data"]["bot_appid"]);let secret=decrypt_secret(value["data"]["bot_encrypt_secret"].as_str().unwrap_or(""),&key)?;
                            if id.is_empty()||secret.is_empty(){return Err(ChannelError::new("QQ 扫码成功，但未返回完整的官方机器人凭据。"))}
                            job.patch(json!({"status":"connecting"}));
                            completed(json!({"appId":id,"appSecret":secret,"ownerUserId":value["data"]["user_openid"].as_str().unwrap_or("")})).await?;
                            job.patch(json!({"status":"completed"}));return Ok(())
                        },3=>{job.patch(json!({"status":"waiting"}));break},_=>{}
                    },Err(_)if job.token.is_cancelled()=>return Err(common::cancelled()),Err(_)=>{}
                }
                tokio::select!{biased;_=job.token.cancelled()=>return Err(common::cancelled()),_=tokio::time::sleep(endpoints.poll_interval)=>{}}
            }
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
                    if !matches!(
                        job.snapshot()["status"].as_str(),
                        Some("completed" | "failed" | "cancelled")
                    ) {
                        job.token.cancel();
                        job.patch(json!({"status":"cancelled"}));
                    }
                }
            }
            let id = uuid::Uuid::new_v4().to_string();
            let job = Arc::new(Job {
                public: Mutex::new(
                    json!({"id":id,"platform":"qq","mode":"qr","status":"starting","qrUrl":"","qrDataUrl":"","expireAt":null,"error":""}),
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
                Ok(Err(_)) => Err(ChannelError::new("QQ 扫码地址生成失败。")),
                Err(_) => Err(ChannelError::new("QQ 扫码地址生成超时。")),
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
#[path = "feishu_qq/qq_onboarding_tests.rs"]
mod tests;
