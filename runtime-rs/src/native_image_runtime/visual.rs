//! release 图像协议的原生 HTTP 驱动；付费生成只发起一次，不隐式重试。
use crate::{
    native_workflow::{
        image_nodes::{GeneratedImage, ImageGenerationRequest},
        media, Result, WorkflowError,
    },
    workflow_engine::RunCancellation,
    AppState,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::StreamExt;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    multipart::{Form, Part},
    Client, Response,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_JSON: usize = 16 * 1024 * 1024;
#[derive(Clone)]
struct VisualModel {
    public: Value,
    key: String,
    headers: HeaderMap,
    configured: bool,
    visual: bool,
    score: i64,
}
impl VisualModel {
    fn value(&self, name: &str) -> &str {
        self.public[name].as_str().unwrap_or("")
    }
    fn reference(&self) -> String {
        format!("{}/{}", self.value("providerId"), self.value("id"))
    }
}
fn failure(code: &str, message: &str) -> WorkflowError {
    WorkflowError::coded(code, message)
}
fn document(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(_) => Err(WorkflowError::io("视觉模型配置无法读取。")),
        Ok(text) => {
            if text.len() > 16 * 1024 * 1024 {
                return Err(WorkflowError::io("视觉模型配置超出大小限制。"));
            }
            let text = pi_rust::coding_agent::utils::json::strip_json_comments(
                text.trim_start_matches('\u{feff}'),
            );
            let value: Value = serde_json::from_str(&text)
                .map_err(|_| WorkflowError::io("视觉模型配置已损坏。"))?;
            if !value.is_object() {
                return Err(WorkflowError::io("视觉模型配置必须是对象。"));
            }
            Ok(value)
        }
    }
}
fn credential(value: &Value) -> &str {
    value.as_str().unwrap_or_else(|| {
        ["key", "token", "access_token"]
            .iter()
            .find_map(|field| value[field].as_str())
            .unwrap_or("")
    })
}
fn default_url(provider: &str, api: &str) -> &'static str {
    let value = format!("{provider} {api}").to_lowercase();
    if value.contains("google") {
        "https://generativelanguage.googleapis.com/v1beta"
    } else if ["xai", "x-ai", "grok"]
        .iter()
        .any(|pattern| value.contains(pattern))
    {
        "https://api.x.ai/v1"
    } else if value.contains("openrouter") {
        "https://openrouter.ai/api/v1"
    } else if value.contains("openai") {
        "https://api.openai.com/v1"
    } else {
        ""
    }
}
fn driver(value: &Value) -> String {
    if let Some(explicit) = value["visualApi"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        return explicit.into();
    }
    let identity = format!(
        "{} {} {} {}",
        value["providerId"], value["api"], value["baseUrl"], value["id"]
    )
    .to_lowercase();
    if identity.contains("google") || identity.contains("generativelanguage.googleapis.com") {
        "google-image"
    } else if identity.contains("xai") || identity.contains("x.ai") || identity.contains("grok") {
        "xai-image"
    } else if identity.contains("openrouter") {
        "openrouter-image"
    } else {
        "openai-image"
    }
    .into()
}
fn score(provider: &str, id: &str) -> i64 {
    let id = id.to_lowercase();
    let mut score = if id.contains("gpt-image-2") || id.contains("gpt-5.4-image") {
        120
    } else if id.contains("gpt-image") || (id.contains("gpt-") && id.contains("image")) {
        105
    } else {
        0
    };
    if id.contains("gemini-3") || id.contains("imagen-4") {
        score += 100;
    }
    if id.contains("grok-imagine") {
        score += 95;
    }
    score
        + match provider {
            "openai" => 8,
            "google" => 7,
            "xai" => 6,
            _ => 0,
        }
}
fn configured_models(
    models: &Value,
    auth: &Value,
    app: &Value,
    runtime: &[Value],
) -> Result<Vec<VisualModel>> {
    let mut providers = BTreeMap::<String, Value>::new();
    for (id, provider) in models["providers"].as_object().into_iter().flatten() {
        providers.insert(id.clone(), provider.clone());
    }
    for model in runtime {
        if let Some(provider) = model["provider"].as_str() {
            providers.entry(provider.into()).or_insert(json!({}));
        }
    }
    let mut candidates = Vec::new();
    for (provider_id, provider) in providers {
        if app["disabledProviders"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == &provider_id))
        {
            continue;
        }
        let key = credential(&auth[&provider_id]);
        if key.is_empty() {
            continue;
        }
        let definitions = provider["models"].as_array().cloned().unwrap_or_default();
        let mut definitions_by_id = BTreeMap::<String, Value>::new();
        for definition in &definitions {
            if let Some(id) = definition["id"].as_str() {
                definitions_by_id.insert(id.into(), definition.clone());
            }
        }
        let visual = app["providerTypes"][&provider_id]
            .as_str()
            .map(|kind| kind == "visual")
            .unwrap_or(
                !definitions.is_empty()
                    && definitions.iter().all(|model| {
                        ["image", "video"].contains(&model["kind"].as_str().unwrap_or("chat"))
                    }),
            );
        for model in runtime
            .iter()
            .filter(|model| model["provider"] == provider_id)
        {
            if let Some(id) = model["id"].as_str() {
                definitions_by_id
                    .entry(id.into())
                    .or_insert(json!({"id":id,"kind":model["pisperKind"]}));
            }
        }
        for (id, definition) in definitions_by_id {
            let supports = definition["capabilities"]
                .as_array()
                .filter(|values| !values.is_empty())
                .map(|values| values.iter().any(|kind| kind == "image"))
                .unwrap_or(definition["kind"] == "image");
            if !supports {
                continue;
            }
            let native = runtime
                .iter()
                .find(|model| model["provider"] == provider_id && model["id"] == id)
                .cloned()
                .unwrap_or(json!({}));
            let choose = |name: &str| {
                definition[name]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .or_else(|| provider[name].as_str().filter(|value| !value.is_empty()))
                    .or_else(|| native[name].as_str().filter(|value| !value.is_empty()))
                    .unwrap_or("")
                    .to_owned()
            };
            let api = choose("api");
            let selected_url = choose("baseUrl");
            let base = if selected_url.is_empty() {
                default_url(&provider_id, &api).into()
            } else {
                selected_url.trim_end_matches('/').to_owned()
            };
            if base.is_empty() {
                continue;
            }
            let url = reqwest::Url::parse(&base)
                .map_err(|_| failure("visual_provider_invalid", "视觉连接地址无效。"))?;
            if !["http", "https"].contains(&url.scheme())
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(failure("visual_provider_invalid", "视觉连接地址无效。"));
            }
            let mut headers = HeaderMap::new();
            for source in [
                &provider["headers"],
                &native["headers"],
                &definition["headers"],
            ] {
                for (name, value) in source.as_object().into_iter().flatten() {
                    let value = value.as_str().ok_or_else(|| {
                        failure("visual_provider_invalid", "视觉连接请求头无效。")
                    })?;
                    headers.insert(
                        HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                            failure("visual_provider_invalid", "视觉连接请求头无效。")
                        })?,
                        HeaderValue::from_str(value).map_err(|_| {
                            failure("visual_provider_invalid", "视觉连接请求头无效。")
                        })?,
                    );
                }
            }
            let model_score = score(&provider_id, &id);
            let name = definition["name"]
                .as_str()
                .or_else(|| native["name"].as_str())
                .unwrap_or(&id);
            let mut public = json!({"id":id,"name":name,"providerId":provider_id,"providerName":provider["name"].as_str().unwrap_or(&provider_id),"api":api,"kind":"image","baseUrl":base,"visualApi":choose("visualApi"),"score":model_score});
            public["driver"] = json!(driver(&public));
            let configured = definitions.iter().any(|model| model["id"] == id);
            candidates.push(VisualModel {
                public,
                key: key.into(),
                headers,
                configured,
                visual,
                score: model_score,
            });
        }
    }
    Ok(candidates)
}
fn ordered(candidates: Vec<VisualModel>, preferred: &str) -> Vec<VisualModel> {
    let mut unique = HashMap::<String, VisualModel>::new();
    for model in candidates {
        let key = format!(
            "{}\0{}\0{}",
            model.value("baseUrl").trim_end_matches('/').to_lowercase(),
            model.value("id").to_lowercase(),
            model.value("driver")
        );
        let priority = |model: &VisualModel| {
            i64::from(model.visual) * 10000 + i64::from(model.configured) * 1000 + model.score
        };
        if unique
            .get(&key)
            .is_none_or(|old| priority(&model) > priority(old))
        {
            unique.insert(key, model);
        }
    }
    let mut models = unique.into_values().collect::<Vec<_>>();
    models.sort_by(|left, right| {
        let priority = |model: &VisualModel| {
            model.score + i64::from(model.visual) * 25 + i64::from(model.configured) * 2
        };
        priority(right)
            .cmp(&priority(left))
            .then_with(|| left.value("name").cmp(right.value("name")))
    });
    if let Some(index) = models
        .iter()
        .position(|model| model.reference().eq_ignore_ascii_case(preferred))
    {
        let selected = models.remove(index);
        models.insert(0, selected);
    }
    models
}
async fn candidates(state: &AppState) -> Result<(Vec<VisualModel>, String)> {
    let root = Path::new(&state.agent_dir);
    let models = document(&root.join("models.json"))?;
    let auth = document(&root.join("auth.json"))?;
    let app = document(&root.join("pisper.json"))?;
    let runtime = state
        .runtime
        .session()
        .model_runtime()
        .get_models(None)
        .await
        .into_iter()
        .map(|model| {
            serde_json::to_value(model).map_err(|_| WorkflowError::io("视觉模型目录无法读取。"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((
        configured_models(&models, &auth, &app, &runtime)?,
        app["visualDefaultModels"]["image"]
            .as_str()
            .unwrap_or("")
            .trim()
            .into(),
    ))
}
pub(crate) async fn models(state: &AppState) -> Result<Value> {
    let status = state
        .visual
        .get_model_status(crate::native_visual::VisualKind::Image)
        .await
        .map_err(visual_error)?;
    Ok(status["models"].clone())
}
fn select(candidates: Vec<VisualModel>, preferred: &str, request: &Value) -> Result<VisualModel> {
    let requested = request.as_str().map(str::to_owned).unwrap_or_else(|| {
        let provider = request["provider"].as_str().unwrap_or("");
        let model = request["model"].as_str().unwrap_or("");
        if provider.is_empty() || model.is_empty() {
            String::new()
        } else {
            format!("{provider}/{model}")
        }
    });
    if let Some(model) = candidates
        .iter()
        .find(|model| model.reference().eq_ignore_ascii_case(requested.trim()))
    {
        return Ok(model.clone());
    }
    let models = ordered(candidates, preferred);
    if requested.trim().is_empty() {
        models.into_iter().next().ok_or_else(|| {
            failure(
                "visual_model_missing",
                "没有已配置并启用的图像生成模型。请先添加视觉模型。",
            )
        })
    } else {
        models
            .into_iter()
            .find(|model| model.value("id").eq_ignore_ascii_case(requested.trim()))
            .ok_or_else(|| failure("visual_model_missing", "未找到已启用的视觉模型。"))
    }
}
fn headers(model: &VisualModel, google: bool) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let key = if google {
        model.key.clone()
    } else {
        format!("Bearer {}", model.key)
    };
    headers.insert(
        if google {
            "x-goog-api-key"
        } else {
            "authorization"
        },
        HeaderValue::from_str(&key)
            .map_err(|_| failure("visual_provider_invalid", "视觉凭据无效。"))?,
    );
    headers.extend(model.headers.clone());
    Ok(headers)
}
async fn bytes(
    response: Response,
    limit: usize,
    cancellation: &RunCancellation,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(failure("visual_output_too_large", "视觉响应超出大小限制。"));
    }
    let mut result = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::select! { _=cancellation.cancelled()=>return Err(WorkflowError::cancelled()), value=stream.next()=>value }
    {
        let chunk = chunk.map_err(|_| failure("visual_download_failed", "视觉响应读取失败。"))?;
        if result.len().saturating_add(chunk.len()) > limit {
            return Err(failure("visual_output_too_large", "视觉响应超出大小限制。"));
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}
async fn response(
    request: reqwest::RequestBuilder,
    cancellation: &RunCancellation,
) -> Result<Response> {
    tokio::select! { _=cancellation.cancelled()=>Err(WorkflowError::cancelled()), result=request.send()=>result.map_err(|_| failure("visual_request_failed", "视觉接口请求失败。请检查连接。")) }
}
async fn json_response(
    request: reqwest::RequestBuilder,
    model: &VisualModel,
    cancellation: &RunCancellation,
) -> Result<Value> {
    let response = response(request, cancellation).await?;
    let status = response.status();
    let value = serde_json::from_slice::<Value>(&bytes(response, MAX_JSON, cancellation).await?)
        .map_err(|_| failure("visual_response_invalid", "视觉接口返回了无效 JSON。"))?;
    if !status.is_success() {
        let raw = value["error"]["message"]
            .as_str()
            .or_else(|| value["error"].as_str())
            .or_else(|| value["message"].as_str())
            .unwrap_or("视觉接口拒绝了请求。");
        let mut error = failure(
            "visual_request_failed",
            &crate::security::redact_secret_text(&raw.replace(&model.key, "[REDACTED]")),
        );
        error.status = status;
        return Err(error);
    }
    Ok(value)
}
fn inline(value: &str, fallback: &str) -> Result<Option<(Vec<u8>, String)>> {
    let Some(data) = value.strip_prefix("data:") else {
        return Ok(None);
    };
    let (mime, encoded) = data
        .split_once(";base64,")
        .ok_or_else(|| failure("visual_response_invalid", "视觉图片数据无效。"))?;
    Ok(Some(decode(
        encoded,
        if mime.is_empty() { fallback } else { mime },
    )?))
}
fn decode(encoded: &str, mime: &str) -> Result<(Vec<u8>, String)> {
    if encoded.len() > MAX_IMAGE * 4 / 3 + 8 {
        return Err(failure("visual_output_too_large", "生成图片超过 8 MiB。"));
    }
    let result = STANDARD
        .decode(encoded)
        .map_err(|_| failure("visual_response_invalid", "视觉图片数据无效。"))?;
    validate_image(&result, mime)?;
    Ok((result, mime.into()))
}
fn validate_image(bytes: &[u8], mime: &str) -> Result<()> {
    let (width, height, actual) = media::raster_dimensions(bytes)
        .ok_or_else(|| failure("visual_response_invalid", "视觉接口没有返回有效图片。"))?;
    if bytes.is_empty()
        || bytes.len() > MAX_IMAGE
        || mime != actual
        || width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 16_000_000
    {
        return Err(failure(
            "visual_response_invalid",
            "视觉接口没有返回有效图片。",
        ));
    }
    image::load_from_memory_with_format(
        bytes,
        match mime {
            "image/png" => image::ImageFormat::Png,
            "image/jpeg" => image::ImageFormat::Jpeg,
            "image/webp" => image::ImageFormat::WebP,
            _ => return Err(failure("visual_response_invalid", "视觉图片类型无效。")),
        },
    )
    .map_err(|_| failure("visual_response_invalid", "视觉图片无法解码。"))?;
    Ok(())
}
async fn download(
    client: &Client,
    url: &str,
    authenticated: Option<&VisualModel>,
    cancellation: &RunCancellation,
) -> Result<(Vec<u8>, String)> {
    let url = reqwest::Url::parse(url)
        .map_err(|_| failure("visual_response_invalid", "生成图片下载地址无效。"))?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(failure("visual_response_invalid", "生成图片下载地址无效。"));
    }
    let mut request = client.get(url);
    if let Some(model) = authenticated {
        request = request.headers(headers(model, false)?);
    }
    let result = response(request, cancellation).await?;
    if !result.status().is_success() {
        return Err(failure("visual_download_failed", "下载生成图片失败。"));
    }
    let mime = result
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    let buffer = bytes(result, MAX_IMAGE, cancellation).await?;
    validate_image(&buffer, &mime)?;
    Ok((buffer, mime))
}
struct Source {
    path: PathBuf,
    mime: String,
    buffer: Vec<u8>,
}
fn sources(request: &ImageGenerationRequest) -> Result<Vec<Source>> {
    if request.source_images.len() > 8 {
        return Err(failure(
            "visual_source_invalid",
            "图片编辑最多支持 8 张来源图。",
        ));
    }
    let cwd = request
        .cwd
        .canonicalize()
        .map_err(|_| failure("visual_source_invalid", "图片工作目录无效。"))?;
    let mut total = 0;
    request
        .source_images
        .iter()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let actual = path
                .canonicalize()
                .map_err(|_| failure("visual_source_invalid", "来源图片无法读取。"))?;
            if !actual.starts_with(&cwd)
                || std::fs::symlink_metadata(&path)
                    .map_err(|_| failure("visual_source_invalid", "来源图片无法读取。"))?
                    .file_type()
                    .is_symlink()
            {
                return Err(failure("visual_source_invalid", "来源图片路径无效。"));
            }
            let buffer = media::read_bounded(&path, MAX_IMAGE as u64)?;
            total += buffer.len();
            if total > 20 * 1024 * 1024 {
                return Err(failure(
                    "visual_source_invalid",
                    "来源图片总大小超过 20 MiB。",
                ));
            }
            let mime = media::raster_dimensions(&buffer)
                .ok_or_else(|| failure("visual_source_invalid", "来源图片格式无效。"))?
                .2
                .to_owned();
            validate_image(&buffer, &mime)?;
            Ok(Source { path, mime, buffer })
        })
        .collect()
}
async fn drive(
    client: &Client,
    model: &VisualModel,
    request: &ImageGenerationRequest,
    sources: &[Source],
    cancellation: &RunCancellation,
) -> Result<(Vec<u8>, String)> {
    let root = model.value("baseUrl").trim_end_matches('/');
    let driver = model.value("driver");
    let size = match request.aspect_ratio.as_str() {
        "9:16" | "3:4" => "1024x1536",
        "16:9" | "4:3" => "1536x1024",
        _ => "1024x1024",
    };
    if driver == "google-image" {
        let mut parts=sources.iter().map(|image|json!({"inlineData":{"mimeType":image.mime,"data":STANDARD.encode(&image.buffer)}})).collect::<Vec<_>>();
        parts.push(json!({"text":request.prompt}));
        let mut url = reqwest::Url::parse(&format!("{root}/models/"))
            .map_err(|_| failure("visual_provider_invalid", "视觉地址无效。"))?;
        url.path_segments_mut()
            .map_err(|_| failure("visual_provider_invalid", "视觉地址无效。"))?
            .pop_if_empty()
            .push(&format!(
                "{}:generateContent",
                model.value("id").trim_start_matches("models/")
            ));
        let data=json_response(client.post(url).headers(headers(model,true)?).json(&json!({"contents":[{"role":"user","parts":parts}],"generationConfig":{"responseModalities":["TEXT","IMAGE"],"imageConfig":{"aspectRatio":request.aspect_ratio}}})),model,cancellation).await?;
        for part in data["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|candidate| {
                candidate["content"]["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
        {
            let image = part
                .get("inlineData")
                .or_else(|| part.get("inline_data"))
                .unwrap_or(&Value::Null);
            if let Some(encoded) = image["data"].as_str().filter(|value| !value.is_empty()) {
                return decode(
                    encoded,
                    image["mimeType"]
                        .as_str()
                        .or_else(|| image["mime_type"].as_str())
                        .unwrap_or("image/png"),
                );
            }
        }
    } else if driver == "openrouter-image" {
        let mut content = vec![json!({"type":"text","text":request.prompt})];
        content.extend(sources.iter().map(|image|json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",image.mime,STANDARD.encode(&image.buffer))}})));
        let data=json_response(client.post(format!("{root}/chat/completions")).headers(headers(model,false)?).json(&json!({"model":model.value("id"),"messages":[{"role":"user","content":content}],"modalities":["image"],"stream":false})),model,cancellation).await?;
        let value = &data["choices"][0]["message"]["images"][0]["image_url"];
        if let Some(url) = value.as_str().or_else(|| value["url"].as_str()) {
            if let Some(value) = inline(url, "image/png")? {
                return Ok(value);
            }
            return download(client, url, None, cancellation).await;
        }
    } else if ["openai-image", "new-api-image", "xai-image"].contains(&driver) {
        let xai = driver == "xai-image";
        let call = if sources.is_empty() {
            let mut body = json!({"model":model.value("id"),"prompt":request.prompt,"n":1});
            if xai {
                body["response_format"] = json!("b64_json");
                body["aspect_ratio"] = json!(request.aspect_ratio);
            } else {
                body["size"] = json!(size);
                if model.value("id").to_lowercase().contains("image")
                    || model.value("id").to_lowercase().contains("dall-e")
                {
                    body["output_format"] = json!("png");
                }
            }
            client
                .post(format!("{root}/images/generations"))
                .headers(headers(model, false)?)
                .json(&body)
        } else {
            let mut form = Form::new()
                .text("model", model.value("id").to_owned())
                .text("prompt", request.prompt.clone())
                .text("n", "1")
                .text("size", size);
            if !xai
                && (model.value("id").to_lowercase().contains("image")
                    || model.value("id").to_lowercase().contains("dall-e"))
            {
                form = form.text("output_format", "png");
            }
            for (index, image) in sources.iter().enumerate() {
                let filename = image
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("image.png")
                    .to_owned();
                let part = Part::bytes(image.buffer.clone())
                    .file_name(if xai {
                        format!("image-{}", index + 1)
                    } else {
                        filename
                    })
                    .mime_str(&image.mime)
                    .map_err(|_| failure("visual_source_invalid", "来源图片类型无效。"))?;
                form = form.part(
                    if !xai && sources.len() > 1 {
                        "image[]"
                    } else {
                        "image"
                    },
                    part,
                );
            }
            client
                .post(format!("{root}/images/edits"))
                .headers(headers(model, false)?)
                .multipart(form)
        };
        let data = json_response(call, model, cancellation).await?;
        let value = if xai {
            data["data"]
                .as_array()
                .and_then(|values| values.first())
                .or_else(|| data.get("image"))
                .or_else(|| data["output"].as_array().and_then(|values| values.first()))
                .or_else(|| data.get("output"))
                .unwrap_or(&Value::Null)
        } else {
            &data["data"][0]
        };
        if let Some(encoded) = value["b64_json"]
            .as_str()
            .or_else(|| {
                if xai {
                    value["base64"]
                        .as_str()
                        .or_else(|| value["image_base64"].as_str())
                } else {
                    None
                }
            })
            .filter(|value| !value.is_empty())
        {
            return decode(
                encoded,
                value["mime_type"]
                    .as_str()
                    .or_else(|| value["mimeType"].as_str())
                    .unwrap_or("image/png"),
            );
        }
        let url = value["url"]
            .as_str()
            .or_else(|| value["image_url"].as_str())
            .or_else(|| {
                if xai {
                    data["url"].as_str().or_else(|| data["image_url"].as_str())
                } else {
                    None
                }
            });
        if let Some(url) = url.filter(|value| !value.is_empty()) {
            return download(client, url, xai.then_some(model), cancellation).await;
        }
    } else {
        return Err(failure(
            "visual_driver_unsupported",
            "所选视觉协议不支持图像生成。",
        ));
    }
    Err(failure(
        "visual_response_invalid",
        "视觉模型没有返回图片数据。",
    ))
}
async fn detects_new_api(client: &Client, base: &str) -> bool {
    static DETECTED: OnceLock<
        tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::OnceCell<bool>>>>,
    > = OnceLock::new();
    let Ok(mut url) = reqwest::Url::parse(base) else {
        return false;
    };
    let key = url.origin().ascii_serialization().to_lowercase();
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    let cell = DETECTED
        .get_or_init(Default::default)
        .lock()
        .await
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
        .clone();
    *cell.get_or_init(||async{
        tokio::time::timeout(Duration::from_millis(3500),async{
            let result=client.get(url).header("accept","text/html,application/json").send().await.ok()?;
            if !result.status().is_success(){return None;}
            let bytes=bytes(result,1024*1024,&RunCancellation::default()).await.ok()?;
            let text=String::from_utf8_lossy(&bytes);
            static SIGNATURE:OnceLock<regex::Regex>=OnceLock::new();
            Some(SIGNATURE.get_or_init(||regex::Regex::new(r"(?i)(?:<title>\s*New API\s*</title>|Unified AI API gateway|QuantumNous)").expect("协议签名固定正则合法")).is_match(&text))
        }).await.ok().flatten().unwrap_or(false)
    }).await
}
fn save(request: &ImageGenerationRequest, buffer: &[u8], mime: &str) -> Result<GeneratedImage> {
    let cwd = request
        .cwd
        .canonicalize()
        .map_err(|_| failure("visual_output_invalid", "图片输出目录无效。"))?;
    media::directory(&cwd)?;
    let generated = cwd.join("generated");
    if !generated.exists() {
        media::create_private_directory(&generated)?;
    }
    media::directory(&generated)?;
    let directory = generated.join("visuals");
    if !directory.exists() {
        media::create_private_directory(&directory)?;
    }
    media::directory(&directory)?;
    let name = request
        .output_name
        .chars()
        .filter(|character| character.is_alphanumeric() || "._-".contains(*character))
        .take(80)
        .collect::<String>();
    let extension = match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    let path = directory.join(format!(
        "{}-{}.{}",
        crate::native_workflow::id()?,
        if name.is_empty() { "visual" } else { &name },
        extension
    ));
    media::write_private(&path, buffer)?;
    Ok(GeneratedImage {
        path,
        mime_type: mime.into(),
    })
}
pub(crate) async fn generate(
    state: &Arc<AppState>,
    request: ImageGenerationRequest,
    cancellation: Arc<RunCancellation>,
) -> Result<GeneratedImage> {
    generate_with(state.visual.clone(), request, cancellation).await
}

// 工作流、游戏和 Image Assets 共用通用服务，但已经计费的每个方向禁止
// SDK 重试、Images→Responses 重试和模型回退，保留原来一次提交的所有权。
async fn generate_with(
    visual: Arc<crate::native_visual::VisualGenerationService>,
    request: ImageGenerationRequest,
    cancellation: Arc<RunCancellation>,
) -> Result<GeneratedImage> {
    let token = tokio_util::sync::CancellationToken::new();
    if cancellation.is_cancelled() {
        token.cancel();
    }
    let bridged = token.clone();
    let bridge = tokio::spawn(async move {
        tokio::select! { _=cancellation.cancelled()=>bridged.cancel(), _=bridged.cancelled()=>{} }
    });
    struct Bridge(
        tokio::task::JoinHandle<()>,
        tokio_util::sync::CancellationToken,
    );
    impl Drop for Bridge {
        fn drop(&mut self) {
            self.1.cancel();
            self.0.abort();
        }
    }
    let _bridge = Bridge(bridge, token.clone());
    let model = if request.model.is_object() {
        let provider = request.model["provider"].as_str().unwrap_or("");
        let model = request.model["model"].as_str().unwrap_or("");
        if provider.is_empty() || model.is_empty() {
            Value::Null
        } else {
            json!(format!("{provider}/{model}"))
        }
    } else {
        request.model
    };
    let result = visual
        .generate(
            crate::native_visual::VisualRequest {
                cwd: request.cwd,
                input: json!({ "kind":"image", "model":model, "prompt":request.prompt,
            "sourceImages":request.source_images, "outputName":request.output_name,
            "aspectRatio":request.aspect_ratio }),
            },
            crate::native_visual::VisualOptions {
                cancellation: token,
                on_progress: None,
                allow_fallback: false,
            },
        )
        .await
        .map_err(visual_error)?;
    Ok(GeneratedImage {
        path: result.path,
        mime_type: result.mime_type,
    })
}

fn visual_error(error: crate::native_visual::VisualError) -> WorkflowError {
    if error.cancelled {
        WorkflowError::cancelled()
    } else {
        WorkflowError::coded("visual_generation_failed", error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Bytes, extract::State, routing::post, Json, Router};
    use tokio::sync::Mutex;
    fn image() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([11, 22, 33, 255]));
        let mut buffer = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        buffer.into_inner()
    }
    fn model(base: &str, driver: &str) -> VisualModel {
        VisualModel {
            public: json!({"id":"gpt-image-fixture","providerId":"synthetic","baseUrl":base,"driver":driver,"api":"openai-responses","name":"fixture"}),
            key: "synthetic-key".into(),
            headers: HeaderMap::new(),
            configured: true,
            visual: true,
            score: 0,
        }
    }
    async fn server(
        data: Value,
    ) -> (
        String,
        Arc<Mutex<Vec<(String, Vec<u8>)>>>,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = (requests.clone(), data);
        let app = Router::new()
            .fallback(post(
                |State((requests, data)): State<(Arc<Mutex<Vec<(String, Vec<u8>)>>>, Value)>,
                 uri: axum::http::Uri,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    if headers.contains_key("x-goog-api-key") {
                        assert_eq!(headers["x-goog-api-key"], "synthetic-key");
                    } else {
                        assert_eq!(headers["authorization"], "Bearer synthetic-key");
                    }
                    requests
                        .lock()
                        .await
                        .push((uri.path().to_owned(), body.to_vec()));
                    let status = data["_test_status"]
                        .as_u64()
                        .and_then(|status| axum::http::StatusCode::from_u16(status as u16).ok())
                        .unwrap_or(axum::http::StatusCode::OK);
                    (status, Json(data))
                },
            ))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        (url, requests, tx, job)
    }
    #[tokio::test]
    async fn shared_paid_adapter_keeps_object_selection_input_bytes_one_submission_and_cancel() {
        use crate::native_visual::{
            VisualConfigPort, VisualConfigSnapshot, VisualGenerationService,
        };
        let fixture = crate::native_workflow::test_support::TempDirectory::new();
        let make_service = |base: String| {
            let snapshot = VisualConfigSnapshot {
                models_json: json!({"providers":{"fixture":{"baseUrl":base,"models":[{
                    "id":"paid-image","kind":"image","visualApi":"openai-image"
                }]}}}),
                auth_json: json!({"fixture":"synthetic-key"}),
                ..Default::default()
            };
            VisualGenerationService::new(VisualConfigPort {
                read: Arc::new(move || {
                    let value = snapshot.clone();
                    Box::pin(async move { Ok(value) })
                }),
                write_preference: Arc::new(|_, _| Box::pin(async { Ok(()) })),
            })
        };
        let request =
            |cwd: std::path::PathBuf, sources: Vec<std::path::PathBuf>| ImageGenerationRequest {
                cwd,
                model: json!({"provider":"fixture","model":"paid-image"}),
                prompt: "paid adapter image".into(),
                source_images: sources,
                output_name: "paid-adapter".into(),
                aspect_ratio: "16:9".into(),
            };
        let (base, calls, stop, server_job) =
            server(json!({"data":[{"b64_json":STANDARD.encode(image())}]})).await;
        let service = make_service(base);
        let generated = generate_with(
            service.clone(),
            request(fixture.path.clone(), vec![]),
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(generated.path).unwrap(), image());
        assert_eq!(generated.mime_type, "image/png");
        let first = calls.lock().await.clone();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, "/images/generations");
        let body: Value = serde_json::from_slice(&first[0].1).unwrap();
        assert_eq!(body["model"], "paid-image");
        assert_eq!(body["size"], "1536x1024");
        let source = fixture.path.join("paid-source.png");
        std::fs::write(&source, image()).unwrap();
        let edited = generate_with(
            service.clone(),
            request(fixture.path.join("edit"), vec![source]),
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(edited.path).unwrap(), image());
        let observed = calls.lock().await.clone();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[1].0, "/images/edits");
        assert!(observed[1]
            .1
            .windows(image().len())
            .any(|bytes| bytes == image()));
        let cancellation = Arc::new(RunCancellation::default());
        cancellation.cancel();
        assert_eq!(
            generate_with(
                service.clone(),
                request(fixture.path.clone(), vec![]),
                cancellation
            )
            .await
            .err()
            .expect("Cancelled paid call must fail")
            .code,
            "WORKFLOW_CANCELLED"
        );
        assert_eq!(calls.lock().await.len(), 2);
        service.dispose().await;
        stop.send(()).unwrap();
        server_job.await.unwrap();
        let (base, calls, stop, server_job) =
            server(json!({"_test_status":503,"error":{"message":"synthetic unavailable"}})).await;
        let service = make_service(base);
        assert!(generate_with(
            service.clone(),
            request(fixture.path.clone(), vec![]),
            Arc::new(RunCancellation::default())
        )
        .await
        .is_err());
        assert_eq!(
            calls.lock().await.len(),
            1,
            "Paid adapter must disable SDK retry and every fallback"
        );
        service.dispose().await;
        stop.send(()).unwrap();
        server_job.await.unwrap();
    }
    #[tokio::test]
    async fn real_openai_generation_and_multipart_edit_preserve_one_paid_request() {
        let fixture = crate::native_workflow::test_support::TempDirectory::new();
        let (url, requests, shutdown, job) =
            server(json!({"data":[{"b64_json":STANDARD.encode(image())}]})).await;
        let model = model(&url, "openai-image");
        let request = ImageGenerationRequest {
            cwd: fixture.path.clone(),
            model: Value::Null,
            prompt: "sprite fixture".into(),
            source_images: vec![],
            output_name: "sprite".into(),
            aspect_ratio: "16:9".into(),
        };
        let result = drive(
            &Client::new(),
            &model,
            &request,
            &[],
            &RunCancellation::default(),
        )
        .await
        .unwrap();
        assert_eq!(result.0, image());
        let source = Source {
            path: fixture.path.join("source.png"),
            mime: "image/png".into(),
            buffer: image(),
        };
        let edited = drive(
            &Client::new(),
            &model,
            &request,
            &[source],
            &RunCancellation::default(),
        )
        .await
        .unwrap();
        assert_eq!(edited.0, image());
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0, "/images/generations");
        let body: Value = serde_json::from_slice(&requests[0].1).unwrap();
        assert_eq!(body["size"], "1536x1024");
        assert_eq!(body["output_format"], "png");
        assert_eq!(requests[1].0, "/images/edits");
        assert!(String::from_utf8_lossy(&requests[1].1)
            .contains("name=\"image\"; filename=\"source.png\""));
        drop(requests);
        let saved = save(&request, &result.0, &result.1).unwrap();
        assert!(saved.path.starts_with(
            fixture
                .path
                .canonicalize()
                .unwrap()
                .join("generated/visuals")
        ));
        assert_eq!(std::fs::read(saved.path).unwrap(), image());
        shutdown.send(()).unwrap();
        job.await.unwrap();
    }
    #[tokio::test]
    async fn real_openrouter_and_xai_image_protocols_return_pixels() {
        let fixture = crate::native_workflow::test_support::TempDirectory::new();
        let request = ImageGenerationRequest {
            cwd: fixture.path.clone(),
            model: Value::Null,
            prompt: "sprite".into(),
            source_images: vec![],
            output_name: "sprite".into(),
            aspect_ratio: "1:1".into(),
        };
        for (driver, data) in [
            (
                "openrouter-image",
                json!({"choices":[{"message":{"images":[{"image_url":{"url":format!("data:image/png;base64,{}",STANDARD.encode(image()))}}]}}]}),
            ),
            (
                "xai-image",
                json!({"image":{"base64":STANDARD.encode(image()),"mime_type":"image/png"}}),
            ),
        ] {
            let (url, requests, shutdown, job) = server(data).await;
            let result = drive(
                &Client::new(),
                &model(&url, driver),
                &request,
                &[],
                &RunCancellation::default(),
            )
            .await
            .unwrap();
            assert_eq!(result.0, image());
            let calls = requests.lock().await;
            assert_eq!(calls.len(), 1);
            let body: Value = serde_json::from_slice(&calls[0].1).unwrap();
            if driver == "openrouter-image" {
                assert_eq!(body["modalities"], json!(["image"]));
                assert_eq!(calls[0].0, "/chat/completions");
            } else {
                assert_eq!(body["response_format"], "b64_json");
                assert_eq!(body["aspect_ratio"], "1:1");
            }
            drop(calls);
            shutdown.send(()).unwrap();
            job.await.unwrap();
        }
    }
    #[tokio::test]
    async fn real_google_images_keep_source_parts_and_aspect_config() {
        let fixture = crate::native_workflow::test_support::TempDirectory::new();
        let(url,calls,shutdown,job)=server(json!({"candidates":[{"content":{"parts":[{"text":"generated"},{"inline_data":{"data":STANDARD.encode(image()),"mime_type":"image/png"}}]}}]})).await;
        let request = ImageGenerationRequest {
            cwd: fixture.path.clone(),
            model: Value::Null,
            prompt: "google sprite".into(),
            source_images: vec![],
            output_name: "sprite".into(),
            aspect_ratio: "16:9".into(),
        };
        let source = Source {
            path: fixture.path.join("source.png"),
            mime: "image/png".into(),
            buffer: image(),
        };
        let result = drive(
            &Client::new(),
            &model(&url, "google-image"),
            &request,
            &[source],
            &RunCancellation::default(),
        )
        .await
        .unwrap();
        assert_eq!(result.0, image());
        let calls = calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.starts_with("/models/gpt-image-fixture"));
        let body: Value = serde_json::from_slice(&calls[0].1).unwrap();
        assert_eq!(
            body["contents"][0]["parts"][0]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(body["contents"][0]["parts"][1]["text"], "google sprite");
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            json!(["TEXT", "IMAGE"])
        );
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            "16:9"
        );
        drop(calls);
        shutdown.send(()).unwrap();
        job.await.unwrap();
    }
    #[tokio::test]
    async fn paid_cancellation_prevents_submission_and_newapi_detection_is_cached() {
        let fixture = crate::native_workflow::test_support::TempDirectory::new();
        let request = ImageGenerationRequest {
            cwd: fixture.path.clone(),
            model: Value::Null,
            prompt: "cancelled".into(),
            source_images: vec![],
            output_name: "sprite".into(),
            aspect_ratio: "1:1".into(),
        };
        let (url, calls, shutdown, job) =
            server(json!({"data":[{"b64_json":STANDARD.encode(image())}]})).await;
        let cancellation = RunCancellation::default();
        cancellation.cancel();
        assert_eq!(
            drive(
                &Client::new(),
                &model(&url, "openai-image"),
                &request,
                &[],
                &cancellation
            )
            .await
            .unwrap_err()
            .code,
            "WORKFLOW_CANCELLED"
        );
        assert!(calls.lock().await.is_empty());
        shutdown.send(()).unwrap();
        job.await.unwrap();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let app = Router::new().route(
            "/",
            axum::routing::get(move || {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { "<title>New API</title>" }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        assert!(detects_new_api(&Client::new(), &url).await);
        assert!(detects_new_api(&Client::new(), &url).await);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        tx.send(()).unwrap();
        job.await.unwrap();
    }
    #[test]
    fn catalog_keeps_explicit_provider_credentials_private_and_honors_preference() {
        let models = json!({"providers":{"chat":{"baseUrl":"https://synthetic.invalid/v1","models":[{"id":"gpt-image-2","kind":"chat"}]},"a":{"baseUrl":"https://same.invalid/v1","models":[{"id":"picture","kind":"image"}]},"b":{"baseUrl":"https://same.invalid/v1","models":[{"id":"picture","kind":"image"}]},"c":{"baseUrl":"https://other.invalid/v1","models":[{"id":"gpt-image-2","kind":"chat","capabilities":["chat","image"]}]}}});
        let auth = json!({"chat":"synthetic","a":{"type":"api_key","key":"synthetic-a"},"b":"synthetic-b","c":"synthetic-c"});
        let app = json!({"providerTypes":{"a":"chat","b":"visual"}});
        let candidates = configured_models(&models, &auth, &app, &[]).unwrap();
        assert_eq!(candidates.len(), 3);
        let selected = select(
            candidates.clone(),
            "",
            &json!({"provider":"a","model":"picture"}),
        )
        .unwrap();
        assert_eq!(selected.key, "synthetic-a");
        let ordered = ordered(candidates, "b/picture");
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].reference(), "b/picture");
        assert!(!serde_json::to_string(&ordered[0].public)
            .unwrap()
            .contains("synthetic-b"));
    }
    #[test]
    fn invalid_remote_pixels_and_mime_mismatch_are_rejected() {
        assert!(decode("AA==", "image/png").is_err());
        assert!(decode(&STANDARD.encode(image()), "image/jpeg").is_err());
        assert!(inline("data:image/png;base64,AA==", "image/png").is_err());
    }
}
