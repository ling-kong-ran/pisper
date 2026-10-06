use super::{
    catalog::{self, js_string_checked, js_whitespace, string_or_empty, truthy},
    drivers::{self, DetectionCache},
    input, output, transport, PreparedRequest, Result, VisualConfigPort, VisualError, VisualKind,
    VisualModel, VisualOperation, VisualOptions, VisualRequest, VisualResult,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub struct VisualGenerationService {
    config: VisualConfigPort,
    detection: DetectionCache,
    shutdown: CancellationToken,
    live: Mutex<HashMap<u64, CancellationToken>>,
    next: AtomicU64,
    settled: Notify,
}
struct GenerationGuard {
    service: Weak<VisualGenerationService>,
    id: u64,
    token: CancellationToken,
    bridge: tokio::task::JoinHandle<()>,
}
impl Drop for GenerationGuard {
    fn drop(&mut self) {
        self.token.cancel();
        self.bridge.abort();
        if let Some(service) = self.service.upgrade() {
            service.live.lock().unwrap().remove(&self.id);
            service.settled.notify_waiters();
        }
    }
}
impl VisualGenerationService {
    pub fn new(config: VisualConfigPort) -> Arc<Self> {
        Arc::new(Self {
            config,
            detection: DetectionCache::default(),
            shutdown: CancellationToken::new(),
            live: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            settled: Notify::new(),
        })
    }
    pub async fn get_model_status(&self, kind: VisualKind) -> Result<Value> {
        let snapshot = (self.config.read)().await?;
        catalog::status(&snapshot, kind)
    }
    pub async fn get_all_status(&self) -> Result<Value> {
        let snapshot = (self.config.read)().await?;
        let image = catalog::status(&snapshot, VisualKind::Image)?;
        let video = catalog::status(&snapshot, VisualKind::Video)?;
        Ok(combined(image, video))
    }
    pub async fn set_preferred_model(&self, kind: VisualKind, requested: &Value) -> Result<Value> {
        let requested = string_or_empty(requested)?
            .trim_matches(js_whitespace)
            .to_owned();
        let selection = if requested.is_empty() {
            None
        } else {
            let snapshot = (self.config.read)().await?;
            Some(catalog::reference(&catalog::select(
                &snapshot,
                kind,
                Some(&requested),
            )?))
        };
        (self.config.write_preference)(kind, selection).await?;
        self.get_all_status().await
    }
    pub async fn generate(
        self: &Arc<Self>,
        request: VisualRequest,
        mut options: VisualOptions,
    ) -> Result<VisualResult> {
        if self.shutdown.is_cancelled() {
            return Err(VisualError::cancelled());
        }
        let token = self.shutdown.child_token();
        let original = options.cancellation.clone();
        if original.is_cancelled() {
            token.cancel();
        }
        let bridged = token.clone();
        let bridge = tokio::spawn(async move {
            tokio::select! {_=original.cancelled()=>bridged.cancel(),_=bridged.cancelled()=>{}}
        });
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.live.lock().unwrap().insert(id, token.clone());
        let _guard = GenerationGuard {
            service: Arc::downgrade(self),
            id,
            token: token.clone(),
            bridge,
        };
        options.cancellation = token;
        self.generate_owned(request, &options).await
    }
    async fn generate_owned(
        &self,
        request: VisualRequest,
        options: &VisualOptions,
    ) -> Result<VisualResult> {
        let kind = if request.input["kind"] == "video" {
            VisualKind::Video
        } else {
            VisualKind::Image
        };
        input::validate_video_inputs(
            kind,
            &request.input["sourceImages"],
            &request.input["maskPath"],
        )?;
        let requested = string_or_empty(&request.input["model"])?
            .trim_matches(js_whitespace)
            .to_owned();
        let snapshot = tokio::select! {biased;_=options.cancellation.cancelled()=>return Err(VisualError::cancelled()),value=(self.config.read)()=>value?};
        let models = if requested.is_empty() {
            catalog::all(&snapshot, kind)?
        } else {
            vec![catalog::select(&snapshot, kind, Some(&requested))?]
        };
        let sources =
            input::load_source_images(&request.input["sourceImages"], &request.cwd).await?;
        let mask = input::load_mask_image(&request.input["maskPath"], &request.cwd).await?;
        let operation = if sources.is_empty() {
            VisualOperation::Generate
        } else {
            VisualOperation::Edit
        };
        let client = transport::client()?;
        let mut attempted_models = Vec::new();
        for (index, model) in models.iter().enumerate() {
            attempted_models.push(catalog::reference(model));
            let input = normalize_request(model, &request.input, kind);
            let prepared = PreparedRequest {
                input,
                kind,
                operation,
                sources: sources.clone(),
                mask: mask.clone(),
            };
            if let Some(callback) = &options.on_progress {
                let action = if operation == VisualOperation::Edit {
                    "编辑图片"
                } else if kind == VisualKind::Video {
                    "生成视频"
                } else {
                    "生成图片"
                };
                callback(format!(
                    "使用 {} / {} {action}…",
                    js_string_checked(&model.public["providerName"])?,
                    js_string_checked(&model.public["name"])?
                ));
            }
            let result = async {
                let result =
                    drivers::run(&client, &self.detection, model, &prepared, options).await?;
                let prompt = string_or_empty(&request.input["prompt"])?;
                let output_name = if truthy(&request.input["outputName"]) {
                    Some(js_string_checked(&request.input["outputName"])?)
                } else {
                    None
                };
                let path = output::save_visual_output(
                    &request.cwd,
                    &prompt,
                    output_name.as_deref(),
                    &result,
                )
                .await?;
                Ok::<_, VisualError>(VisualResult {
                    path,
                    kind,
                    mime_type: result.mime_type,
                    size: result.bytes.len() as u64,
                    provider: model.string("providerId").into(),
                    provider_name: js_string_checked(&model.public["providerName"])?,
                    model: model.string("id").into(),
                    model_name: js_string_checked(&model.public["name"])?,
                    remote_id: result.remote_id.filter(|value| !value.is_empty()),
                    operation,
                    fallback_used: index > 0,
                    attempted_models: attempted_models.clone(),
                })
            }
            .await;
            match result {
                Ok(result) => return Ok(result),
                Err(error) => {
                    let next = models.get(index + 1);
                    if !options.allow_fallback
                        || !requested.is_empty()
                        || next.is_none()
                        || !can_fallback(&error, options, next.unwrap(), model)
                    {
                        return Err(transport::redact_model_error(error, model));
                    }
                    if let Some(callback) = &options.on_progress {
                        let next = next.unwrap();
                        callback(format!(
                            "{} / {} 当前不可用，尝试 {} / {}…",
                            js_string_checked(&model.public["providerName"])?,
                            js_string_checked(&model.public["name"])?,
                            js_string_checked(&next.public["providerName"])?,
                            js_string_checked(&next.public["name"])?
                        ));
                    }
                }
            }
        }
        Err(VisualError::new(format!(
            "没有已配置并启用的{}生成模型。请先在配置页添加视觉模型。",
            if kind == VisualKind::Video {
                "视频"
            } else {
                "图像"
            }
        )))
    }
    pub async fn test_visual(
        self: &Arc<Self>,
        data_dir: &Path,
        options: VisualOptions,
    ) -> Result<Value> {
        let result=self.generate(VisualRequest {cwd:data_dir.join("visual-test"),input:json!({"kind":"image","prompt":"a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background","outputName":"config-test"})},options).await?;
        let mut response =
            serde_json::to_value(&result).map_err(|error| VisualError::new(error.to_string()))?;
        let preview = tokio::fs::read(&result.path)
            .await
            .ok()
            .filter(|bytes| bytes.len() <= 6 * 1024 * 1024)
            .map(|bytes| {
                format!(
                    "data:{};base64,{}",
                    result.mime_type,
                    STANDARD.encode(bytes)
                )
            })
            .unwrap_or_default();
        response["previewDataUrl"] = json!(preview);
        Ok(response)
    }
    pub async fn dispose(&self) {
        self.shutdown.cancel();
        loop {
            let settled = self.settled.notified();
            if self.live.lock().unwrap().is_empty() {
                break;
            }
            settled.await;
        }
    }
    #[cfg(test)]
    pub(super) fn live_count(&self) -> usize {
        self.live.lock().unwrap().len()
    }
}
fn combined(image: Value, video: Value) -> Value {
    json!({"image":image["model"],"video":video["model"],"imageModels":image["models"],"videoModels":video["models"],"imageSelection":image["selection"],"videoSelection":video["selection"]})
}
pub(super) fn normalize_request(model: &VisualModel, input: &Value, kind: VisualKind) -> Value {
    let mut value = input.clone();
    let google = model.string("driver").starts_with("google-");
    if !truthy(&value["size"]) && truthy(&value["aspectRatio"]) {
        if kind == VisualKind::Video {
            value["size"] = json!(if value["aspectRatio"] == "9:16" {
                "720x1280"
            } else {
                "1280x720"
            });
        } else if !google {
            value["size"] = json!(match value["aspectRatio"].as_str() {
                Some("9:16" | "3:4") => "1024x1536",
                Some("16:9" | "4:3") => "1536x1024",
                _ => "1024x1024",
            });
        }
    }
    if google
        && kind == VisualKind::Video
        && !truthy(&value["resolution"])
        && truthy(&value["size"])
    {
        value["resolution"] = json!(if value["size"]
            .as_str()
            .is_some_and(|size| size.contains("1080"))
        {
            "1080p"
        } else {
            "720p"
        });
    }
    value
}
pub(super) fn can_fallback(
    error: &VisualError,
    options: &VisualOptions,
    next: &VisualModel,
    current: &VisualModel,
) -> bool {
    if options.cancellation.is_cancelled() || error.cancelled {
        return false;
    }
    static SAFETY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if SAFETY.get_or_init(||regex::Regex::new(r"(?i)(?:content policy|safety system|moderation|内容安全|安全策略|审核拒绝|违规内容)").unwrap()).is_match(&error.message){return false;}
    if matches!(error.status, Some(401 | 403))
        && next.string("providerId") != current.string("providerId")
    {
        return true;
    }
    static UNAVAILABLE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    error.status.is_some_and(|status|matches!(status,404|408|409|425|429|500|502|503|504)) ||
        UNAVAILABLE.get_or_init(||regex::Regex::new(r"(?i)(?:model[_ -]?not[_ -]?found|unknown provider for model|unsupported model|model .* unavailable|no available (?:channel|provider|model)|(?:token|key|credential).*(?:cannot|can't|not authorized to|no permission to|does not have).*access.*model|无可用渠道|模型不存在|模型不可用|未找到.*模型|没有.*渠道|(?:令牌|密钥|凭证).*无权访问模型)").unwrap()).is_match(&error.message)
}
