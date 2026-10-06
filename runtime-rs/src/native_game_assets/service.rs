use super::protocol::{self, empty_output, error};
use crate::{
    native_workflow::{
        image_nodes::{ImageNodeService, ImageOperationRequest},
        image_protocol,
        media::{self, MediaService},
        Result, WorkflowError,
    },
    workflow_engine::RunCancellation,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
};
use tokio::sync::{Mutex, Notify};

struct Task {
    project_id: String,
    cancellation: Arc<RunCancellation>,
    finished: AtomicBool,
    done: Notify,
    result: StdMutex<Option<Result<()>>>,
}
impl Task {
    fn new(project_id: String) -> Arc<Self> {
        Arc::new(Self {
            project_id,
            cancellation: Arc::new(RunCancellation::default()),
            finished: AtomicBool::new(false),
            done: Notify::new(),
            result: StdMutex::new(None),
        })
    }
    async fn wait(&self) -> Result<()> {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.finished.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .map_err(WorkflowError::io)?
                    .clone()
                    .unwrap_or(Ok(()));
            }
            notified.await;
        }
    }
}
struct Inner {
    state: Value,
    running: HashMap<String, Arc<Task>>,
    storage_failed: bool,
}
pub(crate) struct GameAssetsService {
    path: PathBuf,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    inner: Mutex<Inner>,
    closed: AtomicBool,
}
fn storage(code: &str) -> WorkflowError {
    error(code, StatusCode::INTERNAL_SERVER_ERROR)
}
fn active(cancellation: &RunCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(error("game_assets_cancelled", StatusCode::BAD_REQUEST))
    } else {
        Ok(())
    }
}
fn image_request(operation: &str, images: Vec<Value>, settings: Value) -> ImageOperationRequest {
    ImageOperationRequest {
        operation: operation.into(),
        source: None,
        images,
        settings,
        prompt: String::new(),
        model: Value::Null,
        resume_output: None,
        edits: None,
    }
}
impl GameAssetsService {
    pub(crate) fn open(
        data_dir: &Path,
        media: Arc<MediaService>,
        images: Arc<ImageNodeService>,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir).map_err(|_| storage("game_assets_storage_invalid"))?;
        let data_dir = data_dir
            .canonicalize()
            .map_err(|_| storage("game_assets_storage_invalid"))?;
        media::directory(&data_dir).map_err(|_| storage("game_assets_storage_invalid"))?;
        let path = data_dir.join("game-assets.json");
        let mut state = json!({"projects":[],"jobs":[]});
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.len() > 64 * 1024 * 1024
                {
                    return Err(storage("game_assets_storage_invalid"));
                }
                let stored: Value = serde_json::from_slice(
                    &media::read_bounded(&path, 64 * 1024 * 1024)
                        .map_err(|_| storage("game_assets_storage_invalid"))?,
                )
                .map_err(|_| storage("game_assets_storage_invalid"))?;
                protocol::record(&stored, &["version", "projects", "jobs"])
                    .map_err(|_| storage("game_assets_storage_invalid"))?;
                if stored["version"] != 1 {
                    return Err(storage("game_assets_storage_invalid"));
                }
                state = protocol::catalog(
                    &json!({"projects":stored["projects"],"jobs":stored["jobs"]}),
                )
                .map_err(|_| storage("game_assets_storage_invalid"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(storage("game_assets_storage_invalid")),
        }
        let mut interrupted = false;
        for job in state["jobs"].as_array_mut().unwrap() {
            if job["status"] == "running" {
                job["status"] = json!("interrupted");
                job["finishedAt"] = json!(crate::native_workflow::now());
                job["error"] = json!("game_assets_interrupted");
                interrupted = true;
            }
        }
        if interrupted {
            Self::write_path(&path, &state)?;
        }
        Ok(Arc::new(Self {
            path,
            media,
            images,
            inner: Mutex::new(Inner {
                state,
                running: HashMap::new(),
                storage_failed: false,
            }),
            closed: AtomicBool::new(false),
        }))
    }
    fn check(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(error("game_assets_closed", StatusCode::SERVICE_UNAVAILABLE))
        } else {
            Ok(())
        }
    }
    fn write_path(path: &Path, state: &Value) -> Result<()> {
        let write = || -> Result<()> {
            media::directory(
                path.parent()
                    .ok_or_else(|| storage("game_assets_storage_failed"))?,
            )?;
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                    return Err(storage("game_assets_storage_failed"))
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(storage("game_assets_storage_failed"))
                }
                _ => {}
            }
            let bytes = serde_json::to_vec_pretty(
                &json!({"version":1,"projects":state["projects"],"jobs":state["jobs"]}),
            )
            .map_err(WorkflowError::io)?;
            if bytes.len() > 64 * 1024 * 1024 {
                return Err(storage("game_assets_storage_failed"));
            }
            let temporary = path.with_extension(format!("{}.tmp", crate::native_workflow::id()?));
            let result = media::write_private(&temporary, &bytes)
                .and_then(|()| std::fs::rename(&temporary, path).map_err(WorkflowError::io));
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result
        };
        write().map_err(|_| storage("game_assets_storage_failed"))
    }
    fn commit(&self, inner: &mut Inner, state: Value) -> Result<()> {
        let parsed = protocol::catalog(&state)?;
        Self::write_path(&self.path, &parsed)?;
        inner.state = parsed;
        Ok(())
    }
    pub(crate) async fn catalog(&self) -> Result<Value> {
        Ok(self.inner.lock().await.state.clone())
    }
    #[cfg(test)]
    pub(super) async fn reserved(&self, project_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .running
            .values()
            .any(|task| task.project_id == project_id)
    }
    async fn validate_reference(&self, reference: &Value) -> Result<()> {
        if reference.is_null() {
            return Ok(());
        }
        let valid = async {
            let stored = self
                .media
                .read(reference["id"].as_str().unwrap_or(""))
                .await?;
            if stored.metadata["media"] != *reference
                || !reference["mimeType"]
                    .as_str()
                    .is_some_and(|mime| mime.starts_with("image/"))
            {
                return Err(protocol::invalid());
            }
            Ok(())
        }
        .await;
        valid.map_err(|_| error("game_assets_media_invalid", StatusCode::BAD_REQUEST))
    }
    async fn validate_output(&self, value: &Value) -> Result<Value> {
        let output = image_protocol::output(value)?;
        let mut seen = HashMap::new();
        let mut references = output["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| frame["media"].clone())
            .collect::<Vec<_>>();
        if let Some(atlas) = output.get("atlas") {
            references.push(atlas["media"].clone());
        }
        for reference in references {
            let id = reference["id"].as_str().unwrap_or("").to_string();
            if let Some(previous) = seen.get(&id) {
                if *previous != reference {
                    return Err(error("game_assets_media_invalid", StatusCode::BAD_REQUEST));
                }
            } else {
                self.validate_reference(&reference).await?;
                seen.insert(id, reference);
            }
        }
        Ok(output)
    }
    fn available(inner: &Inner, project_id: &str) -> Result<()> {
        if inner
            .running
            .values()
            .any(|task| task.project_id == project_id)
        {
            Err(error("game_assets_busy", StatusCode::CONFLICT))
        } else {
            Ok(())
        }
    }
    pub(crate) async fn save(&self, value: Value) -> Result<Value> {
        let input = protocol::project_input(&value)?;
        let mut inner = self.inner.lock().await;
        self.check()?;
        let projects = inner.state["projects"].as_array().unwrap();
        let previous = input
            .get("id")
            .and_then(|id| projects.iter().find(|project| &project["id"] == id))
            .cloned();
        if input.get("id").is_some() && previous.is_none() {
            return Err(error("game_assets_not_found", StatusCode::NOT_FOUND));
        }
        if previous.is_none() && projects.len() >= 100 {
            return Err(error("game_assets_limit", StatusCode::CONFLICT));
        }
        if let Some(previous) = &previous {
            Self::available(&inner, previous["id"].as_str().unwrap())?;
        }
        self.validate_reference(&input["reference"]).await?;
        self.validate_reference(&input["originalReference"]).await?;
        let now = crate::native_workflow::now();
        let mut project = input;
        project["id"] = previous
            .as_ref()
            .map(|value| value["id"].clone())
            .unwrap_or(json!(crate::native_workflow::id()?));
        project["createdAt"] = previous
            .as_ref()
            .map(|value| value["createdAt"].clone())
            .unwrap_or(json!(now));
        project["updatedAt"] = json!(now);
        let project = protocol::project(&project)?;
        let mut projects = inner.state["projects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["id"] != project["id"])
            .cloned()
            .collect::<Vec<_>>();
        projects.insert(0, project.clone());
        let state = json!({"projects":projects,"jobs":inner.state["jobs"]});
        self.commit(&mut inner, state)?;
        Ok(project)
    }
    pub(crate) async fn remove(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        self.check()?;
        if !inner.state["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|project| project["id"] == id)
        {
            return Err(error("game_assets_not_found", StatusCode::NOT_FOUND));
        }
        Self::available(&inner, id)?;
        let state = json!({"projects":inner.state["projects"].as_array().unwrap().iter().filter(|project|project["id"]!=id).cloned().collect::<Vec<_>>(),
            "jobs":inner.state["jobs"].as_array().unwrap().iter().filter(|job|job["projectId"]!=id).cloned().collect::<Vec<_>>()});
        self.commit(&mut inner, state)
    }
    pub(crate) async fn get_job(&self, id: &str) -> Result<Option<Value>> {
        Ok(self.inner.lock().await.state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["id"] == id)
            .cloned())
    }
    pub(crate) async fn run(self: &Arc<Self>, project_id: &str) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        self.check()?;
        let project = inner.state["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|project| project["id"] == project_id)
            .cloned()
            .ok_or_else(|| error("game_assets_not_found", StatusCode::NOT_FOUND))?;
        Self::available(&inner, project_id)?;
        if inner.running.len() >= 2 {
            return Err(error("game_assets_busy", StatusCode::CONFLICT));
        }
        if project["reference"].is_null() {
            return Err(error(
                "game_assets_source_required",
                StatusCode::BAD_REQUEST,
            ));
        }
        let total = project["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["enabled"] == true)
            .count();
        if total == 0 {
            return Err(error(
                "game_assets_actions_required",
                StatusCode::BAD_REQUEST,
            ));
        }
        self.validate_reference(&project["reference"]).await?;
        let job = protocol::job(
            &json!({"id":crate::native_workflow::id()?,"projectId":project_id,"status":"running",
            "startedAt":crate::native_workflow::now(),"finishedAt":null,"completed":0,"total":total,"error":null,
            "output":empty_output(),"originalOutput":empty_output(),"revision":0}),
        )?;
        let mut jobs = inner.state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["projectId"] != project_id)
            .cloned()
            .collect::<Vec<_>>();
        jobs.insert(0, job.clone());
        jobs.truncate(100);
        let state = json!({"projects":inner.state["projects"],"jobs":jobs});
        self.commit(&mut inner, state)?;
        let task = Task::new(project_id.into());
        let id = job["id"].as_str().unwrap().to_string();
        inner.running.insert(id.clone(), task.clone());
        let (service, original) = (self.clone(), job.clone());
        tokio::spawn(async move {
            let result = service
                .execute_project(project, original, task.cancellation.clone())
                .await;
            service.finish(&id, &task, result).await;
        });
        Ok(job)
    }
    async fn finish(&self, id: &str, task: &Task, result: Result<()>) {
        let mut inner = self.inner.lock().await;
        if result
            .as_ref()
            .is_err_and(|error| error.code == "game_assets_storage_failed")
        {
            inner.storage_failed = true;
        }
        if let Ok(mut stored) = task.result.lock() {
            *stored = Some(result);
        }
        task.finished.store(true, Ordering::Release);
        inner.running.remove(id);
        task.done.notify_waiters();
    }
    async fn persist_job(&self, job: Value) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let mut state = inner.state.clone();
        for entry in state["jobs"].as_array_mut().unwrap() {
            if entry["id"] == job["id"] {
                *entry = job.clone();
            }
        }
        self.commit(&mut inner, state)
    }
    async fn operation(
        &self,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Value> {
        self.images
            .operate(request, cancellation)
            .await
            .map(|result| result.output)
    }
    async fn compute_project(
        &self,
        project: &Value,
        job: &mut Value,
        pending: &mut Value,
        cancellation: Arc<RunCancellation>,
    ) -> Result<()> {
        active(&cancellation)?;
        let mut request = image_request("input", vec![], json!({}));
        request.source = Some(project["reference"].clone());
        let input = self.operation(request, cancellation.clone()).await?;
        let source = self.validate_output(&input).await?;
        for action in project["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["enabled"] == true)
        {
            active(&cancellation)?;
            let settings = json!({"action":action["name"],"frameCount":project["frameCount"],"directions":project["directions"],"method":"color","maxFrameSize":256});
            let prompt = [
                project["prompt"].as_str().unwrap(),
                action["prompt"].as_str().unwrap(),
            ]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
            for operation in ["generate", "background", "frames", "transform"] {
                active(&cancellation)?;
                let frames = if operation == "generate" {
                    source["frames"].as_array().unwrap()
                } else {
                    pending["frames"].as_array().unwrap()
                };
                let mut request = image_request(operation, frames.clone(), settings.clone());
                request.prompt = prompt.clone();
                request.model = project["model"].clone();
                let result = self.operation(request, cancellation.clone()).await?;
                *pending = self.validate_output(&result).await?;
            }
            active(&cancellation)?;
            let mut frames = job["output"]["frames"].as_array().unwrap().clone();
            frames.extend(pending["frames"].as_array().unwrap().clone());
            let output = json!({"type":"workflow-images","version":1,"frames":frames});
            job["completed"] = json!(job["completed"].as_u64().unwrap() + 1);
            job["output"] = output.clone();
            job["originalOutput"] = output;
            *job = protocol::job(job)?;
            *pending = empty_output();
            self.persist_job(job.clone()).await?;
        }
        active(&cancellation)?;
        let result = self
            .operation(
                image_request(
                    "export",
                    job["output"]["frames"].as_array().unwrap().clone(),
                    json!({"filename":project["name"],"maxFrameSize":256}),
                ),
                cancellation.clone(),
            )
            .await?;
        let output = self.validate_output(&result).await?;
        active(&cancellation)?;
        job["status"] = json!("completed");
        job["finishedAt"] = json!(crate::native_workflow::now());
        job["output"] = output.clone();
        job["originalOutput"] = output;
        *job = protocol::job(job)?;
        self.persist_job(job.clone()).await
    }
    async fn execute_project(
        &self,
        project: Value,
        mut job: Value,
        cancellation: Arc<RunCancellation>,
    ) -> Result<()> {
        let mut pending = empty_output();
        if let Err(failure) = self
            .compute_project(&project, &mut job, &mut pending, cancellation.clone())
            .await
        {
            if let Some(partial) = &failure.partial_output {
                if let Ok(output) = self.validate_output(partial).await {
                    pending = output;
                }
            }
            if !pending["frames"].as_array().unwrap().is_empty() {
                let mut frames = job["output"]["frames"].as_array().unwrap().clone();
                frames.extend(pending["frames"].as_array().unwrap().clone());
                if let Ok(output) = image_protocol::output(
                    &json!({"type":"workflow-images","version":1,"frames":frames}),
                ) {
                    job["output"] = output;
                }
            }
            job["status"] = json!(if cancellation.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            });
            job["finishedAt"] = json!(crate::native_workflow::now());
            job["error"] = json!(if cancellation.is_cancelled() {
                "game_assets_cancelled"
            } else {
                protocol::safe_code(&failure)
            });
            job["originalOutput"] = job["output"].clone();
            let job = protocol::job(&job)?;
            if self.persist_job(job.clone()).await.is_err() {
                let mut inner = self.inner.lock().await;
                for entry in inner.state["jobs"].as_array_mut().unwrap() {
                    if entry["id"] == job["id"] {
                        *entry = job.clone();
                        entry["error"] = json!("game_assets_storage_failed");
                    }
                }
                return Err(storage("game_assets_storage_failed"));
            }
        }
        Ok(())
    }
    pub(crate) async fn edit(self: &Arc<Self>, job_id: &str, value: Value) -> Result<Value> {
        let edits = protocol::edits(&value)?;
        let (task, job, source) =
            {
                let mut inner = self.inner.lock().await;
                self.check()?;
                let job = inner.state["jobs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|job| job["id"] == job_id)
                    .cloned()
                    .ok_or_else(|| error("game_assets_not_found", StatusCode::NOT_FOUND))?;
                Self::available(&inner, job["projectId"].as_str().unwrap())?;
                if job["status"] == "running" || inner.running.len() >= 2 {
                    return Err(error("game_assets_busy", StatusCode::CONFLICT));
                }
                let originals = job["originalOutput"]["frames"].as_array().unwrap();
                if originals.is_empty()
                    || edits["frames"].as_array().unwrap().iter().any(|frame| {
                        frame["sourceIndex"].as_u64().unwrap() as usize >= originals.len()
                    })
                {
                    return Err(protocol::invalid());
                }
                let source = self.validate_output(&job["originalOutput"]).await?;
                let task = Task::new(job["projectId"].as_str().unwrap().into());
                inner.running.insert(job_id.into(), task.clone());
                (task, job, source)
            };
        let (service, owned, id) = (self.clone(), task.clone(), job_id.to_string());
        tokio::spawn(async move {
            let result = service
                .execute_edit(job, source, edits, owned.cancellation.clone())
                .await;
            service.finish(&id, &owned, result).await;
        });
        task.wait().await?;
        self.get_job(job_id)
            .await?
            .ok_or_else(|| error("game_assets_not_found", StatusCode::NOT_FOUND))
    }
    async fn execute_edit(
        &self,
        mut job: Value,
        source: Value,
        edits: Value,
        cancellation: Arc<RunCancellation>,
    ) -> Result<()> {
        let result = async {
            active(&cancellation)?;
            let mut request = image_request(
                "edit",
                source["frames"].as_array().unwrap().clone(),
                json!({"maxFrameSize":256}),
            );
            request.edits = Some(edits.clone());
            let edited = self.operation(request, cancellation.clone()).await?;
            let frames = self.validate_output(&edited).await?;
            active(&cancellation)?;
            let name = self.inner.lock().await.state["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|project| project["id"] == job["projectId"])
                .map(|project| project["name"].clone())
                .unwrap_or(json!("animation"));
            let exported = self
                .operation(
                    image_request(
                        "export",
                        frames["frames"].as_array().unwrap().clone(),
                        json!({"filename":name,"maxFrameSize":256}),
                    ),
                    cancellation.clone(),
                )
                .await?;
            let output = self.validate_output(&exported).await?;
            active(&cancellation)?;
            job["output"] = output;
            job["edits"] = edits;
            job["revision"] = json!(job["revision"].as_u64().unwrap() + 1);
            self.persist_job(protocol::job(&job)?).await
        }
        .await;
        result.map_err(|failure| {
            error(
                if cancellation.is_cancelled() {
                    "game_assets_cancelled"
                } else {
                    protocol::safe_code(&failure)
                },
                StatusCode::BAD_REQUEST,
            )
        })
    }
    pub(crate) async fn stop(&self, job_id: &str) -> Result<Value> {
        let task = {
            let inner = self.inner.lock().await;
            if !inner.state["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|job| job["id"] == job_id)
            {
                return Err(error("game_assets_not_found", StatusCode::NOT_FOUND));
            }
            inner.running.get(job_id).cloned()
        };
        if let Some(task) = task {
            task.cancellation.cancel();
            if let Err(failure) = task.wait().await {
                if failure.code == "game_assets_storage_failed" {
                    return Err(failure);
                }
            }
        }
        self.get_job(job_id)
            .await?
            .ok_or_else(|| error("game_assets_not_found", StatusCode::NOT_FOUND))
    }
    pub(crate) async fn dispose(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        let tasks = self
            .inner
            .lock()
            .await
            .running
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for task in &tasks {
            task.cancellation.cancel();
        }
        for task in tasks {
            let _ = task.wait().await;
        }
        if self.inner.lock().await.storage_failed {
            Err(storage("game_assets_storage_failed"))
        } else {
            Ok(())
        }
    }
}
