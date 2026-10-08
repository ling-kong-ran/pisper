use super::{active, error, export, import, schema, Result};
use crate::{
    native_workflow::{
        image_nodes::{ImageGenerator, ImageNodeService, ImageOperationRequest},
        image_processing::ImageProcessor,
        media::{MediaService, StoredMedia},
    },
    workflow_engine::RunCancellation,
};
use axum::http::StatusCode;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{oneshot, Notify};
use uuid::Uuid;

pub(crate) type AgentEnabledPort = Arc<dyn Fn() -> BoxFuture<'static, Result<bool>> + Send + Sync>;
#[derive(Clone, Debug)]
pub(crate) struct ToolContext {
    pub(crate) cwd: PathBuf,
    pub(crate) session_id: String,
}
#[derive(Clone, Debug)]
pub(crate) struct GeneratedFile {
    pub(crate) path: PathBuf,
    pub(crate) mime_type: String,
}
pub(crate) type GeneratedFilePort =
    Arc<dyn Fn(GeneratedFile, ToolContext) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Narrow ports also let lifecycle tests delay real native media commits. They do
/// not permit URLs or workspace bypasses; those boundaries remain in this domain.
pub(crate) trait AgentMedia: Send + Sync {
    fn upload(&self, name: String, mime: String, bytes: Vec<u8>) -> BoxFuture<'_, Result<Value>>;
    fn read(&self, id: String) -> BoxFuture<'_, Result<StoredMedia>>;
}
impl AgentMedia for MediaService {
    fn upload(&self, name: String, mime: String, bytes: Vec<u8>) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move { self.upload(&name, &mime, &bytes).await })
    }
    fn read(&self, id: String) -> BoxFuture<'_, Result<StoredMedia>> {
        Box::pin(async move { self.read(&id).await })
    }
}
pub(crate) trait AgentOperations: Send + Sync {
    fn execute(
        &self,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<Value>>;
}
struct NativeOperations(Arc<ImageNodeService>);
impl AgentOperations for NativeOperations {
    fn execute(
        &self,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let result = self.0.operate(request, cancellation).await?;
            Ok(json!({"output":result.output,"summary":result.summary}))
        })
    }
}
struct Owned {
    operations: Arc<ImageNodeService>,
    media: Arc<MediaService>,
}
struct Job {
    cancellation: Arc<RunCancellation>,
    finished: AtomicBool,
    done: Notify,
}
struct JobLease {
    service: Arc<ImageAgentService>,
    id: Uuid,
    job: Arc<Job>,
}
impl Drop for JobLease {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.service.jobs.lock() {
            jobs.remove(&self.id);
        }
        self.job.finished.store(true, Ordering::Release);
        self.job.done.notify_waiters();
    }
}
impl Job {
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
pub(crate) struct ImageAgentService {
    pub(super) media: Arc<dyn AgentMedia>,
    operations: Arc<dyn AgentOperations>,
    enabled: AgentEnabledPort,
    owned: Option<Owned>,
    jobs: Mutex<HashMap<Uuid, Arc<Job>>>,
    closed: AtomicBool,
    closing: tokio::sync::Mutex<()>,
    disposed: AtomicBool,
}
impl ImageAgentService {
    pub(crate) fn open(
        data_dir: &Path,
        processor: Arc<ImageProcessor>,
        generator: Arc<dyn ImageGenerator>,
        enabled: AgentEnabledPort,
    ) -> Result<Arc<Self>> {
        let root = data_dir.join("image-tools-agent");
        let media = MediaService::open(&root)?;
        let operations = ImageNodeService::open(&root, media.clone(), processor, generator)?;
        Ok(Arc::new(Self {
            media: media.clone(),
            operations: Arc::new(NativeOperations(operations.clone())),
            enabled,
            owned: Some(Owned { operations, media }),
            jobs: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            closing: tokio::sync::Mutex::new(()),
            disposed: AtomicBool::new(false),
        }))
    }
    #[cfg(test)]
    pub(super) fn with_services(
        media: Arc<dyn AgentMedia>,
        operations: Arc<dyn AgentOperations>,
        enabled: AgentEnabledPort,
    ) -> Arc<Self> {
        Arc::new(Self {
            media,
            operations,
            enabled,
            owned: None,
            jobs: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            closing: tokio::sync::Mutex::new(()),
            disposed: AtomicBool::new(false),
        })
    }
    pub(super) async fn allowed(
        &self,
        cancellation: &RunCancellation,
        exporting: bool,
    ) -> Result<()> {
        active(cancellation, exporting)?;
        if !(self.enabled)().await? {
            return Err(error("image_tools_agent_disabled", StatusCode::FORBIDDEN));
        }
        active(cancellation, exporting)
    }
    async fn run<T, F, Fut>(
        self: &Arc<Self>,
        external: Arc<RunCancellation>,
        operation: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>, Arc<RunCancellation>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        let id = Uuid::new_v4();
        let job = Arc::new(Job {
            cancellation: Arc::new(external.child()),
            finished: AtomicBool::new(false),
            done: Notify::new(),
        });
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| error("image_tools_closed", StatusCode::SERVICE_UNAVAILABLE))?;
            if self.closed.load(Ordering::Acquire) {
                return Err(error("image_tools_closed", StatusCode::SERVICE_UNAVAILABLE));
            }
            jobs.insert(id, job.clone());
        }
        let (sender, mut receiver) = oneshot::channel();
        let (service, own_job) = (self.clone(), job.clone());
        tokio::spawn(async move {
            let _lease = JobLease {
                service: service.clone(),
                id,
                job: own_job.clone(),
            };
            // A separately owned task retains filesystem handles, partial exports
            // and media commits even when the caller drops its future.
            let result = operation(service.clone(), own_job.cancellation.clone()).await;
            let _ = sender.send(result);
        });
        let _drop_cancel = CancelOnDrop(job.cancellation.clone());
        tokio::select! {
            _=external.cancelled()=>{job.cancellation.cancel();receiver.await.map_err(|_|error("image_tools_closed",StatusCode::SERVICE_UNAVAILABLE))?},
            result=&mut receiver=>result.map_err(|_|error("image_tools_closed",StatusCode::SERVICE_UNAVAILABLE))?,
        }
    }
    pub(crate) async fn execute(
        self: &Arc<Self>,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Value> {
        self.run(cancellation, move |service, cancel| async move {
            service.allowed(&cancel, false).await?;
            service.operations.execute(request, cancel).await
        })
        .await
    }
    pub(crate) async fn import_image(
        self: &Arc<Self>,
        cwd: PathBuf,
        source: String,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Value> {
        self.run(cancellation, move |service, cancel| async move {
            import::import(&service, cwd, source, cancel).await
        })
        .await
    }
    pub(crate) async fn export_images(
        self: &Arc<Self>,
        cwd: PathBuf,
        output: Value,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Value> {
        self.run(cancellation, move |service, cancel| async move {
            export::export(&service, cwd, output, cancel)
                .await
                .map(|files| files_value(&files))
        })
        .await
    }
    pub(crate) async fn call(
        self: &Arc<Self>,
        context: ToolContext,
        arguments: Value,
        cancellation: Arc<RunCancellation>,
        generated: GeneratedFilePort,
    ) -> Result<Value> {
        // Match the release tool: reject all pure argument errors before import
        // can persist a resource, including malformed model IDs and edits.
        let arguments = schema::parse(&arguments)?;
        self.run(cancellation,move|service,cancel|async move {
            let source = match arguments.source_image {
                Some(path)=>Some(import::import(&service,context.cwd.clone(),path,cancel.clone()).await?),
                None=>arguments.source,
            };
            service.allowed(&cancel,false).await?;
            let exporting = arguments.operation=="export";
            let mut result=service.operations.execute(ImageOperationRequest {
                operation:arguments.operation,source,images:arguments.images,settings:arguments.settings,
                prompt:arguments.prompt,model:arguments.model,resume_output:None,edits:arguments.edits,
            },cancel.clone()).await?;
            if exporting {
                let files=export::export(&service,context.cwd.clone(),result["output"].clone(),cancel).await?;
                result["files"]=files_value(&files)["files"].clone();
                for file in files {
                    if let Err(error)=generated(file,context.clone()).await {
                        // Indexing must not discard a successfully written export.
                        tracing::warn!(code=%error.code,"image asset indexing failed after successful export");
                    }
                }
            }
            Ok(json!({"content":[{"type":"text","text":serde_json::to_string(&result).map_err(|_|super::invalid())?}],"details":result}))
        }).await
    }
    pub(crate) async fn close(&self) {
        let jobs = {
            let Ok(jobs) = self.jobs.lock() else {
                self.closed.store(true, Ordering::Release);
                return;
            };
            self.closed.store(true, Ordering::Release);
            let owned = jobs.values().cloned().collect::<Vec<_>>();
            for job in &owned {
                job.cancellation.cancel();
            }
            owned
        };
        let _closing = self.closing.lock().await;
        if self.disposed.load(Ordering::Acquire) {
            return;
        }
        for job in jobs {
            job.wait().await;
        }
        if let Some(owned) = &self.owned {
            owned.operations.dispose().await;
            owned.media.dispose().await;
        }
        self.disposed.store(true, Ordering::Release);
    }
}
pub(super) fn files_value(files: &[GeneratedFile]) -> Value {
    json!({"files":files.iter().map(|file|json!({"path":super::fs_boundary::display_path(&file.path),"mimeType":file.mime_type})).collect::<Vec<_>>()})
}
struct CancelOnDrop(Arc<RunCancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
