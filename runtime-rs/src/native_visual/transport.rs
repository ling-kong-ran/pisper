use super::{
    catalog::{js_string_checked, js_whitespace, truthy},
    Result, VisualError, VisualModel, VisualOptions,
};
use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    multipart::{Form, Part},
    Client, Method, Response,
};
use serde_json::{json, Value};
use std::{io::Read, time::Duration};

#[derive(Clone)]
pub(super) enum FormValue {
    Text(String, String),
    File {
        name: String,
        filename: String,
        mime: String,
        bytes: Vec<u8>,
    },
}
#[derive(Clone)]
pub(super) enum Body {
    None,
    Json(Value),
    Multipart(Vec<FormValue>),
}
#[derive(Clone, Copy)]
pub(super) enum Auth {
    Bearer,
    Google,
    None,
}
#[derive(Clone, Copy)]
pub(super) enum ErrorStyle {
    Sdk,
    Google,
    Xai,
    XaiEdit,
    NewApi,
    Download(&'static str),
}
pub(super) struct RequestSpec<'a> {
    pub method: Method,
    pub url: String,
    pub model: Option<&'a VisualModel>,
    pub auth: Auth,
    pub body: Body,
    pub sdk: bool,
    pub video: bool,
    pub style: ErrorStyle,
    pub extra_headers: Vec<(String, String)>,
}
pub(super) fn client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::limited(20))
        .no_deflate()
        .build()
        .map_err(|_| VisualError::new("Connection error."))
}
fn header_value(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| VisualError::new(error.to_string()))?;
    let value =
        HeaderValue::from_str(value).map_err(|error| VisualError::new(error.to_string()))?;
    headers.insert(name, value);
    Ok(())
}
fn headers(spec: &RequestSpec<'_>) -> Result<HeaderMap> {
    if !spec.sdk {
        return fetch_headers(spec);
    }
    let mut result = HeaderMap::new();
    if spec.sdk {
        header_value(&mut result, "Accept", "application/json")?;
        header_value(&mut result, "User-Agent", "OpenAI/JS 6.49.0")?;
    }
    if matches!(spec.style, ErrorStyle::Google) {
        header_value(&mut result, "Content-Type", "application/json")?;
    }
    if let Some(model) = spec.model {
        let key = js_string_checked(&model.key)?;
        match spec.auth {
            Auth::Bearer => header_value(&mut result, "Authorization", &format!("Bearer {key}"))?,
            Auth::Google => header_value(&mut result, "x-goog-api-key", &key)?,
            Auth::None => {}
        }
        // Google video download supplies only the key, with no model headers.
        if !(matches!(spec.auth, Auth::Google) && matches!(spec.style, ErrorStyle::Download(_))) {
            for (name, value) in &model.headers {
                if spec.sdk && name.to_ascii_lowercase().starts_with("x-stainless-") {
                    continue;
                }
                if spec.sdk {
                    // SDK buildHeaders clears the preceding header before
                    // each object value, then appends array entries. Null
                    // explicitly removes it; an empty array changes nothing.
                    let values = value
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_else(|| std::slice::from_ref(value));
                    if !values.is_empty() {
                        result.remove(name);
                        let mut strings = Vec::new();
                        for value in values {
                            if value.is_null() {
                                strings.clear();
                            } else {
                                strings.push(
                                    js_string_checked(value)?
                                        .trim_matches([' ', '\t', '\r', '\n'])
                                        .to_owned(),
                                );
                            }
                        }
                        if !strings.is_empty() {
                            header_value(&mut result, name, &strings.join(", "))?;
                        }
                    }
                } else {
                    header_value(&mut result, name, &js_string_checked(value)?)?;
                }
            }
        }
    }
    if !matches!(spec.style, ErrorStyle::Google) && matches!(spec.body, Body::Json(_)) {
        header_value(&mut result, "Content-Type", "application/json")?;
    }
    for (name, value) in &spec.extra_headers {
        header_value(&mut result, name, value)?;
    }
    finish_headers(result, spec)
}
fn fetch_headers(spec: &RequestSpec<'_>) -> Result<HeaderMap> {
    // fetch receives a case-sensitive JS object. Spread overwrites exact
    // spellings; the final Headers constructor combines distinct spellings
    // of the same HTTP header, unlike SDK buildHeaders' replacement rules.
    let mut object = serde_json::Map::new();
    if matches!(spec.style, ErrorStyle::Google) {
        object.insert("Content-Type".into(), json!("application/json"));
    }
    if let Some(model) = spec.model {
        match spec.auth {
            Auth::Bearer => {
                object.insert(
                    "Authorization".into(),
                    json!(format!("Bearer {}", js_string_checked(&model.key)?)),
                );
            }
            Auth::Google => {
                object.insert("x-goog-api-key".into(), model.key.clone());
            }
            Auth::None => {}
        }
        if !(matches!(spec.auth, Auth::Google) && matches!(spec.style, ErrorStyle::Download(_))) {
            object.extend(model.headers.clone());
        }
    }
    if !matches!(spec.style, ErrorStyle::Google) && matches!(spec.body, Body::Json(_)) {
        object.insert("Content-Type".into(), json!("application/json"));
    }
    for (name, value) in &spec.extra_headers {
        object.insert(name.clone(), json!(value));
    }
    let mut result = HeaderMap::new();
    for (name, value) in object {
        let value = js_string_checked(&value)?
            .trim_matches([' ', '\t', '\r', '\n'])
            .to_owned();
        let combined = result
            .get(name.as_str())
            .and_then(|value: &HeaderValue| value.to_str().ok())
            .map(|previous| format!("{previous}, {value}"))
            .unwrap_or(value);
        header_value(&mut result, &name, &combined)?;
    }
    finish_headers(result, spec)
}
fn finish_headers(mut result: HeaderMap, spec: &RequestSpec<'_>) -> Result<HeaderMap> {
    let secure = spec.url.starts_with("https:");
    if !result.contains_key("accept-encoding") {
        header_value(
            &mut result,
            "Accept-Encoding",
            if secure {
                "br, gzip, deflate"
            } else {
                "gzip, deflate"
            },
        )?;
    }
    if !result.contains_key("accept-language") {
        header_value(&mut result, "Accept-Language", "*")?;
    }
    if !result.contains_key("sec-fetch-mode") {
        header_value(&mut result, "Sec-Fetch-Mode", "cors")?;
    }
    if !result.contains_key("user-agent") {
        header_value(&mut result, "User-Agent", "node")?;
    }
    if !result.contains_key("accept") {
        header_value(&mut result, "Accept", "*/*")?;
    }
    Ok(result)
}
fn build(client: &Client, spec: &RequestSpec<'_>) -> Result<reqwest::RequestBuilder> {
    let mut builder = client.request(spec.method.clone(), &spec.url);
    builder = match &spec.body {
        Body::None => builder,
        Body::Json(value) => builder.body(stringify_js(value)?),
        Body::Multipart(values) => {
            let mut form = Form::new();
            for value in values {
                form = match value {
                    FormValue::Text(name, value) => form.text(name.clone(), value.clone()),
                    FormValue::File {
                        name,
                        filename,
                        mime,
                        bytes,
                    } => form.part(
                        name.clone(),
                        Part::bytes(bytes.clone())
                            .file_name(filename.clone())
                            .mime_str(mime)
                            .map_err(|error| VisualError::new(error.to_string()))?,
                    ),
                };
            }
            builder.multipart(form)
        }
    };
    Ok(builder.headers(headers(spec)?))
}
fn stringify_js(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(_) => js_string_checked(value)?,
        Value::String(_) => value.to_string(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(stringify_js)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}:{}",
                    serde_json::to_string(key)
                        .map_err(|error| VisualError::new(error.to_string()))?,
                    stringify_js(value)?
                )))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
    })
}
fn retryable(status: u16, headers: &HeaderMap) -> bool {
    match headers
        .get("x-should-retry")
        .and_then(|value| value.to_str().ok())
    {
        Some("true") => true,
        Some("false") => false,
        _ => matches!(status, 408 | 409 | 429) || status >= 500,
    }
}
fn parse_float_prefix(value: &str) -> Option<f64> {
    static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    NUMBER
        .get_or_init(|| {
            regex::Regex::new(
                r"^\s*([+-]?(?:Infinity|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?))",
            )
            .unwrap()
        })
        .captures(value)
        .and_then(|value| value[1].parse().ok())
}
fn retry_delay(headers: Option<&HeaderMap>) -> Duration {
    let mut milliseconds = headers
        .and_then(|headers| headers.get("retry-after-ms"))
        .and_then(|value| value.to_str().ok())
        .and_then(parse_float_prefix);
    if milliseconds.is_none_or(|value| value == 0.0) {
        if let Some(value) = headers
            .and_then(|headers| headers.get("retry-after"))
            .and_then(|value| value.to_str().ok())
        {
            milliseconds = parse_float_prefix(value)
                .map(|value| value * 1000.0)
                .or_else(|| {
                    DateTime::parse_from_rfc2822(value).ok().map(|date| {
                        (date.timestamp_millis() - Utc::now().timestamp_millis()) as f64
                    })
                })
                .or_else(|| {
                    DateTime::parse_from_rfc3339(value).ok().map(|date| {
                        (date.timestamp_millis() - Utc::now().timestamp_millis()) as f64
                    })
                });
        }
    }
    let milliseconds = milliseconds.unwrap_or_else(|| {
        let mut random = [0; 8];
        let _ = getrandom::getrandom(&mut random);
        let unit = u64::from_ne_bytes(random) as f64 / u64::MAX as f64;
        500.0 * (1.0 - unit * 0.25)
    });
    Duration::from_millis(
        if !milliseconds.is_finite() || !(1.0..=2147483647.0).contains(&milliseconds) {
            1
        } else {
            milliseconds.trunc() as u64
        },
    )
}
pub(super) async fn wait(duration: Duration, options: &VisualOptions) -> Result<()> {
    tokio::select! {biased;_=options.cancellation.cancelled()=>Err(VisualError::cancelled()),_=tokio::time::sleep(duration)=>Ok(())}
}
pub(super) async fn request(
    client: &Client,
    spec: &RequestSpec<'_>,
    options: &VisualOptions,
) -> Result<Response> {
    let attempts = if spec.sdk && options.allow_fallback {
        2
    } else {
        1
    };
    for index in 0..attempts {
        if options.cancellation.is_cancelled() {
            return Err(VisualError::cancelled());
        }
        let request = build(client, spec)?;
        let send = async {
            if spec.sdk {
                tokio::time::timeout(
                    Duration::from_secs(if spec.video { 600 } else { 180 }),
                    request.send(),
                )
                .await
                .map_err(|_| VisualError::new("Request timed out."))?
                .map_err(|_| VisualError::new("Connection error."))
            } else {
                request
                    .send()
                    .await
                    .map_err(|_| VisualError::new("fetch failed"))
            }
        };
        let result = tokio::select! {biased;_=options.cancellation.cancelled()=>return Err(VisualError::cancelled()),value=send=>value};
        match result {
            Ok(response) => {
                if index + 1 < attempts && retryable(response.status().as_u16(), response.headers())
                {
                    let delay = retry_delay(Some(response.headers()));
                    drop(response);
                    wait(delay, options).await?;
                    continue;
                }
                return Ok(response);
            }
            Err(error) => {
                if index + 1 == attempts {
                    return Err(error);
                }
                wait(retry_delay(None), options).await?;
            }
        }
    }
    unreachable!("a request always has at least one attempt")
}
pub(super) async fn bytes(
    response: Response,
    options: &VisualOptions,
    default_mime: &str,
) -> Result<(Vec<u8>, String)> {
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(default_mime)
        .to_owned();
    let deflate = response
        .headers()
        .get("content-encoding")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("deflate"));
    let body = tokio::select! {biased;_=options.cancellation.cancelled()=>return Err(VisualError::cancelled()),value=response.bytes()=>value.map_err(|_|VisualError::new("terminated"))?};
    if !deflate {
        return Ok((body.to_vec(), mime));
    }
    let mut reader: Box<dyn Read + Send> = if body.first().is_some_and(|byte| byte & 15 == 8) {
        Box::new(flate2::read::ZlibDecoder::new(std::io::Cursor::new(body)))
    } else {
        Box::new(flate2::read::DeflateDecoder::new(std::io::Cursor::new(
            body,
        )))
    };
    let mut result = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        if options.cancellation.is_cancelled() {
            return Err(VisualError::cancelled());
        }
        let count = reader
            .read(&mut chunk)
            .map_err(|_| VisualError::new("terminated"))?;
        if count == 0 {
            break;
        }
        result.extend_from_slice(&chunk[..count]);
        tokio::task::yield_now().await;
    }
    Ok((result, mime))
}
fn error_message(data: &Value) -> Result<String> {
    let value = [&data["error"]["message"], &data["error"], &data["message"]]
        .into_iter()
        .find(|value| truthy(value));
    value
        .map(js_string_checked)
        .transpose()
        .map(|value| value.unwrap_or_default())
}
fn sdk_error(status: u16, text: &str) -> Result<VisualError> {
    // OpenAI 6.49 APIError.generate passes only response.error into
    // makeMessage; a successfully parsed truthy JSON response suppresses the
    // raw text, even when it has no error member.
    let parsed = serde_json::from_str::<Value>(text).ok();
    let message = match parsed.as_ref().filter(|value| truthy(value)) {
        Some(value) if truthy(&value["error"]["message"]) => match &value["error"]["message"] {
            Value::String(value) => value.clone(),
            value => stringify_js(value)?,
        },
        Some(value) if truthy(&value["error"]) => stringify_js(&value["error"])?,
        Some(_) => String::new(),
        None => text.into(),
    };
    Ok(VisualError::with_status(
        if message.is_empty() {
            format!("{status} status code (no body)")
        } else {
            format!("{status} {message}")
        },
        status,
    ))
}
pub(super) fn new_api_error(data: &Value, status: u16) -> Result<VisualError> {
    let value = [&data["error"]["message"], &data["error"], &data["message"]]
        .into_iter()
        .find(|value| truthy(value));
    let upstream = match value {
        Some(Value::String(value)) => value.trim_matches(js_whitespace).to_owned(),
        Some(Value::Object(value)) if value["message"].is_string() => value["message"]
            .as_str()
            .unwrap()
            .trim_matches(js_whitespace)
            .to_owned(),
        Some(value) => value.to_string(),
        None => String::new(),
    };
    static DUPLICATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let message = if DUPLICATE
        .get_or_init(|| regex::Regex::new(r#"(?i)duplicate field\s+[\x60'"]?duration"#).unwrap())
        .is_match(&upstream)
    {
        format!("New API 视频渠道转发失败：{upstream}。请检查中转站中该模型的渠道映射或协议适配。")
    } else if upstream.is_empty() {
        format!("New API 视觉接口请求失败 ({status})")
    } else {
        upstream
    };
    Ok(VisualError::new(message))
}
pub(super) async fn json_request(
    client: &Client,
    spec: &RequestSpec<'_>,
    options: &VisualOptions,
) -> Result<Value> {
    let response = request(client, spec, options).await?;
    let status = response.status().as_u16();
    let (body, _) = bytes(response, options, "application/octet-stream").await?;
    let text = String::from_utf8_lossy(&body);
    let parsed = serde_json::from_str::<Value>(&text);
    let success = (200..300).contains(&status);
    let data = if spec.sdk && success {
        parsed.map_err(|error| VisualError::new(error.to_string()))?
    } else {
        parsed.unwrap_or_else(|_| json!({}))
    };
    if success {
        return Ok(data);
    }
    let message = if spec.sdk {
        String::new()
    } else {
        error_message(&data)?
    };
    match spec.style {
        ErrorStyle::Sdk => Err(sdk_error(status, &text)?),
        ErrorStyle::Google => {
            let message = if truthy(&data["error"]["message"]) {
                js_string_checked(&data["error"]["message"])?
            } else {
                format!("Google 视觉接口请求失败 ({status})")
            };
            Err(VisualError::new(message))
        }
        ErrorStyle::Xai => Err(VisualError::with_status(
            if message.is_empty() {
                format!("xAI 视觉接口请求失败 ({status})")
            } else {
                message
            },
            status,
        )),
        ErrorStyle::XaiEdit => Err(VisualError::new(if message.is_empty() {
            format!("xAI 图片编辑失败 ({status})")
        } else {
            message
        })),
        ErrorStyle::NewApi => {
            let mut error = new_api_error(&data, status)?;
            error.status = Some(status);
            Err(error)
        }
        ErrorStyle::Download(name) => Err(VisualError::new(format!("下载{name}失败 ({status})"))),
    }
}
pub(super) async fn download(
    client: &Client,
    spec: &RequestSpec<'_>,
    options: &VisualOptions,
) -> Result<(Vec<u8>, String)> {
    let response = request(client, spec, options).await?;
    if !response.status().is_success() {
        if spec.sdk {
            let status = response.status().as_u16();
            let (body, _) = bytes(response, options, "application/octet-stream").await?;
            let text = String::from_utf8_lossy(&body);
            return Err(sdk_error(status, &text)?);
        }
        let name = match spec.style {
            ErrorStyle::Download(name) => name,
            _ => "生成结果",
        };
        return Err(VisualError::new(format!(
            "下载{name}失败 ({})",
            response.status().as_u16()
        )));
    }
    bytes(
        response,
        options,
        if spec.video {
            "video/mp4"
        } else {
            "application/octet-stream"
        },
    )
    .await
}
pub(super) fn redact_model_error(mut error: VisualError, model: &VisualModel) -> VisualError {
    if let Ok(key) = js_string_checked(&model.key) {
        if !key.is_empty() {
            error.message = error.message.replace(&key, "[REDACTED]");
        }
    }
    for value in model.headers.values() {
        let values = value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(value));
        for value in values {
            if let Ok(value) = js_string_checked(value) {
                if value.len() >= 8 {
                    error.message = error.message.replace(&value, "[REDACTED]");
                }
            }
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdk_status_error_uses_exact_error_body_and_empty_body_semantics() {
        for (text, expected) in [
            ("", "400 status code (no body)"),
            ("plain error", "400 plain error"),
            (r#"{"message":"top level"}"#, "400 status code (no body)"),
            (r#"{"error":"denied"}"#, r#"400 "denied""#),
            (
                r#"{"error":{"message":{"reason":"blocked"}}}"#,
                r#"400 {"reason":"blocked"}"#,
            ),
        ] {
            let error = sdk_error(400, text).unwrap();
            assert_eq!(error.message, expected);
            assert_eq!(error.status, Some(400));
        }
    }
}
