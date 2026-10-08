//! 真实下载、进度、换源、取消与原子发布由一个服务拥有；观察 HTTP 请求不拥有下载。
use crate::native_workflow::{
    bundle::EngineBundleStore,
    engine_cache::{EngineCache, EngineDefinition, EngineFile},
    media::{self, BundleFiles},
    Result, WorkflowError,
};
use axum::{
    body::Body,
    extract::Path,
    http::{header, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use futures::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

struct Download {
    cancellation: CancellationToken,
    done: Notify,
    finished: AtomicBool,
}
impl Download {
    async fn wait(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.finished.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}
struct Inner {
    states: HashMap<String, Value>,
    jobs: HashMap<String, Arc<Download>>,
}
pub(crate) struct EngineService {
    cache: Arc<EngineCache>,
    client: reqwest::Client,
    inner: Mutex<Inner>,
    mutation: Mutex<()>,
    closed: AtomicBool,
    timeout: Duration,
    source_timeout: Duration,
}
fn failure(code: &str, status: StatusCode) -> WorkflowError {
    let mut error = WorkflowError::coded(code, "本地图片引擎资源不可用，请重试。");
    error.status = status;
    error
}
impl EngineService {
    pub(crate) fn open(cache: Arc<EngineCache>) -> Result<Arc<Self>> {
        Self::with_timeouts(cache, Duration::from_secs(180), Duration::from_secs(45))
    }
    fn with_timeouts(
        cache: Arc<EngineCache>,
        timeout: Duration,
        source_timeout: Duration,
    ) -> Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|_| {
                failure(
                    "sprite_engine_download_failed",
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;
        Ok(Arc::new(Self {
            cache,
            client,
            inner: Mutex::new(Inner {
                states: HashMap::new(),
                jobs: HashMap::new(),
            }),
            mutation: Mutex::new(()),
            closed: AtomicBool::new(false),
            timeout,
            source_timeout,
        }))
    }
    fn check(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(failure(
                "sprite_engine_closed",
                StatusCode::SERVICE_UNAVAILABLE,
            ))
        } else {
            Ok(())
        }
    }
    fn definition(&self, id: &str) -> Result<EngineDefinition> {
        self.cache
            .definitions()
            .iter()
            .find(|definition| definition.id == id)
            .cloned()
            .ok_or_else(|| failure("sprite_engine_not_found", StatusCode::NOT_FOUND))
    }
    pub(crate) fn cache(&self) -> Arc<EngineCache> {
        self.cache.clone()
    }
    pub(crate) async fn catalog(&self) -> Result<Value> {
        self.check()?;
        let mut catalog = self.cache.catalog().await?;
        let inner = self.inner.lock().await;
        for state in catalog["engines"].as_array_mut().into_iter().flatten() {
            if let Some(current) = state["id"].as_str().and_then(|id| inner.states.get(id)) {
                if current["status"] != "ready" || state["status"] == "ready" {
                    *state = current.clone();
                }
            }
        }
        Ok(catalog)
    }
    pub(crate) async fn download(self: &Arc<Self>, id: &str) -> Result<Value> {
        let definition = self.definition(id)?;
        let _mutation = self.mutation.lock().await;
        self.check()?;
        if self.cache.execution_directory(id).await.is_ok() {
            return self.catalog().await;
        }
        let job = {
            let mut inner = self.inner.lock().await;
            if inner.jobs.contains_key(id) {
                drop(inner);
                return self.catalog().await;
            }
            let total = definition
                .files
                .iter()
                .map(|file| file.bytes)
                .sum::<usize>();
            let job = Arc::new(Download {
                cancellation: CancellationToken::new(),
                done: Notify::new(),
                finished: AtomicBool::new(false),
            });
            inner.states.insert(id.into(),json!({"id":id,"name":definition.name,"version":definition.version,"bytes":total,"status":"downloading","received":0,"total":total,"error":"","file":""}));
            inner.jobs.insert(id.into(), job.clone());
            job
        };
        let service = self.clone();
        tokio::spawn(async move {
            let result =
                tokio::time::timeout(service.timeout, service.download_files(&definition, &job))
                    .await;
            let mut inner = service.inner.lock().await;
            if let Some(state) = inner.states.get_mut(&definition.id) {
                match result {
                    Ok(Ok(())) => {
                        state["status"] = json!("ready");
                        state["received"] = state["total"].clone();
                        state["error"] = json!("");
                        state["file"] = json!("");
                    }
                    Ok(Err(error)) => {
                        state["status"] = json!(if job.cancellation.is_cancelled() {
                            "missing"
                        } else {
                            "failed"
                        });
                        state["received"] = json!(0);
                        state["error"] = json!(if job.cancellation.is_cancelled() {
                            ""
                        } else {
                            &error.code
                        });
                    }
                    Err(_) => {
                        job.cancellation.cancel();
                        state["status"] = json!("failed");
                        state["received"] = json!(0);
                        state["error"] = json!("sprite_engine_timeout");
                    }
                }
            }
            inner.jobs.remove(&definition.id);
            drop(inner);
            job.finished.store(true, Ordering::Release);
            job.done.notify_waiters();
        });
        self.catalog().await
    }
    async fn download_files(&self, definition: &EngineDefinition, job: &Download) -> Result<()> {
        let mut files = BundleFiles::new();
        let mut before = 0;
        for file in &definition.files {
            {
                let mut inner = self.inner.lock().await;
                if let Some(state) = inner.states.get_mut(&definition.id) {
                    state["file"] = json!(file.name);
                }
            }
            let mut last = failure("sprite_engine_download_failed", StatusCode::BAD_REQUEST);
            let mut success = None;
            for url in &file.urls {
                if job.cancellation.is_cancelled() {
                    return Err(failure("sprite_engine_cancelled", StatusCode::BAD_REQUEST));
                }
                self.progress(&definition.id, before).await;
                match tokio::time::timeout(
                    self.source_timeout,
                    self.download_file(&definition.id, file, url, before, &job.cancellation),
                )
                .await
                {
                    Ok(Ok(bytes)) => {
                        success = Some(bytes);
                        break;
                    }
                    Ok(Err(error)) => last = error,
                    Err(_) => last = failure("sprite_engine_timeout", StatusCode::BAD_REQUEST),
                }
            }
            let bytes = success.ok_or(last)?;
            before += bytes.len();
            files.insert(format!("engines/{}/{}", definition.id, file.name), bytes);
        }
        if job.cancellation.is_cancelled() {
            return Err(failure("sprite_engine_cancelled", StatusCode::BAD_REQUEST));
        }
        // 发布过程不受观察请求的取消影响；complete 必须在 fsync/安装清单提交后出现。
        self.cache.install_files(files).await
    }
    async fn progress(&self, id: &str, received: usize) {
        if let Some(state) = self.inner.lock().await.states.get_mut(id) {
            state["received"] = json!(received);
        }
    }
    async fn download_file(
        &self,
        id: &str,
        file: &EngineFile,
        url: &str,
        before: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        let url = reqwest::Url::parse(url)
            .map_err(|_| failure("sprite_engine_download_failed", StatusCode::BAD_REQUEST))?;
        if !["http", "https"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failure(
                "sprite_engine_download_failed",
                StatusCode::BAD_REQUEST,
            ));
        }
        let response = tokio::select! {_ = cancellation.cancelled()=>return Err(failure("sprite_engine_cancelled",StatusCode::BAD_REQUEST)),result=self.client.get(url).send()=>result.map_err(|_|failure("sprite_engine_download_failed",StatusCode::BAD_REQUEST))?};
        if !response.status().is_success() {
            return Err(failure(
                "sprite_engine_download_failed",
                StatusCode::BAD_REQUEST,
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > file.bytes as u64)
        {
            return Err(failure("sprite_engine_integrity", StatusCode::BAD_REQUEST));
        }
        let mut buffer = Vec::with_capacity(file.bytes);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = tokio::select! {_=cancellation.cancelled()=>return Err(failure("sprite_engine_cancelled",StatusCode::BAD_REQUEST)),chunk=stream.next()=>chunk}
        {
            let chunk = chunk
                .map_err(|_| failure("sprite_engine_download_failed", StatusCode::BAD_REQUEST))?;
            if buffer.len().saturating_add(chunk.len()) > file.bytes {
                return Err(failure("sprite_engine_integrity", StatusCode::BAD_REQUEST));
            }
            buffer.extend_from_slice(&chunk);
            self.progress(id, before + buffer.len()).await;
        }
        if buffer.len() != file.bytes || media::digest(&buffer) != file.sha256 {
            return Err(failure("sprite_engine_integrity", StatusCode::BAD_REQUEST));
        }
        Ok(buffer)
    }
    async fn cancel_job(&self, id: &str) {
        let job = self.inner.lock().await.jobs.get(id).cloned();
        if let Some(job) = job {
            job.cancellation.cancel();
            job.wait().await;
        }
    }
    pub(crate) async fn cancel(&self, id: &str) -> Result<Value> {
        self.definition(id)?;
        let _mutation = self.mutation.lock().await;
        self.check()?;
        self.cancel_job(id).await;
        self.catalog().await
    }
    pub(crate) async fn remove(&self, id: &str) -> Result<Value> {
        self.definition(id)?;
        let _mutation = self.mutation.lock().await;
        self.check()?;
        self.cancel_job(id).await;
        self.cache.remove(id).await?;
        self.inner.lock().await.states.remove(id);
        self.catalog().await
    }
    pub(crate) async fn file(&self, id: &str, name: &str) -> Result<(Vec<u8>, String)> {
        self.check()?;
        self.cache.file(id, name).await
    }
    pub(crate) async fn dispose(&self) {
        let _mutation = self.mutation.lock().await;
        self.closed.store(true, Ordering::Release);
        let jobs = self
            .inner
            .lock()
            .await
            .jobs
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for job in &jobs {
            job.cancellation.cancel();
        }
        for job in jobs {
            job.wait().await;
        }
        self.cache.dispose().await;
    }
}
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(service: Arc<EngineService>) -> Router<S> {
    let catalog = service.clone();
    let download = service.clone();
    let cancel = service.clone();
    let remove = service.clone();
    Router::new()
        .route(
            "/api/sprite-engines",
            get(move || {
                let service = catalog.clone();
                async move { service.catalog().await.map(Json) }
            }),
        )
        .route(
            "/api/sprite-engines/{id}/download",
            post(move |Path(id): Path<String>| {
                let service = download.clone();
                async move {
                    service
                        .download(&id)
                        .await
                        .map(|value| (StatusCode::ACCEPTED, Json(value)))
                }
            }),
        )
        .route(
            "/api/sprite-engines/{id}/cancel",
            post(move |Path(id): Path<String>| {
                let service = cancel.clone();
                async move { service.cancel(&id).await.map(Json) }
            }),
        )
        .route(
            "/api/sprite-engines/{id}",
            delete(move |Path(id): Path<String>| {
                let service = remove.clone();
                async move { service.remove(&id).await.map(Json) }
            }),
        )
        .route(
            "/api/sprite-engines/{id}/files/{name}",
            get(move |Path((id, name)): Path<(String, String)>| {
                let service = service.clone();
                async move {
                    let (bytes, mime) = service.file(&id, &name).await?;
                    Response::builder()
                        .header(header::CONTENT_TYPE, mime)
                        .header(header::CONTENT_LENGTH, bytes.len())
                        .header(header::CACHE_CONTROL, "private, max-age=60")
                        .header("x-content-type-options", "nosniff")
                        .body(Body::from(bytes))
                        .map_err(WorkflowError::io)
                }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::test_support::TempDirectory;
    async fn fixture() -> (
        String,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let app = Router::new()
            .route("/good", get(|| async { Body::from("verified-model") }))
            .route("/bad", get(|| async { Body::from("wrong") }))
            .route(
                "/pending",
                get(|| async { std::future::pending::<String>().await }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        (root, tx, job)
    }
    fn definition(urls: Vec<String>) -> EngineDefinition {
        EngineDefinition {
            id: "background".into(),
            name: "fixture".into(),
            version: "fixed".into(),
            files: vec![EngineFile {
                name: "u2netp.onnx".into(),
                bytes: 14,
                sha256: media::digest(b"verified-model"),
                mime_type: "application/octet-stream".into(),
                urls,
            }],
        }
    }
    async fn settled(service: &EngineService) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let state = service.catalog().await.unwrap();
                if state["engines"][0]["status"] != "downloading" {
                    return state;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn real_download_checks_digest_falls_back_restarts_and_remove_clears_content() {
        let directory = TempDirectory::new();
        let (root, shutdown, job) = fixture().await;
        let definitions = vec![definition(vec![
            format!("{root}/bad"),
            format!("{root}/good"),
        ])];
        let cache = EngineCache::open(&directory.path, definitions.clone()).unwrap();
        let service = EngineService::open(cache).unwrap();
        assert_eq!(
            service.download("background").await.unwrap()["engines"][0]["status"],
            "downloading"
        );
        assert_eq!(settled(&service).await["engines"][0]["status"], "ready");
        assert_eq!(
            service.file("background", "u2netp.onnx").await.unwrap().0,
            b"verified-model"
        );
        service.dispose().await;
        let restarted =
            EngineService::open(EngineCache::open(&directory.path, definitions).unwrap()).unwrap();
        assert_eq!(
            restarted.catalog().await.unwrap()["engines"][0]["status"],
            "ready"
        );
        assert_eq!(
            restarted.remove("background").await.unwrap()["engines"][0]["status"],
            "missing"
        );
        assert!(restarted.file("background", "u2netp.onnx").await.is_err());
        restarted.dispose().await;
        shutdown.send(()).unwrap();
        job.await.unwrap();
    }
    #[tokio::test]
    async fn cancellation_and_shutdown_join_real_pending_requests() {
        let directory = TempDirectory::new();
        let (root, shutdown, job) = fixture().await;
        let definitions = vec![definition(vec![format!("{root}/pending")])];
        let service =
            EngineService::open(EngineCache::open(&directory.path, definitions).unwrap()).unwrap();
        service.download("background").await.unwrap();
        assert_eq!(
            service.cancel("background").await.unwrap()["engines"][0]["status"],
            "missing"
        );
        assert!(service.inner.lock().await.jobs.is_empty());
        service.download("background").await.unwrap();
        service.dispose().await;
        assert!(service.inner.lock().await.jobs.is_empty());
        assert_eq!(
            service.download("background").await.unwrap_err().code,
            "sprite_engine_closed"
        );
        shutdown.send(()).unwrap();
        job.abort();
        let _ = job.await;
    }
    #[tokio::test]
    async fn integrity_failure_never_publishes_and_timeout_has_stable_state() {
        let directory = TempDirectory::new();
        let (root, shutdown, job) = fixture().await;
        let service = EngineService::with_timeouts(
            EngineCache::open(
                &directory.path,
                vec![definition(vec![format!("{root}/bad")])],
            )
            .unwrap(),
            Duration::from_secs(1),
            Duration::from_millis(50),
        )
        .unwrap();
        service.download("background").await.unwrap();
        assert_eq!(
            settled(&service).await["engines"][0]["error"],
            "sprite_engine_integrity"
        );
        assert!(service.file("background", "u2netp.onnx").await.is_err());
        service.dispose().await;
        shutdown.send(()).unwrap();
        job.await.unwrap();
    }
}
