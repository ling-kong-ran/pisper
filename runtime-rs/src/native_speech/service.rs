use super::{
    catalog::Catalog,
    downloads::SpeechDownloads,
    error::{engine, Result},
    native,
    terms::SpeechTerms,
};
use base64::Engine;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{oneshot, Mutex as AsyncMutex, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
struct Worker {
    child: AsyncMutex<Child>,
    input: AsyncMutex<ChildStdin>,
    output: AsyncMutex<BufReader<ChildStdout>>,
    closed: AtomicBool,
}
impl Worker {
    async fn unload(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let graceful = async {
            let mut input = self.input.lock().await;
            input.write_all(b"{\"method\":\"shutdown\"}\n").await?;
            input.flush().await?;
            drop(input);
            self.child.lock().await.wait().await.map(|_| ())
        };
        if !matches!(
            tokio::time::timeout(Duration::from_millis(250), graceful).await,
            Ok(Ok(()))
        ) {
            let mut child = self.child.lock().await;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
    async fn stop(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let mut child = self.child.lock().await;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
    async fn rpc(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> Result<Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(engine("worker"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut bytes = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))
            .map_err(|_| engine("invalid"))?;
        bytes.push(b'\n');
        let exchange = async {
            let mut input = self.input.lock().await;
            input
                .write_all(&bytes)
                .await
                .map_err(|_| engine("worker"))?;
            input.flush().await.map_err(|_| engine("worker"))?;
            drop(input);
            let mut output = self.output.lock().await;
            let mut response = String::new();
            let length = (&mut *output)
                .take(8_000_000)
                .read_line(&mut response)
                .await
                .map_err(|_| engine("worker"))?;
            if length == 0 || !response.ends_with('\n') {
                return Err(engine("worker"));
            }
            let value: Value = serde_json::from_str(&response).map_err(|_| engine("worker"))?;
            if value["id"] != id {
                return Err(engine("worker"));
            }
            if value["ok"] == true {
                Ok(value["result"].clone())
            } else {
                let code = match value["error"]["code"].as_str().unwrap_or("") {
                    "invalid" => "invalid",
                    "busy" => "busy",
                    "session" => "session",
                    "limit" => "limit",
                    "config" => "config",
                    _ => "inference",
                };
                Err(engine(code))
            }
        };
        let result = tokio::select! {_=cancel.cancelled()=>Err(engine("cancelled")),result=tokio::time::timeout(timeout,exchange)=>result.unwrap_or_else(|_|Err(engine("timeout")))};
        if result
            .as_ref()
            .is_err_and(|e| ["worker", "timeout", "cancelled"].contains(&e.code))
        {
            self.stop().await;
        }
        result
    }
}
struct Job {
    token: CancellationToken,
    done: AtomicBool,
    notify: Notify,
    handle: Mutex<Option<JoinHandle<()>>>,
    kind: &'static str,
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Session {
    worker: Arc<Worker>,
    last_active: Instant,
    samples: usize,
}
pub struct SpeechService {
    pub downloads: Arc<SpeechDownloads>,
    pub terms: SpeechTerms,
    resource_dir: PathBuf,
    native_dir: PathBuf,
    hotwords_dir: PathBuf,
    executable: PathBuf,
    asr: AsyncMutex<Option<Arc<Worker>>>,
    tts: AsyncMutex<Option<Arc<Worker>>>,
    sessions: Mutex<HashMap<String, Session>>,
    leases: Mutex<HashMap<String, CancellationToken>>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    last_activity: Mutex<Instant>,
    closed: AtomicBool,
    timer_cancel: CancellationToken,
    timer: Mutex<Option<JoinHandle<()>>>,
}
impl SpeechService {
    pub fn new(agent_dir: &Path, resource_dir: &Path, native_dir: &Path) -> Result<Arc<Self>> {
        Self::with_executable(
            agent_dir,
            resource_dir,
            native_dir,
            std::env::current_exe().map_err(|_| engine("worker"))?,
        )
    }
    pub fn with_executable(
        agent_dir: &Path,
        resource_dir: &Path,
        native_dir: &Path,
        executable: PathBuf,
    ) -> Result<Arc<Self>> {
        let absolute = |path: &Path| {
            if path.is_absolute() {
                Ok(path.to_owned())
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .map_err(|_| engine("config"))
            }
        };
        let agent_dir = absolute(agent_dir)?;
        let service = Arc::new(Self {
            downloads: SpeechDownloads::new(&agent_dir, Catalog::shared()?)?,
            terms: SpeechTerms::new(&agent_dir),
            resource_dir: absolute(resource_dir)?,
            native_dir: absolute(native_dir)?,
            hotwords_dir: agent_dir.join("speech"),
            executable,
            asr: AsyncMutex::new(None),
            tts: AsyncMutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            last_activity: Mutex::new(Instant::now()),
            closed: AtomicBool::new(false),
            timer_cancel: CancellationToken::new(),
            timer: Mutex::new(None),
        });
        let weak = Arc::downgrade(&service);
        let token = service.timer_cancel.clone();
        let handle = tokio::spawn(async move {
            Self::sweep(weak, token).await;
        });
        *service.timer.lock().map_err(|_| engine("worker"))? = Some(handle);
        Ok(service)
    }
    fn touch(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }
    fn queue(&self, kind: &str) -> &AsyncMutex<Option<Arc<Worker>>> {
        if kind == "asr" {
            &self.asr
        } else {
            &self.tts
        }
    }
    async fn lock_queue<'a>(
        &'a self,
        kind: &str,
        token: &CancellationToken,
    ) -> Result<tokio::sync::MutexGuard<'a, Option<Arc<Worker>>>> {
        tokio::select! {_=token.cancelled()=>Err(engine("cancelled")),state=self.queue(kind).lock()=>Ok(state)}
    }
    fn check(&self, token: &CancellationToken) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(engine("disposed"))
        } else if token.is_cancelled() {
            Err(engine("cancelled"))
        } else {
            Ok(())
        }
    }
    async fn ensure(
        &self,
        kind: &str,
        worker: &mut Option<Arc<Worker>>,
        cancel: &CancellationToken,
    ) -> Result<Arc<Worker>> {
        self.check(cancel)?;
        if let Some(existing) = worker {
            if !existing.closed.load(Ordering::Acquire) {
                return Ok(existing.clone());
            }
            existing.stop().await;
        }
        let model = self.downloads.catalog.default_model(kind)?.clone();
        let directory = self
            .downloads
            .model_directory(&model.id)
            .await
            .map_err(|_| engine("missing"))?;
        self.check(cancel)?;
        let mut command = Command::new(&self.executable);
        command
            .arg("--pisper-speech-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "SYSTEMDRIVE",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "LANG",
            "LC_ALL",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        // Even a future worker entry-point regression must stay in this service's own agent directory.
        if let Some(agent_dir) = self.hotwords_dir.parent() {
            command
                .env("PISPER_AGENT_DIR", agent_dir)
                .env("PI_CODING_AGENT_DIR", agent_dir);
        }
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| engine("worker"))?;
        let input = child.stdin.take().ok_or_else(|| engine("worker"))?;
        let output = child.stdout.take().ok_or_else(|| engine("worker"))?;
        let created = Arc::new(Worker {
            child: AsyncMutex::new(child),
            input: AsyncMutex::new(input),
            output: AsyncMutex::new(BufReader::new(output)),
            closed: AtomicBool::new(false),
        });
        *worker = Some(created.clone());
        let mut model_value: Value =
            serde_json::from_str(super::catalog::CATALOG).map_err(|_| engine("config"))?;
        let models = model_value["models"]
            .as_array_mut()
            .ok_or_else(|| engine("config"))?;
        let model_value = models
            .iter()
            .find(|v| v["id"] == model.id)
            .cloned()
            .ok_or_else(|| engine("config"))?;
        if let Err(error)=created.rpc("init",json!({"kind":kind,"model":model_value,"modelDir":directory,"resourceDir":self.resource_dir,"hotwordsDir":self.hotwords_dir,"nativeLibraryDir":self.native_dir}),cancel,Duration::from_secs(60)).await{created.stop().await;*worker=None;return Err(error);}
        self.check(cancel)?;
        Ok(created)
    }
    async fn run(
        self: &Arc<Self>,
        key: String,
        kind: &'static str,
        token: CancellationToken,
        operation: impl FnOnce(Arc<Self>, CancellationToken) -> BoxFuture<'static, Result<Value>>
            + Send
            + 'static,
    ) -> Result<Value> {
        let _cancel = CancelOnDrop(token.clone());
        let (receiver, job) = {
            let mut jobs = self.jobs.lock().map_err(|_| engine("worker"))?;
            self.check(&token)?;
            jobs.retain(|_, job| !job.done.load(Ordering::Acquire));
            if jobs.contains_key(&key)
                || (kind == "tts" && jobs.values().filter(|j| j.kind == "tts").count() >= 16)
            {
                return Err(engine("busy"));
            }
            let job = Arc::new(Job {
                token: token.clone(),
                done: AtomicBool::new(false),
                notify: Notify::new(),
                handle: Mutex::new(None),
                kind,
            });
            let (sender, receiver) = oneshot::channel();
            let service = self.clone();
            let task = job.clone();
            let handle = tokio::spawn(async move {
                let result = operation(service.clone(), token).await;
                service.touch();
                let _ = sender.send(result);
                task.done.store(true, Ordering::Release);
                task.notify.notify_waiters();
            });
            *job.handle.lock().map_err(|_| engine("worker"))? = Some(handle);
            jobs.insert(key, job.clone());
            (receiver, job)
        };
        self.touch();
        let result = receiver.await.map_err(|_| engine("worker"))?;
        Self::wait(&job).await;
        result
    }
    async fn wait(job: &Arc<Job>) {
        loop {
            let notified = job.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if job.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
        let handle = job.handle.lock().ok().and_then(|mut h| h.take());
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
    pub async fn prepare(self: &Arc<Self>, input: &Value, token: CancellationToken) -> Result<()> {
        let id = request_id(input)?;
        let kinds = input["kinds"].as_array().ok_or_else(|| engine("invalid"))?;
        if kinds.is_empty()
            || kinds.len() > 2
            || kinds
                .iter()
                .any(|v| !v.as_str().is_some_and(|v| ["asr", "tts"].contains(&v)))
            || (kinds.len() == 2 && kinds[0] == kinds[1])
        {
            return Err(engine("invalid"));
        }
        let hotwords = input
            .get("hotwords")
            .map(|v| v.as_str().ok_or_else(|| engine("invalid")))
            .transpose()?
            .unwrap_or("")
            .to_owned();
        if hotwords.len() > 20 * 1024
            || (!hotwords.is_empty()
                && (hotwords.split('\n').count() > 128
                    || hotwords
                        .split('\n')
                        .any(|v| v.trim().is_empty() || v.encode_utf16().count() > 128)))
            || regex::Regex::new(r"[:#@/\p{Cc}\p{Z}]")
                .unwrap()
                .is_match(&hotwords.replace(['\n', ' '], ""))
        {
            return Err(engine("invalid"));
        }
        if kinds.iter().any(|v| v == "tts") {
            self.voice(input)?;
        }
        {
            let mut leases = self.leases.lock().map_err(|_| engine("worker"))?;
            self.check(&token)?;
            leases.retain(|_, v| !v.is_cancelled());
            if leases.contains_key(&id) || leases.len() >= 16 {
                return Err(engine("busy"));
            }
            leases.insert(id.clone(), token.clone());
        }
        let prepare_asr = kinds.iter().any(|v| v == "asr");
        let prepare_tts = kinds.iter().any(|v| v == "tts");
        let terms = if hotwords.is_empty() {
            Vec::new()
        } else {
            hotwords.split('\n').map(str::to_owned).collect()
        };
        let service = self.clone();
        let asr_token = token.child_token();
        let tts_token = token.child_token();
        let asr = async move {
            if !prepare_asr {
                return Ok(json!({}));
            }
            service
                .run(
                    uuid::Uuid::new_v4().to_string(),
                    "asr",
                    asr_token,
                    move |service, cancel| {
                        Box::pin(async move {
                            let mut state = service.lock_queue("asr", &cancel).await?;
                            let worker = service.ensure("asr", &mut state, &cancel).await?;
                            worker
                                .rpc(
                                    "warmup",
                                    json!({"terms":terms}),
                                    &cancel,
                                    Duration::from_secs(120),
                                )
                                .await
                        })
                    },
                )
                .await
        };
        let service = self.clone();
        let tts = async move {
            if !prepare_tts {
                return Ok(json!({}));
            }
            service
                .run(
                    uuid::Uuid::new_v4().to_string(),
                    "warmup",
                    tts_token,
                    move |service, cancel| {
                        Box::pin(async move {
                            let mut state = service.lock_queue("tts", &cancel).await?;
                            service.ensure("tts", &mut state, &cancel).await?;
                            Ok(json!({"ready":true}))
                        })
                    },
                )
                .await
        };
        let (a, b) = tokio::join!(asr, tts);
        let result = a.and(b).map(|_| ());
        if result.is_err() {
            token.cancel();
            self.release(&id);
        }
        result
    }
    pub fn release(&self, id: &str) {
        if let Ok(mut leases) = self.leases.lock() {
            if let Some(token) = leases.remove(id) {
                token.cancel();
            }
        }
        self.touch();
    }
    fn voice(&self, input: &Value) -> Result<()> {
        let default = self.downloads.catalog.defaults["voice"]
            .as_str()
            .ok_or_else(|| engine("config"))?;
        let voice = input
            .get("voiceId")
            .map(|v| v.as_str().ok_or_else(|| engine("invalid")))
            .transpose()?
            .unwrap_or(default);
        let model = self.downloads.catalog.default_model("tts")?;
        if !model
            .voices
            .iter()
            .any(|v| v["id"] == voice && v["sid"] == 0)
        {
            return Err(engine("invalid"));
        }
        Ok(())
    }
    pub async fn synthesize(
        self: &Arc<Self>,
        input: &Value,
        token: CancellationToken,
    ) -> Result<Vec<u8>> {
        let id = request_id(input)?;
        self.voice(input)?;
        let text = input["text"]
            .as_str()
            .ok_or_else(|| engine("invalid"))?
            .to_owned();
        let model = self.downloads.catalog.default_model("tts")?;
        native::validate_text(
            &text,
            model.config["maxTextCodePoints"].as_u64().unwrap_or(400) as usize,
        )?;
        let value = self
            .run(id, "tts", token, move |service, cancel| {
                Box::pin(async move {
                    let mut state = service.lock_queue("tts", &cancel).await?;
                    let worker = service.ensure("tts", &mut state, &cancel).await?;
                    let result = worker
                        .rpc(
                            "synthesize",
                            json!({"text":text}),
                            &cancel,
                            Duration::from_secs(120),
                        )
                        .await;
                    if result.is_err() {
                        worker.stop().await;
                        *state = None;
                    }
                    result
                })
            })
            .await?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value["wav"].as_str().ok_or_else(|| engine("inference"))?)
            .map_err(|_| engine("inference"))?;
        validate_wav(&bytes)?;
        Ok(bytes)
    }
    pub async fn cancel_speech(&self, input: &Value) -> Result<Value> {
        let id = request_id(input)?;
        let job = self
            .jobs
            .lock()
            .map_err(|_| engine("worker"))?
            .get(&id)
            .filter(|j| j.kind == "tts" && !j.done.load(Ordering::Acquire))
            .cloned();
        if let Some(job) = job {
            job.token.cancel();
            Self::wait(&job).await;
            Ok(json!({"cancelled":true}))
        } else {
            Ok(json!({"cancelled":false}))
        }
    }
    pub async fn transcribe(
        self: &Arc<Self>,
        pcm: &[u8],
        terms: Vec<String>,
        token: CancellationToken,
    ) -> Result<Value> {
        native::decode_pcm(pcm)?;
        let pcm = base64::engine::general_purpose::STANDARD.encode(pcm);
        self.run(
            uuid::Uuid::new_v4().to_string(),
            "asr",
            token,
            move |service, cancel| {
                Box::pin(async move {
                    let mut state = service.lock_queue("asr", &cancel).await?;
                    let worker = service.ensure("asr", &mut state, &cancel).await?;
                    worker
                        .rpc(
                            "transcribe",
                            json!({"pcm":pcm,"terms":terms}),
                            &cancel,
                            Duration::from_secs(120),
                        )
                        .await
                })
            },
        )
        .await
    }
    pub async fn start_session(self: &Arc<Self>, terms: Vec<String>) -> Result<Value> {
        self.run(
            uuid::Uuid::new_v4().to_string(),
            "asr",
            CancellationToken::new(),
            move |service, cancel| {
                Box::pin(async move {
                    let mut state = service.lock_queue("asr", &cancel).await?;
                    let worker = service.ensure("asr", &mut state, &cancel).await?;
                    {
                        let mut sessions = service.sessions.lock().map_err(|_| engine("worker"))?;
                        sessions.retain(|_, s| !s.worker.closed.load(Ordering::Acquire));
                        if sessions.len() >= 4 {
                            return Err(engine("busy"));
                        }
                    }
                    let value = worker
                        .rpc(
                            "startSession",
                            json!({"terms":terms}),
                            &cancel,
                            Duration::from_secs(120),
                        )
                        .await?;
                    let id = value["id"]
                        .as_str()
                        .ok_or_else(|| engine("inference"))?
                        .to_owned();
                    service
                        .sessions
                        .lock()
                        .map_err(|_| engine("worker"))?
                        .insert(
                            id,
                            Session {
                                worker,
                                last_active: Instant::now(),
                                samples: 0,
                            },
                        );
                    Ok(value)
                })
            },
        )
        .await
    }
    pub async fn chunk(self: &Arc<Self>, id: &str, pcm: &[u8]) -> Result<Value> {
        let samples = native::decode_pcm(pcm)?.len();
        let pcm = base64::engine::general_purpose::STANDARD.encode(pcm);
        let id = id.to_owned();
        self.run(
            uuid::Uuid::new_v4().to_string(),
            "asr",
            CancellationToken::new(),
            move |service, cancel| {
                Box::pin(async move {
                    let _queue = service.lock_queue("asr", &cancel).await?;
                    let (worker, too_long) = {
                        let mut sessions = service.sessions.lock().map_err(|_| engine("worker"))?;
                        let session = sessions
                            .get_mut(&id)
                            .filter(|s| !s.worker.closed.load(Ordering::Acquire))
                            .ok_or_else(|| engine("session"))?;
                        session.samples += samples;
                        session.last_active = Instant::now();
                        (session.worker.clone(), session.samples > 16000 * 600)
                    };
                    if too_long {
                        let _ = worker
                            .rpc(
                                "cancelSession",
                                json!({"id":id}),
                                &cancel,
                                Duration::from_secs(120),
                            )
                            .await;
                        service
                            .sessions
                            .lock()
                            .map_err(|_| engine("worker"))?
                            .remove(&id);
                        return Err(engine("limit"));
                    }
                    let result = worker
                        .rpc(
                            "acceptChunk",
                            json!({"id":id,"pcm":pcm}),
                            &cancel,
                            Duration::from_secs(120),
                        )
                        .await;
                    if result.is_err() {
                        service
                            .sessions
                            .lock()
                            .map_err(|_| engine("worker"))?
                            .remove(&id);
                        let _ = worker
                            .rpc(
                                "cancelSession",
                                json!({"id":id}),
                                &cancel,
                                Duration::from_secs(120),
                            )
                            .await;
                    }
                    result
                })
            },
        )
        .await
    }
    pub async fn finish_session(self: &Arc<Self>, id: &str) -> Result<Value> {
        let id = id.to_owned();
        self.run(
            uuid::Uuid::new_v4().to_string(),
            "asr",
            CancellationToken::new(),
            move |service, cancel| {
                Box::pin(async move {
                    let _queue = service.lock_queue("asr", &cancel).await?;
                    let session = service
                        .sessions
                        .lock()
                        .map_err(|_| engine("worker"))?
                        .remove(&id)
                        .ok_or_else(|| engine("session"))?;
                    session
                        .worker
                        .rpc(
                            "finishSession",
                            json!({"id":id}),
                            &cancel,
                            Duration::from_secs(120),
                        )
                        .await
                })
            },
        )
        .await
    }
    pub async fn cancel_session(self: &Arc<Self>, id: &str) -> Result<Value> {
        let id = id.to_owned();
        self.run(
            uuid::Uuid::new_v4().to_string(),
            "asr",
            CancellationToken::new(),
            move |service, cancel| {
                Box::pin(async move {
                    let _queue = service.lock_queue("asr", &cancel).await?;
                    let session = service
                        .sessions
                        .lock()
                        .map_err(|_| engine("worker"))?
                        .remove(&id);
                    if let Some(session) = session {
                        let _ = session
                            .worker
                            .rpc(
                                "cancelSession",
                                json!({"id":id}),
                                &cancel,
                                Duration::from_secs(120),
                            )
                            .await;
                    }
                    Ok(json!({"ok":true}))
                })
            },
        )
        .await
    }
    async fn sweep(service: Weak<Self>, token: CancellationToken) {
        loop {
            tokio::select! {_=token.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            let Some(service) = service.upgrade() else {
                break;
            };
            let expired = service
                .sessions
                .lock()
                .map(|sessions| {
                    sessions
                        .iter()
                        .filter(|(_, s)| s.last_active.elapsed() > Duration::from_secs(600))
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for id in expired {
                let _ = service.cancel_session(&id).await;
            }
            let idle = service
                .last_activity
                .lock()
                .map(|last| last.elapsed() > Duration::from_secs(30))
                .unwrap_or(false)
                && service
                    .jobs
                    .lock()
                    .map(|jobs| jobs.values().all(|j| j.done.load(Ordering::Acquire)))
                    .unwrap_or(false)
                && service
                    .sessions
                    .lock()
                    .map(|s| s.is_empty())
                    .unwrap_or(false)
                && service
                    .leases
                    .lock()
                    .map(|leases| leases.values().all(CancellationToken::is_cancelled))
                    .unwrap_or(false);
            if idle {
                for kind in ["asr", "tts"] {
                    let mut state = service.queue(kind).lock().await;
                    if let Some(worker) = state.take() {
                        worker.unload().await;
                    }
                }
            }
        }
    }
    pub async fn shutdown(self: &Arc<Self>) {
        let jobs = {
            let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            self.closed.store(true, Ordering::Release);
            jobs.values().cloned().collect::<Vec<_>>()
        };
        self.timer_cancel.cancel();
        if let Ok(mut leases) = self.leases.lock() {
            for token in leases.values() {
                token.cancel();
            }
            leases.clear();
        }
        for job in &jobs {
            job.token.cancel();
        }
        for kind in ["asr", "tts"] {
            let mut state = self.queue(kind).lock().await;
            if let Some(worker) = state.take() {
                worker.stop().await;
            }
        }
        for job in jobs {
            Self::wait(&job).await;
        }
        let timer = self.timer.lock().ok().and_then(|mut h| h.take());
        if let Some(timer) = timer {
            let _ = timer.await;
        }
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.downloads.shutdown().await;
    }

    #[cfg(test)]
    pub(super) async fn fixture_worker_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for queue in [&self.asr, &self.tts] {
            if let Some(worker) = queue.lock().await.as_ref() {
                if let Some(id) = worker.child.lock().await.id() {
                    ids.push(id);
                }
            }
        }
        ids
    }

    #[cfg(test)]
    pub(super) fn fixture_lease_count(&self) -> usize {
        self.leases.lock().unwrap().len()
    }
}
fn request_id(input: &Value) -> Result<String> {
    let id = input["requestId"]
        .as_str()
        .ok_or_else(|| engine("invalid"))?;
    if !regex::Regex::new(
        r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
    )
    .unwrap()
    .is_match(id)
    {
        return Err(engine("invalid"));
    }
    Ok(id.to_owned())
}
fn validate_wav(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 46
        || bytes.len() % 2 != 0
        || &bytes[..4] != b"RIFF"
        || &bytes[8..16] != b"WAVEfmt "
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len() - 8
        || u32::from_le_bytes(bytes[16..20].try_into().unwrap()) != 16
        || u16::from_le_bytes(bytes[20..22].try_into().unwrap()) != 1
        || u16::from_le_bytes(bytes[22..24].try_into().unwrap()) != 1
        || u16::from_le_bytes(bytes[32..34].try_into().unwrap()) != 2
        || u16::from_le_bytes(bytes[34..36].try_into().unwrap()) != 16
        || &bytes[36..40] != b"data"
        || u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize != bytes.len() - 44
    {
        return Err(engine("limit"));
    }
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    if !(8000..=48000).contains(&rate)
        || u32::from_le_bytes(bytes[28..32].try_into().unwrap()) != rate * 2
        || bytes.len() > 44 + rate as usize * 45 * 2
    {
        return Err(engine("limit"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn queued_synthesis_cancel_does_not_wait_for_or_cancel_another_task_and_shutdown_joins() {
        let root =
            std::env::temp_dir().join(format!("pisper-speech-jobs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let service = SpeechService::new(&root, &root, &root).unwrap();
        let first_id = uuid::Uuid::new_v4().to_string();
        let first_service = service.clone();
        let began = Arc::new(Notify::new());
        let signal = began.clone();
        let first = tokio::spawn(async move {
            first_service
                .run(
                    first_id,
                    "tts",
                    CancellationToken::new(),
                    move |service, cancel| {
                        Box::pin(async move {
                            let _queue = service.lock_queue("tts", &cancel).await?;
                            signal.notify_one();
                            cancel.cancelled().await;
                            Err(engine("cancelled"))
                        })
                    },
                )
                .await
        });
        began.notified().await;
        let second_id = uuid::Uuid::new_v4().to_string();
        let id = second_id.clone();
        let second_service = service.clone();
        let second = tokio::spawn(async move {
            second_service
                .run(
                    id,
                    "tts",
                    CancellationToken::new(),
                    move |service, cancel| {
                        Box::pin(async move {
                            let _queue = service.lock_queue("tts", &cancel).await?;
                            Ok(json!({"unexpected":true}))
                        })
                    },
                )
                .await
        });
        while !service.jobs.lock().unwrap().contains_key(&second_id) {
            tokio::task::yield_now().await;
        }
        let cancelled = tokio::time::timeout(
            Duration::from_secs(1),
            service.cancel_speech(&json!({"requestId":second_id})),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(cancelled, json!({"cancelled":true}));
        assert_eq!(second.await.unwrap().unwrap_err().code, "cancelled");
        assert!(!first.is_finished());
        tokio::time::timeout(Duration::from_secs(1), service.shutdown())
            .await
            .unwrap();
        assert_eq!(first.await.unwrap().unwrap_err().code, "cancelled");
        assert!(service.timer.lock().unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
