//! DAG 媒体适配和付费生成生命周期；已经保存的方向始终作为 partial output 返回。
use super::{
    image_processing::{ImageProcessor, ProcessingFrame, ProcessingRequest},
    image_protocol, inputs,
    media::{self, MediaService},
    Result, WorkflowError,
};
use crate::workflow_engine::{ImageNodeRequest, ImageNodeResult, RunCancellation};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{oneshot, Notify};
pub(crate) struct ImageGenerationRequest {
    pub(crate) cwd: PathBuf,
    pub(crate) model: Value,
    pub(crate) prompt: String,
    pub(crate) source_images: Vec<PathBuf>,
    pub(crate) output_name: String,
    pub(crate) aspect_ratio: String,
}
pub(crate) struct GeneratedImage {
    pub(crate) path: PathBuf,
    pub(crate) mime_type: String,
}
pub(crate) struct ImageOperationRequest {
    pub(crate) operation: String,
    pub(crate) source: Option<Value>,
    pub(crate) images: Vec<Value>,
    pub(crate) settings: Value,
    pub(crate) prompt: String,
    pub(crate) model: Value,
    pub(crate) resume_output: Option<Value>,
    pub(crate) edits: Option<Value>,
}
pub(crate) trait ImageGenerator: Send + Sync {
    fn generate(
        &self,
        request: ImageGenerationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<GeneratedImage>>;
}
struct Job {
    cancellation: Arc<RunCancellation>,
    finished: AtomicBool,
    done: Notify,
}
pub(crate) struct ImageNodeService {
    root: PathBuf,
    media: Arc<MediaService>,
    processor: Arc<ImageProcessor>,
    generator: Arc<dyn ImageGenerator>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    closed: AtomicBool,
}
fn failure(code: &str) -> WorkflowError {
    WorkflowError::coded(code, code)
}
fn active(cancellation: &RunCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(failure("workflow_image_cancelled"))
    } else {
        Ok(())
    }
}
fn dimensions(bytes: &[u8], mime: &str) -> Result<(u32, u32)> {
    media::validate_bytes(bytes, mime)?;
    let (width, height, actual) =
        media::raster_dimensions(bytes).ok_or_else(image_protocol::invalid)?;
    if actual != mime {
        return Err(image_protocol::invalid());
    }
    Ok((width, height))
}
fn extension(mime: &str) -> Result<&'static str> {
    match mime {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        _ => Err(image_protocol::invalid()),
    }
}
fn grid(count: u32) -> (u32, u32) {
    match count {
        0 | 1 => (1, 1),
        2 => (2, 1),
        3 => (3, 1),
        4 => (4, 1),
        5 | 6 => (3, 2),
        _ => (4, count.div_ceil(4)),
    }
}
fn clip(text: &str, maximum: usize) -> String {
    if text.encode_utf16().count() <= maximum {
        text.to_string()
    } else {
        let characters = text.encode_utf16().take(maximum - 1).collect::<Vec<_>>();
        format!("{}…", String::from_utf16_lossy(&characters))
    }
}
fn phase(action: &str, index: usize, count: usize) -> String {
    let phases: &[&str] = match action.to_lowercase().as_str() {
        "idle" | "待机" | "呼吸" => &["rest", "inhale rise", "full breath", "exhale lower"],
        "walk" | "走路" | "行走" => &[
            "left heel contact",
            "left support passing",
            "right heel contact",
            "right support passing",
        ],
        "run" | "跑步" | "奔跑" => &[
            "left foot contact",
            "airborne right lead",
            "right foot contact",
            "airborne left lead",
        ],
        "attack" | "攻击" => &[
            "ready",
            "anticipate",
            "wind up",
            "strike",
            "follow through",
            "recover",
            "settle",
            "ready",
        ],
        _ => &[],
    };
    phases
        .get(index * phases.len() / count)
        .map(|value| value.to_string())
        .unwrap_or_else(|| format!("motion phase {}/{}", index + 1, count))
}
fn sheet_prompt(settings: &Value, direction: &str, prompt: &str, background: &str) -> String {
    let count = settings["frameCount"].as_u64().unwrap_or(4) as usize;
    let (cols, rows) = grid(count as u32);
    let action = settings["action"].as_str().unwrap_or("idle");
    let direction = match direction {
        "S" => "FRONT (face and chest toward viewer)",
        "SW" => "FRONT-LEFT three-quarter",
        "W" => "LEFT profile",
        "NW" => "BACK-LEFT three-quarter",
        "N" => "BACK (back toward viewer)",
        "NE" => "BACK-RIGHT three-quarter",
        "E" => "RIGHT profile",
        _ => "FRONT-RIGHT three-quarter",
    };
    let mut parts=vec![format!("Same character and art style as reference. One {rows}×{cols} sprite sheet: {count}-frame continuous {action} cycle, L→R then T→B. Identical look each panel; smooth motion; last loops to first. Plain/transparent bg, no text."),"Char: Preserve reference art style, palette, identity, outfit, equipment and proportions. Do not convert art style.".into(),format!("Frames: {}",(0..count).map(|index|format!("{}:action/{}",index+1,phase(action,index,count))).collect::<Vec<_>>().join("; "))];
    let empty = cols * rows - count as u32;
    if empty > 0 {
        parts.push(format!("Blank last {empty} panel(s)."));
    }
    parts.push(clip(&format!("Every frame faces {direction}; rotate whole body, never just the head. Fixed orthographic camera, scale, ground baseline and lighting. Full body and equipment, 15% safe margin in each equal cell. No grid lines, labels, cast shadows or duplicate poses. Solid {background} background, including empty cells. {prompt}"),600));
    clip(&parts.join(" "), 1400)
}
struct JobDirectory(PathBuf);
impl Drop for JobDirectory {
    fn drop(&mut self) {
        if media::directory(&self.0).is_ok() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
impl ImageNodeService {
    pub(crate) fn open(
        data_dir: &Path,
        media: Arc<MediaService>,
        processor: Arc<ImageProcessor>,
        generator: Arc<dyn ImageGenerator>,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir).map_err(WorkflowError::io)?;
        let root = data_dir
            .canonicalize()
            .map_err(WorkflowError::io)?
            .join("image-operation-jobs");
        if !root.exists() {
            media::create_private_directory(&root)?;
        }
        media::directory(&root)?;
        Ok(Arc::new(Self {
            root,
            media,
            processor,
            generator,
            jobs: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }))
    }
    async fn load(
        &self,
        reference: &Value,
        cancellation: &RunCancellation,
    ) -> Result<(Value, Vec<u8>)> {
        active(cancellation)?;
        let reference = inputs::media(reference)?;
        let stored = self
            .media
            .read(reference["id"].as_str().unwrap_or(""))
            .await?;
        if stored.metadata["media"] != reference {
            return Err(failure("workflow_media_invalid"));
        }
        let (width, height) =
            dimensions(&stored.buffer, reference["mimeType"].as_str().unwrap_or(""))?;
        active(cancellation)?;
        Ok((
            json!({"media":reference,"width":width,"height":height}),
            stored.buffer,
        ))
    }
    async fn pixels(
        &self,
        frames: &[Value],
        cancellation: &RunCancellation,
    ) -> Result<Vec<ProcessingFrame>> {
        let mut total = 0_u64;
        let mut loaded = Vec::new();
        for frame in frames {
            total += frame["media"]["size"].as_u64().unwrap_or(0);
            if total > 128 * 1024 * 1024 {
                return Err(failure("workflow_image_too_large"));
            }
            let (actual, buffer) = self.load(&frame["media"], cancellation).await?;
            if actual["width"].as_f64() != frame["width"].as_f64()
                || actual["height"].as_f64() != frame["height"].as_f64()
            {
                return Err(image_protocol::invalid());
            }
            loaded.push(ProcessingFrame {
                wire: frame.clone(),
                buffer,
            });
        }
        Ok(loaded)
    }
    async fn save(
        &self,
        mut wire: Value,
        buffer: &[u8],
        mime: &str,
        name: &str,
        cancellation: &RunCancellation,
    ) -> Result<Value> {
        active(cancellation)?;
        let (width, height) = dimensions(buffer, mime)?;
        if wire["width"].as_f64() != Some(width as f64)
            || wire["height"].as_f64() != Some(height as f64)
        {
            return Err(image_protocol::invalid());
        }
        let media = self.media.upload(name, mime, buffer).await?;
        wire["media"] = media;
        let parsed =
            image_protocol::output(&json!({"type":"workflow-images","version":1,"frames":[wire]}))?;
        Ok(parsed["frames"][0].clone())
    }
    async fn resume(
        &self,
        previous: &Value,
        settings: &Value,
        cancellation: &RunCancellation,
    ) -> Result<Vec<Value>> {
        let output = image_protocol::output(previous)?;
        let mut frames = output["frames"]
            .as_array()
            .cloned()
            .ok_or_else(image_protocol::invalid)?;
        let (cols, rows) = grid(settings["frameCount"].as_u64().unwrap_or(4) as u32);
        let directions = settings["directions"]
            .as_array()
            .ok_or_else(image_protocol::invalid)?;
        let mut seen = HashSet::new();
        if output.get("atlas").is_some()
            || frames.iter().any(|frame| {
                !seen.insert(frame["direction"].as_str().unwrap_or(""))
                    || frame["action"] != settings["action"]
                    || !directions.contains(&frame["direction"])
                    || frame["columns"].as_f64() != Some(cols as f64)
                    || frame["rows"].as_f64() != Some(rows as f64)
                    || frame["frameCount"].as_f64() != settings["frameCount"].as_f64()
                    || frame["durationMs"].as_f64() != settings["durationMs"].as_f64()
            })
        {
            return Err(image_protocol::invalid());
        }
        self.pixels(&frames, cancellation).await?;
        frames.sort_by_key(|frame| {
            directions
                .iter()
                .position(|direction| direction == &frame["direction"])
        });
        Ok(frames)
    }
    fn read_generated(
        &self,
        generated: GeneratedImage,
        directory: &Path,
    ) -> Result<(Vec<u8>, String, u32, u32)> {
        let root = directory.join("generated");
        let visuals = root.join("visuals");
        let path = if generated.path.is_absolute() {
            generated.path
        } else {
            std::env::current_dir()
                .map_err(WorkflowError::io)?
                .join(generated.path)
        };
        if path.parent() != Some(visuals.as_path())
            || path.file_name().is_none()
            || path.canonicalize().map_err(|_| image_protocol::invalid())? != path
        {
            return Err(image_protocol::invalid());
        }
        media::directory(directory).map_err(|_| image_protocol::invalid())?;
        media::directory(&root).map_err(|_| image_protocol::invalid())?;
        media::directory(&visuals).map_err(|_| image_protocol::invalid())?;
        let buffer =
            media::read_bounded(&path, 8 * 1024 * 1024).map_err(|_| image_protocol::invalid())?;
        let (width, height) = dimensions(&buffer, &generated.mime_type)?;
        Ok((buffer, generated.mime_type, width, height))
    }
    async fn generate(
        &self,
        node: &Value,
        settings: &Value,
        sources: Vec<ProcessingFrame>,
        output: &mut Value,
        cancellation: Arc<RunCancellation>,
        resume: Option<&Value>,
    ) -> Result<()> {
        if sources.len() > 8 {
            return Err(image_protocol::invalid());
        }
        if let Some(previous) = resume {
            output["frames"] = json!(self.resume(previous, settings, &cancellation).await?);
        }
        let directions = settings["directions"]
            .as_array()
            .ok_or_else(image_protocol::invalid)?;
        let remaining = directions
            .iter()
            .filter(|direction| {
                !output["frames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|frame| &frame["direction"] == *direction)
            })
            .cloned()
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            return Ok(());
        }
        media::directory(&self.root)?;
        let directory = self.root.join(format!("job-{}", super::id()?));
        media::create_private_directory(&directory)?;
        let _directory = JobDirectory(directory.clone());
        let mut paths = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            let path = directory.join(format!(
                "reference-{index}.{}",
                extension(source.wire["media"]["mimeType"].as_str().unwrap_or(""))?
            ));
            media::write_private(&path, &source.buffer)?;
            paths.push(path);
        }
        let background = if let Some(color) = settings["colors"]
            .as_array()
            .and_then(|colors| colors.first())
            .and_then(Value::as_str)
        {
            color.to_string()
        } else {
            let source = sources.first().ok_or_else(image_protocol::invalid)?;
            self.processor
                .process(
                    ProcessingRequest {
                        operation: "palette".into(),
                        settings: settings.clone(),
                        edits: None,
                        frames: vec![ProcessingFrame {
                            wire: source.wire.clone(),
                            buffer: source.buffer.clone(),
                        }],
                    },
                    cancellation.clone(),
                )
                .await?
                .recommended_background
                .ok_or_else(image_protocol::invalid)?
        };
        let (cols, rows) = grid(settings["frameCount"].as_u64().unwrap_or(4) as u32);
        for direction in remaining {
            active(&cancellation)?;
            let direction = direction.as_str().ok_or_else(image_protocol::invalid)?;
            let request = ImageGenerationRequest {
                cwd: directory.clone(),
                model: node["model"].clone(),
                prompt: sheet_prompt(
                    settings,
                    direction,
                    node["prompt"].as_str().unwrap_or(""),
                    &background,
                ),
                source_images: paths.clone(),
                output_name: format!("action-{direction}"),
                aspect_ratio: if rows == 1 && cols > 1 { "16:9" } else { "1:1" }.into(),
            };
            let generated = tokio::select! {_=cancellation.cancelled()=>return Err(failure("workflow_image_cancelled")),result=self.generator.generate(request,cancellation.clone())=>result?};
            active(&cancellation)?;
            let (buffer, mime, width, height) = self.read_generated(generated, &directory)?;
            let wire = json!({"width":width,"height":height,"durationMs":settings["durationMs"],"action":settings["action"],"direction":direction,"columns":cols,"rows":rows,"frameCount":settings["frameCount"]});
            let frame = self
                .save(
                    wire,
                    &buffer,
                    &mime,
                    &format!("action-{direction}.{}", extension(&mime)?),
                    &cancellation,
                )
                .await?;
            let frames = output["frames"]
                .as_array_mut()
                .ok_or_else(image_protocol::invalid)?;
            frames.push(frame);
            frames.sort_by_key(|frame| {
                directions
                    .iter()
                    .position(|direction| direction == &frame["direction"])
            });
        }
        Ok(())
    }
    async fn run(
        &self,
        request: ImageNodeRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<ImageNodeResult> {
        let mut output = json!({"type":"workflow-images","version":1,"frames":[]});
        let generation = request.node["kind"] == "media-generate";
        let result=async{active(&cancellation)?;let kind=request.node["kind"].as_str().filter(|kind|image_protocol::KINDS.contains(kind)).ok_or_else(image_protocol::invalid)?;let settings=image_protocol::settings(request.node.get("image"))?;if request.resume_output.is_some()&&!generation{return Err(image_protocol::invalid());}if kind=="media-input"{let reference=&request.inputs[settings["inputName"].as_str().unwrap_or("reference")];let(mut wire,_)=self.load(reference,&cancellation).await?;wire["durationMs"]=settings["durationMs"].clone();wire["action"]=json!("");wire["direction"]=json!("");for key in["columns","rows","frameCount"]{wire[key]=json!(1);}output["frames"]=json!([wire]);}else{let mut frames=Vec::new();for predecessor in &request.predecessors{let parsed=image_protocol::output(&predecessor["output"])?;frames.extend(parsed["frames"].as_array().cloned().ok_or_else(image_protocol::invalid)?);}let parsed=image_protocol::output(&json!({"type":"workflow-images","version":1,"frames":frames}))?;let frames=parsed["frames"].as_array().cloned().ok_or_else(image_protocol::invalid)?;if frames.is_empty(){return Err(failure("workflow_image_source_required"));}let loaded=self.pixels(&frames,&cancellation).await?;match kind{"media-preview"=>output["frames"]=json!(frames),"media-generate"=>self.generate(&request.node,&settings,loaded,&mut output,cancellation.clone(),request.resume_output.as_ref()).await?,_=>{let operation=kind.trim_start_matches("media-");let processed=self.processor.process(ProcessingRequest{operation:operation.into(),frames:loaded,settings:settings.clone(),edits:None},cancellation.clone()).await?;active(&cancellation)?;if operation=="export"{output["frames"]=json!(frames);}else{for(index,frame)in processed.frames.into_iter().enumerate(){let frame=self.save(frame.wire,&frame.buffer,"image/png",&format!("frame-{}.png",index+1),&cancellation).await?;output["frames"].as_array_mut().ok_or_else(image_protocol::invalid)?.push(frame);}}if let Some(atlas)=processed.atlas{let(width,height)=dimensions(&atlas.buffer,"image/png")?;if atlas.wire["width"].as_f64()!=Some(width as f64)||atlas.wire["height"].as_f64()!=Some(height as f64){return Err(image_protocol::invalid());}let name=settings["filename"].as_str().filter(|name|!name.is_empty()).unwrap_or("animation");let media=self.media.upload(&format!("{name}.png"),"image/png",&atlas.buffer).await?;output["atlas"]=json!({"media":media,"width":width,"height":height,"frames":atlas.wire["frames"]});}}}}
        active(&cancellation)?;let output=image_protocol::output(&output)?;let count=output["frames"].as_array().map(Vec::len).unwrap_or(0);Ok(ImageNodeResult{output,summary:format!("{count} frames"),assets:vec![]})}.await;
        result.map_err(|error| {
            let safe = [
                "workflow_image_invalid",
                "workflow_image_invalid_edits",
                "workflow_image_source_required",
                "workflow_image_too_large",
                "workflow_image_cancelled",
                "workflow_image_closed",
                "workflow_image_generation_failed",
                "workflow_image_processing_failed",
                "workflow_image_timeout",
                "workflow_media_invalid",
                "workflow_media_missing",
                "workflow_media_too_large",
                "sprite_engine_missing",
                "sprite_engine_invalid",
            ];
            let code = if cancellation.is_cancelled() {
                "workflow_image_cancelled"
            } else if safe.contains(&error.code.as_str()) {
                &error.code
            } else if generation {
                "workflow_image_generation_failed"
            } else {
                "workflow_image_processing_failed"
            };
            let mut error = failure(code);
            if output["frames"]
                .as_array()
                .is_some_and(|frames| !frames.is_empty())
            {
                if let Ok(partial) = image_protocol::output(&output) {
                    error = error.with_partial_output(partial);
                }
            }
            error
        })
    }
    async fn tracked<T, F, Fut>(
        self: &Arc<Self>,
        external: Arc<RunCancellation>,
        operation: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>, Arc<RunCancellation>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        active(&external)?;
        let token = super::id()?;
        let job = Arc::new(Job {
            cancellation: Arc::new(external.child()),
            finished: AtomicBool::new(false),
            done: Notify::new(),
        });
        {
            let mut jobs = self.jobs.lock().map_err(WorkflowError::io)?;
            if self.closed.load(Ordering::Acquire) {
                return Err(failure("workflow_image_closed"));
            }
            jobs.insert(token.clone(), job.clone());
        }
        let (sender, mut receiver) = oneshot::channel();
        let (service, owned) = (self.clone(), job.clone());
        tokio::spawn(async move {
            let result = operation(service.clone(), owned.cancellation.clone()).await;
            if let Ok(mut jobs) = service.jobs.lock() {
                jobs.remove(&token);
            }
            owned.finished.store(true, Ordering::Release);
            owned.done.notify_waiters();
            let _ = sender.send(result);
        });
        tokio::select! {_=external.cancelled()=>{job.cancellation.cancel();receiver.await.map_err(|_|failure("workflow_image_processing_failed"))?},result=&mut receiver=>result.map_err(|_|failure("workflow_image_processing_failed"))?}
    }
    pub(crate) async fn execute(
        self: &Arc<Self>,
        request: ImageNodeRequest,
    ) -> Result<ImageNodeResult> {
        self.tracked(
            request.cancellation.clone(),
            move |service, cancellation| async move { service.run(request, cancellation).await },
        )
        .await
    }
    pub(crate) async fn operate(
        self: &Arc<Self>,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<ImageNodeResult> {
        self.tracked(cancellation, move |service, cancellation| async move {
            service.run_operation(request, cancellation).await
        })
        .await
    }
    async fn run_operation(
        &self,
        request: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<ImageNodeResult> {
        let mut output = json!({"type":"workflow-images","version":1,"frames":[]});
        let generation = request.operation == "generate";
        let result=async {
            active(&cancellation)?;
            if !["input","background","inpaint","generate","frames","transform","preview","export","edit"].contains(&request.operation.as_str())
                || (request.resume_output.is_some() && !generation) { return Err(image_protocol::invalid()); }
            let settings=image_protocol::settings(Some(&request.settings))?;
            if request.operation=="input" {
                let (mut wire,_)=self.load(request.source.as_ref().ok_or_else(image_protocol::invalid)?,&cancellation).await?;
                wire["durationMs"]=settings["durationMs"].clone();
                wire["action"]=json!("");wire["direction"]=json!("");
                for key in ["columns","rows","frameCount"]{wire[key]=json!(1);}
                output["frames"]=json!([wire]);
            } else {
                let parsed=image_protocol::output(&json!({"type":"workflow-images","version":1,"frames":request.images}))?;
                let frames=parsed["frames"].as_array().cloned().ok_or_else(image_protocol::invalid)?;
                if frames.is_empty(){return Err(failure("workflow_image_source_required"));}
                let loaded=self.pixels(&frames,&cancellation).await?;
                if request.operation=="preview" {output["frames"]=json!(frames);}
                else if generation {
                    self.generate(&json!({"prompt":request.prompt,"model":request.model}),&settings,loaded,&mut output,
                        cancellation.clone(),request.resume_output.as_ref()).await?;
                } else {
                    let result=self.processor.process(ProcessingRequest{operation:request.operation.clone(),frames:loaded,
                        settings:settings.clone(),edits:request.edits},cancellation.clone()).await?;
                    active(&cancellation)?;
                    if request.operation=="export" {output["frames"]=json!(frames);}
                    else { for (index,frame) in result.frames.into_iter().enumerate(){
                        output["frames"].as_array_mut().ok_or_else(image_protocol::invalid)?.push(
                            self.save(frame.wire,&frame.buffer,"image/png",&format!("frame-{}.png",index+1),&cancellation).await?);
                    }}
                    if let Some(atlas)=result.atlas {
                        let (width,height)=dimensions(&atlas.buffer,"image/png")?;
                        if atlas.wire["width"].as_u64()!=Some(width as u64)||atlas.wire["height"].as_u64()!=Some(height as u64){return Err(image_protocol::invalid());}
                        let name=settings["filename"].as_str().filter(|name|!name.is_empty()).unwrap_or("animation");
                        let media=self.media.upload(&format!("{name}.png"),"image/png",&atlas.buffer).await?;
                        output["atlas"]=json!({"media":media,"width":width,"height":height,"frames":atlas.wire["frames"]});
                    }
                }
            }
            active(&cancellation)?;
            let output=image_protocol::output(&output)?;
            let count=output["frames"].as_array().map(Vec::len).unwrap_or(0);
            Ok(ImageNodeResult{output,summary:format!("{count} frames"),assets:vec![]})
        }.await;
        result.map_err(|error| {
            let safe = [
                "workflow_image_invalid",
                "workflow_image_invalid_edits",
                "workflow_image_source_required",
                "workflow_image_too_large",
                "workflow_image_cancelled",
                "workflow_image_closed",
                "workflow_image_generation_failed",
                "workflow_image_processing_failed",
                "workflow_image_timeout",
                "workflow_media_invalid",
                "workflow_media_missing",
                "workflow_media_too_large",
                "sprite_engine_missing",
                "sprite_engine_invalid",
            ];
            let code = if cancellation.is_cancelled() {
                "workflow_image_cancelled"
            } else if safe.contains(&error.code.as_str()) {
                &error.code
            } else if generation {
                "workflow_image_generation_failed"
            } else {
                "workflow_image_processing_failed"
            };
            let mut mapped = failure(code);
            if output["frames"]
                .as_array()
                .is_some_and(|frames| !frames.is_empty())
            {
                if let Ok(partial) = image_protocol::output(&output) {
                    mapped = mapped.with_partial_output(partial);
                }
            }
            mapped
        })
    }
    // 通用图片操作使用此入口；DAG 的合法节点种类保持 release 契约。
    pub(crate) async fn edit(
        self: &Arc<Self>,
        images: Value,
        settings: Value,
        edits: Value,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Value> {
        active(&cancellation)?;
        let images = image_protocol::output(&images)?;
        let settings = image_protocol::settings(Some(&settings))?;
        let parsed = image_protocol::frame_edits(&edits)?;
        let frames = images["frames"]
            .as_array()
            .ok_or_else(image_protocol::invalid)?;
        if parsed.iter().any(|edit| edit.source_index >= frames.len()) {
            return Err(image_protocol::invalid_edits());
        }
        self.tracked(cancellation, move |service, cancellation| async move {
            let frames = images["frames"]
                .as_array()
                .ok_or_else(image_protocol::invalid)?;
            let loaded = service.pixels(frames, &cancellation).await?;
            let result = service
                .processor
                .process(
                    ProcessingRequest {
                        operation: "edit".into(),
                        frames: loaded,
                        settings,
                        edits: Some(edits),
                    },
                    cancellation.clone(),
                )
                .await?;
            let mut output = Vec::new();
            for (index, frame) in result.frames.into_iter().enumerate() {
                active(&cancellation)?;
                output.push(
                    service
                        .save(
                            frame.wire,
                            &frame.buffer,
                            "image/png",
                            &format!("frame-{}.png", index + 1),
                            &cancellation,
                        )
                        .await?,
                );
            }
            active(&cancellation)?;
            image_protocol::output(&json!({"type":"workflow-images","version":1,"frames":output}))
        })
        .await
    }
    pub(crate) async fn dispose(&self) {
        self.closed.store(true, Ordering::Release);
        let jobs = self
            .jobs
            .lock()
            .map(|jobs| jobs.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for job in &jobs {
            job.cancellation.cancel();
        }
        for job in jobs {
            let notified = job.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !job.finished.load(Ordering::Acquire) {
                notified.await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::{image_processing::encode_png, test_support::TempDirectory};
    struct Generator {
        calls: Arc<Mutex<Vec<Value>>>,
        fail_west: AtomicBool,
        escape: bool,
        bytes: Vec<u8>,
    }
    impl ImageGenerator for Generator {
        fn generate(
            &self,
            request: ImageGenerationRequest,
            _cancellation: Arc<RunCancellation>,
        ) -> BoxFuture<'_, Result<GeneratedImage>> {
            Box::pin(async move {
                assert!(request.source_images.iter().all(|path| path
                    .extension()
                    .is_some_and(|extension| ["png", "jpg", "webp"]
                        .contains(&extension.to_str().unwrap_or("")))
                    && path.is_file()));
                self.calls.lock().unwrap().push(json!({"name":request.output_name,"model":request.model,"prompt":request.prompt,"aspectRatio":request.aspect_ratio}));
                if request.output_name == "action-W" && self.fail_west.swap(false, Ordering::AcqRel)
                {
                    return Err(WorkflowError::invalid("private provider request failed"));
                }
                let path = if self.escape {
                    request.cwd.join("outside.png")
                } else {
                    let directory = request.cwd.join("generated").join("visuals");
                    std::fs::create_dir_all(&directory).unwrap();
                    directory.join(format!("{}.png", request.output_name))
                };
                std::fs::write(&path, &self.bytes).unwrap();
                Ok(GeneratedImage {
                    path,
                    mime_type: "image/png".into(),
                })
            })
        }
    }
    fn request(
        node: Value,
        inputs: Value,
        predecessors: Vec<Value>,
        resume_output: Option<Value>,
    ) -> ImageNodeRequest {
        ImageNodeRequest {
            node,
            inputs,
            predecessors,
            workflow_id: "fixture-workflow".into(),
            run_id: "fixture-run".into(),
            cwd: "fixture".into(),
            cancellation: Arc::new(RunCancellation::default()),
            resume_output,
        }
    }
    async fn fixture(
        directory: &TempDirectory,
        fail_west: bool,
        escape: bool,
    ) -> (
        Arc<MediaService>,
        Arc<ImageNodeService>,
        Arc<Generator>,
        Value,
    ) {
        let media = MediaService::open(&directory.path).unwrap();
        let bytes = encode_png(&image::RgbaImage::from_pixel(
            4,
            2,
            image::Rgba([10, 20, 30, 255]),
        ))
        .unwrap();
        let reference = media
            .upload("reference.png", "image/png", &bytes)
            .await
            .unwrap();
        let generator = Arc::new(Generator {
            calls: Arc::new(Mutex::new(vec![])),
            fail_west: AtomicBool::new(fail_west),
            escape,
            bytes,
        });
        let service = ImageNodeService::open(
            &directory.path,
            media.clone(),
            ImageProcessor::new(None),
            generator.clone(),
        )
        .unwrap();
        (media, service, generator, reference)
    }
    #[tokio::test]
    async fn manual_edits_save_independent_png_frames_and_metadata_survive_media_reopen() {
        let directory = TempDirectory::new();
        let (media, service, generator, reference) = fixture(&directory, false, false).await;
        let first = json!({"media":reference,"width":4,"height":2,"durationMs":125,
            "action":"walk","direction":"S","columns":2,"rows":1,"frameCount":2});
        let second_pixels = image::RgbaImage::from_pixel(3, 5, image::Rgba([90, 40, 220, 77]));
        let second_ref = media
            .upload(
                "second.png",
                "image/png",
                &encode_png(&second_pixels).unwrap(),
            )
            .await
            .unwrap();
        let second = json!({"media":second_ref,"width":3,"height":5,"durationMs":125,
            "action":"jump","direction":"N","columns":1,"rows":1,"frameCount":1});
        let result=service.edit(json!({"type":"workflow-images","version":1,"frames":[first.clone(),second]}),
            json!({"padding":64,"maxFrameSize":16,"align":"bottom-center","trim":true}),
            json!({"frames":[{"sourceIndex":1,"durationMs":240},
                {"sourceIndex":0,"opacity":0.5,"durationMs":500},
                {"sourceIndex":1,"durationMs":60,"eraseStrokes":[{"radius":0.25,"points":[{"x":0.5,"y":0.5}]}]}]}),
            Arc::new(RunCancellation::default())).await.unwrap();
        let frames = result["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames
                .iter()
                .map(|f| f["durationMs"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![240, 500, 60]
        );
        assert_eq!(
            frames
                .iter()
                .map(|f| (f["width"].as_u64().unwrap(), f["height"].as_u64().unwrap()))
                .collect::<Vec<_>>(),
            vec![(3, 5), (4, 2), (3, 5)]
        );
        assert_eq!(
            (
                frames[0]["action"].as_str(),
                frames[0]["direction"].as_str()
            ),
            (Some("jump"), Some("N"))
        );
        assert_eq!(
            (
                frames[1]["action"].as_str(),
                frames[1]["direction"].as_str()
            ),
            (Some("walk"), Some("S"))
        );
        for frame in frames {
            for key in ["columns", "rows", "frameCount"] {
                assert_eq!(frame[key], 1);
            }
            assert_eq!(frame["media"]["mimeType"], "image/png");
        }
        assert_ne!(frames[0]["media"]["id"], frames[2]["media"]["id"]);
        assert!(generator.calls.lock().unwrap().is_empty());
        assert!(service.jobs.lock().unwrap().is_empty());
        service.dispose().await;
        service.processor.dispose().await;
        let reopened = MediaService::open(&directory.path).unwrap();
        let mut stored_pixels = Vec::new();
        for frame in frames {
            let stored = reopened
                .read(frame["media"]["id"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(stored.metadata["media"], frame["media"]);
            let pixels = image::load_from_memory(&stored.buffer)
                .unwrap()
                .into_rgba8();
            assert_eq!(
                (pixels.width() as u64, pixels.height() as u64),
                (
                    frame["width"].as_u64().unwrap(),
                    frame["height"].as_u64().unwrap()
                )
            );
            stored_pixels.push(pixels);
        }
        assert_eq!(stored_pixels[0], second_pixels);
        assert!(stored_pixels[1].pixels().all(|p| p.0 == [10, 20, 30, 128]));
        assert_ne!(stored_pixels[0], stored_pixels[2]);
        let original = reopened
            .read(reference["id"].as_str().unwrap())
            .await
            .unwrap();
        assert!(image::load_from_memory(&original.buffer)
            .unwrap()
            .into_rgba8()
            .pixels()
            .all(|p| p.0 == [10, 20, 30, 255]));
        assert_eq!(first["media"], reference);
    }
    #[tokio::test]
    async fn manual_edit_rejects_out_of_range_sources_and_cancel_without_paid_calls() {
        let directory = TempDirectory::new();
        let (_media, service, generator, reference) = fixture(&directory, false, false).await;
        let images = json!({"type":"workflow-images","version":1,"frames":[{"media":reference,
            "width":4,"height":2,"durationMs":125,"action":"idle","direction":"S"}]});
        assert_eq!(
            service
                .edit(
                    images.clone(),
                    json!({}),
                    json!({"frames":[{"sourceIndex":1}]}),
                    Arc::new(RunCancellation::default())
                )
                .await
                .err()
                .unwrap()
                .code,
            "workflow_image_invalid_edits"
        );
        let cancellation = Arc::new(RunCancellation::default());
        cancellation.cancel();
        assert_eq!(
            service
                .edit(
                    images.clone(),
                    json!({}),
                    json!({"frames":[{"sourceIndex":0}]}),
                    cancellation
                )
                .await
                .err()
                .unwrap()
                .code,
            "workflow_image_cancelled"
        );
        let output = service
            .edit(
                images,
                json!({}),
                json!({"frames":[{"sourceIndex":0}]}),
                Arc::new(RunCancellation::default()),
            )
            .await
            .unwrap();
        assert_eq!(output["frames"][0]["durationMs"], 125);
        assert!(generator.calls.lock().unwrap().is_empty());
        service.dispose().await;
        assert_eq!(
            service
                .edit(
                    output,
                    json!({}),
                    json!({"frames":[{"sourceIndex":0}]}),
                    Arc::new(RunCancellation::default())
                )
                .await
                .err()
                .unwrap()
                .code,
            "workflow_image_closed"
        );
        service.processor.dispose().await;
    }
    #[tokio::test]
    async fn native_media_nodes_load_process_preview_and_export_actual_png() {
        let directory = TempDirectory::new();
        let (media, service, generator, reference) = fixture(&directory, false, false).await;
        let input = service
            .execute(request(
                json!({"kind":"media-input","image":{"inputName":"reference"}}),
                json!({"reference":reference}),
                vec![],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(input.output["frames"][0]["media"], reference);
        let split = service
            .execute(request(
                json!({"kind":"media-frames","image":{"columns":2,"rows":1,"frameCount":2}}),
                json!({}),
                vec![json!({"output":input.output})],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(split.output["frames"].as_array().unwrap().len(), 2);
        assert_eq!(split.output["frames"][0]["width"], 2);
        let preview = service
            .execute(request(
                json!({"kind":"media-preview"}),
                json!({}),
                vec![json!({"output":split.output})],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(preview.output, split.output);
        let exported = service
            .execute(request(
                json!({"kind":"media-export","image":{"padding":0}}),
                json!({}),
                vec![json!({"output":split.output})],
                None,
            ))
            .await
            .unwrap();
        assert_eq!(exported.output["frames"], split.output["frames"]);
        let atlas = media
            .read(exported.output["atlas"]["media"]["id"].as_str().unwrap())
            .await
            .unwrap();
        let pixels = image::load_from_memory(&atlas.buffer).unwrap().into_rgba8();
        assert!(pixels.pixels().all(|pixel| pixel.0 == [10, 20, 30, 255]));
        assert!(generator.calls.lock().unwrap().is_empty());
        service.dispose().await;
        service.processor.dispose().await;
    }
    #[tokio::test]
    async fn failed_paid_generation_retains_completed_direction_and_resume_never_regenerates_it() {
        let directory = TempDirectory::new();
        let (media, service, generator, reference) = fixture(&directory, true, false).await;
        let source = service
            .execute(request(
                json!({"kind":"media-input"}),
                json!({"reference":reference}),
                vec![],
                None,
            ))
            .await
            .unwrap()
            .output;
        let node = json!({"kind":"media-generate","model":{"provider":"fixture","model":"image"},"prompt":"custom","image":{"directions":["S","W"],"colors":["#ff00ff"],"frameCount":4}});
        let failed = service
            .execute(request(
                node.clone(),
                json!({}),
                vec![json!({"output":source})],
                None,
            ))
            .await
            .err()
            .unwrap();
        assert_eq!(failed.code, "workflow_image_generation_failed");
        assert!(!failed.message.contains("private provider"));
        let partial = failed.partial_output.unwrap();
        assert_eq!(partial["frames"].as_array().unwrap().len(), 1);
        assert_eq!(partial["frames"][0]["direction"], "S");
        let retained_id = partial["frames"][0]["media"]["id"].clone();
        assert!(media.read(retained_id.as_str().unwrap()).await.is_ok());
        let completed = service
            .execute(request(
                node,
                json!({}),
                vec![json!({"output":source})],
                Some(partial),
            ))
            .await
            .unwrap();
        assert_eq!(completed.output["frames"].as_array().unwrap().len(), 2);
        assert_eq!(completed.output["frames"][0]["media"]["id"], retained_id);
        let calls = generator.calls.lock().unwrap().clone();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["name"] == "action-S")
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["name"] == "action-W")
                .count(),
            2
        );
        assert!(calls[0]["prompt"]
            .as_str()
            .unwrap()
            .contains("Preserve reference art style"));
        assert_eq!(calls[0]["aspectRatio"], "16:9");
        assert_eq!(
            std::fs::read_dir(directory.path.join("image-operation-jobs"))
                .unwrap()
                .count(),
            0
        );
        service.dispose().await;
        service.processor.dispose().await;
    }
    #[tokio::test]
    async fn invalid_cached_reference_is_rejected_before_paid_call_and_generated_path_cannot_escape(
    ) {
        let directory = TempDirectory::new();
        let (_, service, generator, reference) = fixture(&directory, false, true).await;
        let source = service
            .execute(request(
                json!({"kind":"media-input"}),
                json!({"reference":reference}),
                vec![],
                None,
            ))
            .await
            .unwrap()
            .output;
        let node = json!({"kind":"media-generate","image":{"directions":["S"],"colors":["#ff00ff"],"frameCount":4}});
        let mut cached = source.clone();
        cached["frames"][0]["columns"] = json!(4);
        cached["frames"][0]["frameCount"] = json!(4);
        cached["frames"][0]["action"] = json!("idle");
        cached["frames"][0]["direction"] = json!("S");
        cached["frames"][0]["media"]["name"] = json!("forged.png");
        let failed = service
            .execute(request(
                node.clone(),
                json!({}),
                vec![json!({"output":source})],
                Some(cached),
            ))
            .await
            .err()
            .unwrap();
        assert_eq!(failed.code, "workflow_media_invalid");
        assert!(generator.calls.lock().unwrap().is_empty());
        let failed = service
            .execute(request(
                node,
                json!({}),
                vec![json!({"output":source})],
                None,
            ))
            .await
            .err()
            .unwrap();
        assert_eq!(failed.code, "workflow_image_invalid");
        assert!(failed.partial_output.is_none());
        assert_eq!(
            std::fs::read_dir(directory.path.join("image-operation-jobs"))
                .unwrap()
                .count(),
            0
        );
        service.dispose().await;
        service.processor.dispose().await;
    }
}
