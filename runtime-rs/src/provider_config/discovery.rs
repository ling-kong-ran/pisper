//! 模型发现只在用户显式提交连接参数时访问 Provider；错误响应不回显远端凭据。
use super::*;
use std::{collections::BTreeSet, time::Duration};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn validated_url(value: &str) -> Result<reqwest::Url, ApiError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| ApiError::bad_request("Provider Base URL 无效。"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::bad_request(
            "Provider Base URL 仅支持不含登录凭据的 HTTP 或 HTTPS 地址。",
        ));
    }
    Ok(url)
}

fn candidates(base_url: &str, api: &str) -> Result<Vec<reqwest::Url>, ApiError> {
    let mut url = validated_url(base_url)?;
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/models") {
        return Ok(vec![url]);
    }
    if path.ends_with("/v1") || path.ends_with("/v1beta") || path.ends_with("/v1alpha") {
        url.set_path(&format!("{path}/models"));
        return Ok(vec![url]);
    }
    let mut direct = url.clone();
    direct.set_path(&format!("{path}/models"));
    url.set_path(&format!("{path}/v1/models"));
    Ok(if api == "anthropic-messages" {
        vec![url, direct]
    } else {
        vec![direct, url]
    })
}

fn credential(docs: &Documents, id: &str) -> Result<String, ApiError> {
    let auth = docs.auth.get(id).unwrap_or(&Value::Null);
    if string(auth, "type") == "oauth" {
        // 官方 OAuth token 的模型枚举接口因供应商而异，不能当兼容端点的 API Key 外发。
        return Err(ApiError::bad_request(
            "OAuth 连接请使用已有模型或手动添加模型 ID。",
        ));
    }
    if let Some(key) = auth
        .as_str()
        .or_else(|| auth.get("key").and_then(Value::as_str))
    {
        return Ok(key.to_string());
    }
    let reference = string(overlay(docs, id), "apiKey");
    if let Some(variable) = reference.strip_prefix('$') {
        return Ok(std::env::var(variable).unwrap_or_default());
    }
    if !reference.is_empty() {
        return Ok(reference.to_string());
    }
    Ok(String::new())
}

fn discovery_parameters(docs: &Documents, body: &Value) -> Result<Value, ApiError> {
    let id = string(body, "providerId");
    if !id.is_empty() {
        check_provider(docs, id)?;
    }
    let connection = overlay(docs, id);
    let api = body
        .get("api")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            let existing = string(connection, "api");
            if existing.is_empty() {
                "openai-responses"
            } else {
                existing
            }
        });
    if !PROTOCOLS.contains(&api) {
        return Err(ApiError::bad_request("当前 API 协议不支持自动获取模型。"));
    }
    let base = body
        .get("baseUrl")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or(string(connection, "baseUrl"));
    validated_url(base)?;
    let key = body
        .get("apiKey")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string())
        .map(Ok)
        .unwrap_or_else(|| credential(docs, id))?;
    let organization = body
        .get("organization")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            connection
                .get("headers")
                .and_then(|headers| headers.get("OpenAI-Organization"))
                .and_then(Value::as_str)
                .unwrap_or("")
        });
    Ok(
        json!({"api":api, "baseUrl":base, "apiKey":key, "organization":organization,
        "providerType": body.get("providerType").and_then(Value::as_str).unwrap_or("chat"), "headers":connection.get("headers").cloned().unwrap_or_else(|| json!({}))}),
    )
}

pub(super) async fn discover_connection(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let docs = state.providers.read()?;
    let parameters = discovery_parameters(&docs, &body)?;
    let models = discover(&parameters).await?;
    Ok(Json(
        json!({ "models":models, "scope":string(&parameters,"providerType") }),
    ))
}

pub(super) async fn discover_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    body["providerId"] = json!(id);
    discover_connection(State(state), Json(body)).await
}

pub(super) async fn refresh(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let _engine = state
        .engine_mutation
        .try_write()
        .map_err(|_| engine_busy())?;
    let _write = state.providers.write.lock().await;
    let docs = state.providers.read()?;
    // 后台加载只更新本地 Pi 目录。远端枚举由显式“拉取模型”请求承载，
    // 避免打开设置页就向每个旧连接发起昂贵或可能过期的认证请求。
    reload_engine(&state.runtime, &docs).await?;
    Ok(Json(
        json!({ "config":snapshot(&state,&docs).await?, "source":"local-engine-catalog" }),
    ))
}

async fn discover(parameters: &Value) -> Result<Vec<Value>, ApiError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ApiError::internal("无法初始化模型发现 HTTP 客户端。"))?;
    let api = string(parameters, "api");
    let key = string(parameters, "apiKey")
        .strip_prefix("Bearer ")
        .unwrap_or(string(parameters, "apiKey"));
    let mut final_not_found = None;
    for candidate in candidates(string(parameters, "baseUrl"), api)? {
        match discover_at(&client, candidate, api, key, parameters).await {
            Ok(models) => return Ok(models),
            Err(error) if error.status == StatusCode::NOT_FOUND => final_not_found = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(final_not_found.unwrap_or_else(|| ApiError::bad_request("Provider 未提供模型列表。")))
}

async fn discover_at(
    client: &reqwest::Client,
    mut url: reqwest::Url,
    api: &str,
    key: &str,
    parameters: &Value,
) -> Result<Vec<Value>, ApiError> {
    let mut models = BTreeMap::<String, Value>::new();
    let mut visited = BTreeSet::new();
    let mut total_bytes = 0_usize;
    for page in 0..50 {
        if !visited.insert(url.to_string()) {
            return Err(ApiError::bad_request("Provider 返回了重复的分页游标。"));
        }
        let mut request = client.get(url.clone()).header("Accept", "application/json");
        if let Some(headers) = parameters.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                if [
                    "host",
                    "content-length",
                    "authorization",
                    "x-api-key",
                    "x-goog-api-key",
                ]
                .contains(&name.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                if let Some(value) = value.as_str().filter(|v| !v.trim().is_empty()) {
                    request = request.header(name, value);
                }
            }
        }
        request = match api {
            "anthropic-messages" => {
                let request = request.header("anthropic-version", "2023-06-01");
                if key.is_empty() {
                    request
                } else {
                    request.header("x-api-key", key)
                }
            }
            "google-generative-ai" => {
                if key.is_empty() {
                    request
                } else {
                    request.header("x-goog-api-key", key)
                }
            }
            _ => {
                let mut request = if key.is_empty() {
                    request
                } else {
                    request.bearer_auth(key)
                };
                if !string(parameters, "organization").is_empty() {
                    request =
                        request.header("OpenAI-Organization", string(parameters, "organization"));
                }
                request
            }
        };
        let mut response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                ApiError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "provider_timeout",
                    "获取模型超时，请检查 Provider 地址或网络。",
                )
            } else {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "provider_unreachable",
                    "无法连接 Provider，请检查地址和网络。",
                )
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            // 不回显远端消息体：它可能包含 request headers / API Key。
            let mapped = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            return Err(ApiError::new(
                mapped,
                "provider_http_error",
                format!("获取模型失败 ({status})。"),
            ));
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes as usize > MAX_RESPONSE_BYTES.saturating_sub(total_bytes))
        {
            return Err(ApiError::bad_request("Provider 返回的模型列表过大。"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "provider_response_error",
                "读取 Provider 模型列表失败。",
            )
        })? {
            total_bytes += chunk.len();
            if total_bytes > MAX_RESPONSE_BYTES {
                return Err(ApiError::bad_request("Provider 返回的模型列表过大。"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "provider_invalid_json",
                "Provider 返回了无效的模型列表。",
            )
        })?;
        let items = payload
            .as_array()
            .or_else(|| payload.get("data").and_then(Value::as_array))
            .or_else(|| payload.get("models").and_then(Value::as_array));
        if let Some(items) = items {
            for item in items {
                if let Some(model) = candidate_model(item, api) {
                    models
                        .entry(string(&model, "id").to_string())
                        .or_insert(model);
                }
            }
        }
        let cursor = if api == "google-generative-ai" {
            payload
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(|value| ("pageToken", value))
        } else if payload.get("has_more").and_then(Value::as_bool) == Some(true) {
            payload
                .get("last_id")
                .and_then(Value::as_str)
                .map(|value| ("after_id", value))
        } else {
            None
        };
        if let Some((name, value)) = cursor {
            if page == 49 {
                return Err(ApiError::bad_request("Provider 返回的模型分页过多。"));
            }
            let mut next = url.clone();
            let previous: Vec<(String, String)> = next
                .query_pairs()
                .filter(|(key, _)| key != name)
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            next.query_pairs_mut()
                .clear()
                .extend_pairs(previous)
                .append_pair(name, value);
            url = next;
        } else {
            break;
        }
    }
    if models.is_empty() {
        return Err(ApiError::bad_request("Provider 没有返回可用的模型 ID。"));
    }
    Ok(models.into_values().collect())
}

fn candidate_model(item: &Value, api: &str) -> Option<Value> {
    let id = item.as_str().or_else(|| {
        ["id", "model_id", "model", "slug", "name"]
            .iter()
            .find_map(|field| item.get(*field).and_then(Value::as_str))
    })?;
    let id = if api == "google-generative-ai" {
        id.strip_prefix("models/").unwrap_or(id)
    } else {
        id
    }
    .trim();
    if id.is_empty() {
        return None;
    }
    let name = ["display_name", "displayName", "name"]
        .iter()
        .find_map(|field| item.get(*field).and_then(Value::as_str))
        .unwrap_or(id);
    let explicit_kind = item
        .get("kind")
        .and_then(Value::as_str)
        .filter(|value| ["chat", "image", "video"].contains(value));
    let lower = id.to_ascii_lowercase();
    let model_kind = explicit_kind.unwrap_or_else(|| {
        if ["sora", "veo", "kling", "seedance"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            "video"
        } else if [
            "dall-e",
            "gpt-image",
            "imagen",
            "flux",
            "stable-diffusion",
            "seedream",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        {
            "image"
        } else {
            "chat"
        }
    });
    let mut model = json!({"id":id,"name":name,"kind":model_kind,"capabilities":[model_kind],"input":["text"],"reasoning":item.get("reasoning").and_then(Value::as_bool).unwrap_or(false),"contextWindow":128000,"maxTokens":8192});
    for field in [
        "input",
        "reasoning",
        "contextWindow",
        "maxTokens",
        "thinkingLevels",
    ] {
        if let Some(value) = item.get(field) {
            model[field] = value.clone();
        }
    }
    Some(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_urls_preserve_version_paths_and_reject_embedded_credentials() {
        let urls = candidates("http://localhost:1234/v1/", "openai-responses").unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "http://localhost:1234/v1/models");
        assert_eq!(
            candidates("https://example.test", "anthropic-messages").unwrap()[0].path(),
            "/v1/models"
        );
        assert_eq!(
            candidates("https://example.test/v1beta", "google-generative-ai").unwrap()[0].path(),
            "/v1beta/models"
        );
        assert!(validated_url("https://secret@example.test").is_err());
    }

    #[tokio::test]
    async fn actual_http_discovery_passes_auth_and_handles_pagination() {
        use axum::{extract::Query, http::HeaderMap};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/v1/models",
            get(
                |headers: HeaderMap, Query(query): Query<BTreeMap<String, String>>| async move {
                    assert_eq!(
                        headers["authorization"],
                        "Bearer fictional-discovery-test-key"
                    );
                    if query.contains_key("after_id") {
                        Json(json!({"data":[{"id":"chat-b"},{"id":"chat-a"}]}))
                    } else {
                        Json(json!({"data":[{"id":"chat-a"}],"has_more":true,"last_id":"chat-a"}))
                    }
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let result=discover(&json!({"api":"openai-responses","baseUrl":format!("http://{addr}/v1"),"apiKey":"fictional-discovery-test-key"})).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["id"], "chat-a");
        assert_eq!(result[1]["id"], "chat-b");
        server.abort();
    }

    #[tokio::test]
    async fn actual_http_authentication_error_never_echoes_remote_secrets() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/models",
            get(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":{"message":"fictional-secret-echo"}})),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error=discover(&json!({"api":"openai-completions","baseUrl":format!("http://{addr}"),"apiKey":"fictional-discovery-test-key"})).await.unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(!error.message.contains("fictional-secret"));
        server.abort();
    }
}
