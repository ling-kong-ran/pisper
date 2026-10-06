use super::{
    catalog::{js_string_checked, js_whitespace, truthy},
    transport::{self, Auth, Body, ErrorStyle, FormValue, RequestSpec},
    DriverResult, PreparedRequest, Result, VisualError, VisualKind, VisualModel, VisualOperation,
    VisualOptions,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell};

pub(super) type DetectionCache = Mutex<HashMap<String, Arc<OnceCell<bool>>>>;
fn progress(options: &VisualOptions, message: impl Into<String>) {
    if let Some(callback) = &options.on_progress {
        callback(message.into());
    }
}
fn optional(body: &mut Value, name: &str, input: &Value, input_name: &str) {
    if truthy(&input[input_name]) {
        body[name] = input[input_name].clone();
    }
}
fn output_mime(request: &PreparedRequest) -> &'static str {
    match request.input["outputFormat"].as_str() {
        Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}
fn image_extension(mime: &str) -> &'static str {
    if mime.contains("jpeg") {
        ".jpg"
    } else if mime.contains("webp") {
        ".webp"
    } else {
        ".png"
    }
}
fn xai_extension(mime: &str, fallback: &str) -> String {
    if mime.contains("jpeg") {
        ".jpg"
    } else if mime.contains("webp") {
        ".webp"
    } else if mime.contains("mp4") {
        ".mp4"
    } else if mime.contains("webm") {
        ".webm"
    } else {
        fallback
    }
    .into()
}
fn decode_base64(value: &str) -> Vec<u8> {
    // Node Buffer.from(base64) accepts both alphabets, missing padding and
    // ignored non-alphabet characters; decoding is not a raster validation.
    let mut encoded = value
        .chars()
        .take_while(|character| *character != '=')
        .filter_map(|character| match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(character),
            '-' => Some('+'),
            '_' => Some('/'),
            _ => None,
        })
        .collect::<String>();
    if encoded.len() % 4 == 1 {
        encoded.pop();
    }
    while encoded.len() % 4 != 0 {
        encoded.push('=');
    }
    STANDARD.decode(encoded).unwrap_or_default()
}
fn data_url(value: &Value) -> Result<Option<DriverResult>> {
    let value = if truthy(value) {
        js_string_checked(value)?
    } else {
        String::new()
    };
    static DATA: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let Some(capture) = DATA
        .get_or_init(|| regex::Regex::new(r"^data:([^;]+);base64,(.+)$").unwrap())
        .captures(&value)
    else {
        return Ok(None);
    };
    let mime = capture[1].to_owned();
    Ok(Some(DriverResult {
        bytes: decode_base64(&capture[2]),
        extension: image_extension(&mime).into(),
        mime_type: mime,
        remote_id: None,
    }))
}
fn spec<'a>(
    model: Option<&'a VisualModel>,
    method: Method,
    url: String,
    auth: Auth,
    body: Body,
    sdk: bool,
    video: bool,
    style: ErrorStyle,
) -> RequestSpec<'a> {
    RequestSpec {
        method,
        url,
        model,
        auth,
        body,
        sdk,
        video,
        style,
        extra_headers: Vec::new(),
    }
}
async fn image_download(
    client: &Client,
    model: Option<&VisualModel>,
    url: String,
    options: &VisualOptions,
    style: ErrorStyle,
) -> Result<DriverResult> {
    if url.starts_with("data:") {
        if let Some(value) = data_url(&json!(url))? {
            return Ok(value);
        }
    }
    let (bytes, mime) = transport::download(
        client,
        &spec(
            model,
            Method::GET,
            url,
            if model.is_some() {
                Auth::Bearer
            } else {
                Auth::None
            },
            Body::None,
            false,
            false,
            style,
        ),
        options,
    )
    .await?;
    Ok(DriverResult {
        bytes,
        extension: image_extension(&mime).into(),
        mime_type: mime,
        remote_id: None,
    })
}
pub(super) async fn detect_new_api(client: &Client, cache: &DetectionCache, base: &str) -> bool {
    let Ok(mut url) = reqwest::Url::parse(base) else {
        return false;
    };
    let key = url.origin().ascii_serialization().to_lowercase();
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    let cell = cache
        .lock()
        .await
        .entry(key)
        .or_insert_with(|| Arc::new(OnceCell::new()))
        .clone();
    *cell.get_or_init(||async {
        tokio::time::timeout(Duration::from_millis(3500),async {
            let options=VisualOptions::default();
            let mut request=spec(None,Method::GET,url.into(),Auth::None,Body::None,false,false,ErrorStyle::Download(""));
            request.extra_headers.push(("Accept".into(),"text/html,application/json".into()));
            let response=transport::request(client,&request,&options).await.ok()?;
            if !response.status().is_success(){return None;}
            let (bytes,_)=transport::bytes(response,&options,"application/octet-stream").await.ok()?;
            static SIGNATURE:std::sync::OnceLock<regex::Regex>=std::sync::OnceLock::new();
            Some(SIGNATURE.get_or_init(||regex::Regex::new(r"(?i)(?:<title>\s*New API\s*</title>|Unified AI API gateway|QuantumNous)").unwrap()).is_match(&String::from_utf8_lossy(&bytes)))
        }).await.ok().flatten().unwrap_or(false)
    }).await
}

pub(super) async fn run(
    client: &Client,
    cache: &DetectionCache,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let mut driver = model.string("driver").to_owned();
    if driver.starts_with("xai-") && detect_new_api(client, cache, model.string("baseUrl")).await {
        driver = format!("new-api-{}", request.kind.as_str());
    }
    if options.cancellation.is_cancelled() {
        return Err(VisualError::cancelled());
    }
    match driver.as_str() {
        "openai-image" | "openai-video" | "openrouter-image" => {
            if request.kind == VisualKind::Video {
                openai_video(client, model, request, options).await
            } else {
                openai_image(
                    client,
                    model,
                    request,
                    options,
                    model.string("driver") == "openrouter-image",
                )
                .await
            }
        }
        "new-api-image" | "new-api-video" => {
            if request.kind == VisualKind::Video {
                relay_video(client, model, request, options, true).await
            } else {
                openai_image(client, model, request, options, false).await
            }
        }
        "google-image" | "google-video" => {
            if request.kind == VisualKind::Video {
                google_video(client, model, request, options).await
            } else {
                google_image(client, model, request, options).await
            }
        }
        "xai-image" | "xai-video" => {
            if request.kind == VisualKind::Video {
                relay_video(client, model, request, options, false).await
            } else {
                xai_image(client, model, request, options).await
            }
        }
        _ => Err(VisualError::new(format!(
            "不支持的视觉接口驱动：{}",
            model.string("driver")
        ))),
    }
}
async fn openai_image(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
    router: bool,
) -> Result<DriverResult> {
    if router {
        return openrouter_image(client, model, request, options).await;
    }
    let root = model.string("baseUrl").trim_end_matches('/');
    let mut common = json!({"model":model.string("id"),"prompt":request.input["prompt"],"n":1});
    optional(&mut common, "size", &request.input, "size");
    optional(&mut common, "quality", &request.input, "quality");
    static FORMAT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if FORMAT
        .get_or_init(|| regex::Regex::new(r"(?i)gpt.*image|dall-e").unwrap())
        .is_match(model.string("id"))
    {
        common["output_format"] = if truthy(&request.input["outputFormat"]) {
            request.input["outputFormat"].clone()
        } else {
            json!("png")
        };
    }
    let editing = request.operation == VisualOperation::Edit;
    let body = if editing {
        let mut values = Vec::new();
        for (name, value) in common.as_object().unwrap() {
            values.push(FormValue::Text(name.clone(), js_string_checked(value)?));
        }
        for image in &request.sources {
            values.push(FormValue::File {
                name: if request.sources.len() == 1 {
                    "image"
                } else {
                    "image[]"
                }
                .into(),
                filename: image
                    .path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                mime: image.mime_type.clone(),
                bytes: image.bytes.clone(),
            });
        }
        if let Some(mask) = &request.mask {
            values.push(FormValue::File {
                name: "mask".into(),
                filename: mask
                    .path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                mime: mask.mime_type.clone(),
                bytes: mask.bytes.clone(),
            });
        }
        Body::Multipart(values)
    } else {
        Body::Json(common)
    };
    let result = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!(
                "{root}/images/{}",
                if editing { "edits" } else { "generations" }
            ),
            Auth::Bearer,
            body,
            true,
            false,
            ErrorStyle::Sdk,
        ),
        options,
    )
    .await;
    let data = match result {
        Ok(data) => data,
        Err(error) => {
            if !options.allow_fallback
                || editing
                || !model.string("api").eq_ignore_ascii_case("openai-responses")
                || !error
                    .status
                    .is_some_and(|status| matches!(status, 404 | 405 | 501 | 502))
            {
                return Err(error);
            }
            progress(
                options,
                "Images 接口不可用，正在通过 Responses 接口生成图片…",
            );
            return responses_image(client, model, request, options).await;
        }
    };
    let image = &data["data"][0];
    if truthy(&image["b64_json"]) {
        let mime = output_mime(request);
        return Ok(DriverResult {
            bytes: decode_base64(&js_string_checked(&image["b64_json"])?),
            mime_type: mime.into(),
            extension: image_extension(mime).into(),
            remote_id: None,
        });
    }
    if truthy(&image["url"]) {
        return image_download(
            client,
            None,
            js_string_checked(&image["url"])?,
            options,
            ErrorStyle::Download("生成结果"),
        )
        .await;
    }
    Err(VisualError::new("视觉模型没有返回图片数据。"))
}
async fn responses_image(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let root = model.string("baseUrl").trim_end_matches('/');
    let response = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!("{root}/responses"),
            Auth::Bearer,
            Body::Json(json!({"model":model.string("id"),"input":request.input["prompt"]})),
            true,
            false,
            ErrorStyle::Sdk,
        ),
        options,
    )
    .await?;
    let image = response["output"]
        .as_array()
        .and_then(|values| {
            values
                .iter()
                .find(|value| value["type"] == "image_generation_call")
        })
        .unwrap_or(&Value::Null);
    let remote_id = response["id"].as_str().map(str::to_owned);
    if let Some(mut result) = data_url(&image["result"])? {
        result.remote_id = remote_id;
        return Ok(result);
    }
    if let Some(value) = image["result"]
        .as_str()
        .filter(|value| !value.trim_matches(js_whitespace).is_empty())
    {
        let mime = output_mime(request);
        return Ok(DriverResult {
            bytes: decode_base64(value),
            mime_type: mime.into(),
            extension: image_extension(mime).into(),
            remote_id,
        });
    }
    let url = if truthy(&image["image_url"]["url"]) {
        &image["image_url"]["url"]
    } else {
        &image["image_url"]
    };
    if let Some(url) = url.as_str().filter(|value| !value.is_empty()) {
        let mut result = image_download(
            client,
            None,
            url.into(),
            options,
            ErrorStyle::Download("生成结果"),
        )
        .await?;
        result.remote_id = remote_id;
        return Ok(result);
    }
    Err(VisualError::new("Responses 视觉模型没有返回图片数据。"))
}
async fn openrouter_image(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let mut content = vec![json!({"type":"text","text":request.input["prompt"]})];
    content.extend(request.sources.iter().map(|image|json!({"type":"image_url","image_url":{"url":format!("data:{};base64,{}",image.mime_type,STANDARD.encode(&image.bytes))}})));
    let response=transport::json_request(client,&spec(Some(model),Method::POST,format!("{}/chat/completions",model.string("baseUrl").trim_end_matches('/')),Auth::Bearer,Body::Json(json!({"model":model.string("id"),"messages":[{"role":"user","content":content}],"modalities":["image"],"stream":false})),true,false,ErrorStyle::Sdk),options).await?;
    let image = &response["choices"][0]["message"]["images"][0]["image_url"];
    let url = if image.is_string() {
        image
    } else {
        &image["url"]
    };
    if let Some(result) = data_url(url)? {
        return Ok(result);
    }
    if truthy(url) {
        return image_download(
            client,
            None,
            js_string_checked(url)?,
            options,
            ErrorStyle::Download("生成结果"),
        )
        .await;
    }
    Err(VisualError::new("视觉模型没有返回图片数据。"))
}
async fn openai_video(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let root = model.string("baseUrl").trim_end_matches('/');
    let mut fields = vec![
        FormValue::Text("model".into(), model.string("id").into()),
        FormValue::Text(
            "prompt".into(),
            js_string_checked(&request.input["prompt"])?,
        ),
    ];
    if truthy(&request.input["durationSeconds"]) {
        fields.push(FormValue::Text(
            "seconds".into(),
            js_string_checked(&request.input["durationSeconds"])?,
        ));
    }
    if truthy(&request.input["size"]) {
        fields.push(FormValue::Text(
            "size".into(),
            js_string_checked(&request.input["size"])?,
        ));
    }
    let mut video = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!("{root}/videos"),
            Auth::Bearer,
            Body::Multipart(fields),
            true,
            true,
            ErrorStyle::Sdk,
        ),
        options,
    )
    .await?;
    loop {
        let status = video["status"].as_str().unwrap_or("");
        if matches!(status, "completed" | "failed") {
            break;
        }
        let progress_value = video["progress"].as_f64().unwrap_or(0.0).round();
        progress(options, format!("视频生成中：{progress_value:.0}%"));
        transport::wait(Duration::from_secs(5), options).await?;
        let id = js_string_checked(&video["id"])?;
        video = transport::json_request(
            client,
            &spec(
                Some(model),
                Method::GET,
                format!("{root}/videos/{}", encode_component(&id)),
                Auth::Bearer,
                Body::None,
                true,
                true,
                ErrorStyle::Sdk,
            ),
            options,
        )
        .await?;
    }
    if video["status"] == "failed" {
        return Err(VisualError::new(if truthy(&video["error"]["message"]) {
            js_string_checked(&video["error"]["message"])?
        } else {
            "视频生成失败。".into()
        }));
    }
    progress(options, "视频已生成，正在下载…");
    let id = js_string_checked(&video["id"])?;
    let mut request = spec(
        Some(model),
        Method::GET,
        format!(
            "{root}/videos/{}/content?variant=video",
            encode_component(&id)
        ),
        Auth::Bearer,
        Body::None,
        true,
        true,
        ErrorStyle::Download("生成结果"),
    );
    request
        .extra_headers
        .push(("Accept".into(), "application/binary".into()));
    let (bytes, mime) = transport::download(client, &request, options).await?;
    Ok(DriverResult {
        bytes,
        mime_type: mime,
        extension: ".mp4".into(),
        remote_id: Some(id),
    })
}
fn encode_component(value: &str) -> String {
    let mut result = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(byte) {
            result.push(*byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
async fn google_image(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let id = model
        .string("id")
        .strip_prefix("models/")
        .unwrap_or(model.string("id"));
    let mut parts=request.sources.iter().map(|image|json!({"inlineData":{"mimeType":image.mime_type,"data":STANDARD.encode(&image.bytes)}})).collect::<Vec<_>>();
    parts.push(json!({"text":request.input["prompt"]}));
    let mut config = json!({"responseModalities":["TEXT","IMAGE"]});
    let mut image = json!({});
    optional(&mut image, "aspectRatio", &request.input, "aspectRatio");
    optional(&mut image, "imageSize", &request.input, "imageSize");
    if !image.as_object().unwrap().is_empty() {
        config["imageConfig"] = image;
    }
    let data = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!(
                "{}/models/{}:generateContent",
                model.string("baseUrl"),
                encode_component(id)
            ),
            Auth::Google,
            Body::Json(
                json!({"contents":[{"role":"user","parts":parts}],"generationConfig":config}),
            ),
            false,
            false,
            ErrorStyle::Google,
        ),
        options,
    )
    .await?;
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
        let inline = if truthy(&part["inlineData"]) {
            &part["inlineData"]
        } else {
            &part["inline_data"]
        };
        if truthy(&inline["data"]) {
            let mime = if truthy(&inline["mimeType"]) {
                js_string_checked(&inline["mimeType"])?
            } else if truthy(&inline["mime_type"]) {
                js_string_checked(&inline["mime_type"])?
            } else {
                "image/png".into()
            };
            return Ok(DriverResult {
                bytes: decode_base64(&js_string_checked(&inline["data"])?),
                extension: image_extension(&mime).into(),
                mime_type: mime,
                remote_id: None,
            });
        }
    }
    Err(VisualError::new("Gemini 视觉模型没有返回图片数据。"))
}
fn google_video_uri(value: &Value) -> Option<String> {
    if let Some(value) = value["uri"].as_str() {
        static URI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        if URI
            .get_or_init(|| regex::Regex::new(r"(?i)video|files|download|googleapis").unwrap())
            .is_match(value)
        {
            return Some(value.into());
        }
    }
    match value {
        Value::Array(values) => values.iter().find_map(google_video_uri),
        Value::Object(values) => values.values().find_map(google_video_uri),
        _ => None,
    }
}
async fn google_video(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let id = model
        .string("id")
        .strip_prefix("models/")
        .unwrap_or(model.string("id"));
    let mut parameters = json!({"sampleCount":1});
    for name in ["aspectRatio", "durationSeconds", "resolution"] {
        optional(&mut parameters, name, &request.input, name);
    }
    let mut operation = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!(
                "{}/models/{}:predictLongRunning",
                model.string("baseUrl"),
                encode_component(id)
            ),
            Auth::Google,
            Body::Json(
                json!({"instances":[{"prompt":request.input["prompt"]}],"parameters":parameters}),
            ),
            false,
            true,
            ErrorStyle::Google,
        ),
        options,
    )
    .await?;
    if !truthy(&operation["name"]) {
        return Err(VisualError::new("Google 视频接口没有返回任务 ID。"));
    }
    while !truthy(&operation["done"]) {
        progress(options, "视频生成中，等待 Google Veo 完成…");
        transport::wait(Duration::from_secs(5), options).await?;
        let name = js_string_checked(&operation["name"])?;
        operation = transport::json_request(
            client,
            &spec(
                Some(model),
                Method::GET,
                format!(
                    "{}/{}",
                    model.string("baseUrl").trim_end_matches('/'),
                    name.trim_start_matches('/')
                ),
                Auth::Google,
                Body::None,
                false,
                true,
                ErrorStyle::Google,
            ),
            options,
        )
        .await?;
    }
    if truthy(&operation["error"]) {
        return Err(VisualError::new(
            if truthy(&operation["error"]["message"]) {
                js_string_checked(&operation["error"]["message"])?
            } else {
                "Google 视频生成失败。".into()
            },
        ));
    }
    let uri = google_video_uri(&operation["response"])
        .ok_or_else(|| VisualError::new("Google 视频任务完成，但没有返回可下载文件。"))?;
    progress(options, "视频已生成，正在下载…");
    let (bytes, mime) = transport::download(
        client,
        &spec(
            Some(model),
            Method::GET,
            uri,
            Auth::Google,
            Body::None,
            false,
            true,
            ErrorStyle::Download(" Google 视频"),
        ),
        options,
    )
    .await?;
    Ok(DriverResult {
        bytes,
        mime_type: mime,
        extension: ".mp4".into(),
        remote_id: operation["name"].as_str().map(str::to_owned),
    })
}
fn xai_result(data: &Value) -> Result<Option<DriverResult>> {
    let value = [
        &data["data"][0],
        &data["image"],
        &data["output"][0],
        &data["output"],
    ]
    .into_iter()
    .find(|value| truthy(value))
    .unwrap_or(&Value::Null);
    let encoded = [&value["b64_json"], &value["base64"], &value["image_base64"]]
        .into_iter()
        .find(|value| truthy(value));
    if let Some(encoded) = encoded {
        let mime = [&value["mime_type"], &value["mimeType"]]
            .into_iter()
            .find(|value| truthy(value))
            .map(js_string_checked)
            .transpose()?
            .unwrap_or_else(|| "image/png".into());
        return Ok(Some(DriverResult {
            bytes: decode_base64(&js_string_checked(encoded)?),
            extension: xai_extension(&mime, ".png"),
            mime_type: mime,
            remote_id: None,
        }));
    }
    Ok(None)
}
fn xai_image_url(data: &Value) -> Result<Option<String>> {
    let value = [
        &data["data"][0],
        &data["image"],
        &data["output"][0],
        &data["output"],
    ]
    .into_iter()
    .find(|value| truthy(value))
    .unwrap_or(&Value::Null);
    [
        &value["url"],
        &value["image_url"],
        &data["url"],
        &data["image_url"],
    ]
    .into_iter()
    .find(|value| truthy(value))
    .map(js_string_checked)
    .transpose()
}
async fn xai_image(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
) -> Result<DriverResult> {
    let root = model.string("baseUrl").trim_end_matches('/');
    let editing = request.operation == VisualOperation::Edit;
    let body = if editing {
        let mut fields = vec![
            FormValue::Text("model".into(), model.string("id").into()),
            FormValue::Text(
                "prompt".into(),
                js_string_checked(&request.input["prompt"])?,
            ),
        ];
        for (index, image) in request.sources.iter().enumerate() {
            fields.push(FormValue::File {
                name: "image".into(),
                filename: format!("image-{}", index + 1),
                mime: image.mime_type.clone(),
                bytes: image.bytes.clone(),
            });
        }
        if let Some(mask) = &request.mask {
            fields.push(FormValue::File {
                name: "mask".into(),
                filename: "mask.png".into(),
                mime: mask.mime_type.clone(),
                bytes: mask.bytes.clone(),
            });
        }
        for name in ["size", "quality"] {
            if truthy(&request.input[name]) {
                fields.push(FormValue::Text(
                    name.into(),
                    js_string_checked(&request.input[name])?,
                ));
            }
        }
        Body::Multipart(fields)
    } else {
        let mut body = json!({"model":model.string("id"),"prompt":request.input["prompt"],"n":1,"response_format":"b64_json"});
        optional(&mut body, "aspect_ratio", &request.input, "aspectRatio");
        optional(&mut body, "output_format", &request.input, "outputFormat");
        Body::Json(body)
    };
    let data = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!(
                "{root}/images/{}",
                if editing { "edits" } else { "generations" }
            ),
            Auth::Bearer,
            body,
            false,
            false,
            if editing {
                ErrorStyle::XaiEdit
            } else {
                ErrorStyle::Xai
            },
        ),
        options,
    )
    .await?;
    if let Some(result) = xai_result(&data)? {
        return Ok(result);
    }
    if let Some(url) = xai_image_url(&data)? {
        let mut result = image_download(
            client,
            Some(model),
            url,
            options,
            ErrorStyle::Download(" xAI 视觉结果"),
        )
        .await?;
        result.extension = xai_extension(&result.mime_type, ".png");
        return Ok(result);
    }
    Err(VisualError::new(if editing {
        "xAI 图片编辑接口没有返回图片数据。"
    } else {
        "xAI 图片生成接口没有返回图片数据。"
    }))
}
fn relay_url(value: &Value) -> Option<String> {
    for key in ["url", "video_url", "download_url"] {
        if let Some(value) = value[key].as_str().filter(|value| {
            value.to_ascii_lowercase().starts_with("http:")
                || value.to_ascii_lowercase().starts_with("https:")
        }) {
            return Some(value.into());
        }
    }
    match value {
        Value::Array(values) => values.iter().find_map(relay_url),
        Value::Object(values) => values.values().find_map(relay_url),
        _ => None,
    }
}
fn relay_id(value: &Value) -> Result<Option<String>> {
    [
        &value["task_id"],
        &value["request_id"],
        &value["id"],
        &value["data"]["task_id"],
        &value["data"]["id"],
    ]
    .into_iter()
    .find(|value| truthy(value))
    .map(js_string_checked)
    .transpose()
}
fn relay_status(value: &Value) -> Result<String> {
    [&value["status"], &value["state"], &value["data"]["status"]]
        .into_iter()
        .find(|value| truthy(value))
        .map(js_string_checked)
        .transpose()
        .map(|value| value.unwrap_or_default().to_lowercase())
}
async fn relay_video(
    client: &Client,
    model: &VisualModel,
    request: &PreparedRequest,
    options: &VisualOptions,
    new_api: bool,
) -> Result<DriverResult> {
    let root = model.string("baseUrl").trim_end_matches('/');
    let size = if truthy(&request.input["size"]) {
        js_string_checked(&request.input["size"])?
    } else if request.input["aspectRatio"] == "9:16" {
        "720x1280".into()
    } else {
        "1280x720".into()
    };
    let style = if new_api {
        ErrorStyle::NewApi
    } else {
        ErrorStyle::Xai
    };
    let mut body = json!({"model":model.string("id"),"prompt":request.input["prompt"]});
    if new_api {
        if truthy(&request.input["durationSeconds"]) {
            body["seconds"] = json!(js_string_checked(&request.input["durationSeconds"])?);
        }
        body["size"] = json!(size);
    } else {
        optional(&mut body, "duration", &request.input, "durationSeconds");
        optional(&mut body, "aspect_ratio", &request.input, "aspectRatio");
        optional(&mut body, "resolution", &request.input, "resolution");
    }
    let mut route = if new_api {
        "videos"
    } else {
        "videos/generations"
    };
    let first = transport::json_request(
        client,
        &spec(
            Some(model),
            Method::POST,
            format!("{root}/{route}"),
            Auth::Bearer,
            Body::Json(body),
            false,
            true,
            style,
        ),
        options,
    )
    .await;
    let mut task = match first {
        Ok(task) => task,
        Err(error) => {
            if !error
                .status
                .is_some_and(|status| matches!(status, 404 | 405))
            {
                return Err(error);
            }
            route = if new_api {
                "video/generations"
            } else {
                "videos"
            };
            let mut body = json!({"model":model.string("id"),"prompt":request.input["prompt"]});
            if new_api {
                optional(&mut body, "duration", &request.input, "durationSeconds");
                let dimensions = size
                    .split('x')
                    .filter_map(|value| value.parse::<f64>().ok())
                    .collect::<Vec<_>>();
                if dimensions.len() == 2 && dimensions.iter().all(|value| *value != 0.0) {
                    body["width"] = json!(dimensions[0]);
                    body["height"] = json!(dimensions[1]);
                }
                body["response_format"] = json!("url");
            } else {
                if truthy(&request.input["durationSeconds"]) {
                    body["seconds"] = json!(js_string_checked(&request.input["durationSeconds"])?);
                }
                body["size"] = json!(size);
            }
            transport::json_request(
                client,
                &spec(
                    Some(model),
                    Method::POST,
                    format!("{root}/{route}"),
                    Auth::Bearer,
                    Body::Json(body),
                    false,
                    true,
                    style,
                ),
                options,
            )
            .await?
        }
    };
    let id = relay_id(&task)?;
    let mut url = relay_url(&task);
    while url.is_none() && id.is_some() {
        let status = relay_status(&task)?;
        if matches!(
            status.as_str(),
            "failed" | "error" | "cancelled" | "canceled"
        ) {
            return if new_api {
                Err(transport::new_api_error(
                    &json!({"error":task["error"]}),
                    502,
                )?)
            } else {
                Err(VisualError::new(if truthy(&task["error"]["message"]) {
                    js_string_checked(&task["error"]["message"])?
                } else if truthy(&task["error"]) {
                    js_string_checked(&task["error"])?
                } else {
                    "xAI 视频生成失败。".into()
                }))
            };
        }
        if matches!(status.as_str(), "completed" | "succeeded" | "done") {
            break;
        }
        progress(
            options,
            format!(
                "视频生成中{}…",
                if status.is_empty() {
                    String::new()
                } else {
                    format!("：{status}")
                }
            ),
        );
        transport::wait(Duration::from_secs(5), options).await?;
        let encoded = encode_component(id.as_ref().unwrap());
        let response = transport::json_request(
            client,
            &spec(
                Some(model),
                Method::GET,
                format!("{root}/{route}/{encoded}"),
                Auth::Bearer,
                Body::None,
                false,
                true,
                style,
            ),
            options,
        )
        .await;
        task = match response {
            Err(error)
                if !new_api
                    && route == "videos"
                    && error
                        .status
                        .is_some_and(|status| matches!(status, 404 | 405)) =>
            {
                transport::json_request(
                    client,
                    &spec(
                        Some(model),
                        Method::GET,
                        format!("{root}/videos/generations/{encoded}"),
                        Auth::Bearer,
                        Body::None,
                        false,
                        true,
                        style,
                    ),
                    options,
                )
                .await?
            }
            value => value?,
        };
        url = relay_url(&task);
    }
    let url = match url {
        Some(url) => url,
        None => {
            let id = id.as_ref().ok_or_else(|| {
                VisualError::new(if new_api {
                    "New API 视频接口没有返回任务 ID。"
                } else {
                    "xAI 视频接口没有返回任务 ID 或下载地址。"
                })
            })?;
            format!("{root}/videos/{}/content", encode_component(id))
        }
    };
    let (bytes, mime) = transport::download(
        client,
        &spec(
            Some(model),
            Method::GET,
            url,
            Auth::Bearer,
            Body::None,
            false,
            true,
            ErrorStyle::Download(if new_api {
                " New API 视频"
            } else {
                " xAI 视频"
            }),
        ),
        options,
    )
    .await?;
    let extension = if new_api {
        if mime.contains("webm") {
            ".webm".into()
        } else {
            ".mp4".into()
        }
    } else {
        xai_extension(&mime, ".mp4")
    };
    Ok(DriverResult {
        bytes,
        mime_type: mime,
        extension,
        remote_id: id,
    })
}
