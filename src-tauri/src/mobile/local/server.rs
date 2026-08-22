//! 本机 Runtime 回环服务：hyper HTTP/1.1，仅绑 127.0.0.1 随机端口。
//! 路由分三类：内置对话页（GET /）、JSON 管理 API（/api/local/*）、
//! 流式对话（POST /api/local/chat，SSE 输出）。
//!
//! 与 remote 代理同模式：监听器留在创建它的 Tokio 运行时上驱动，
//! 响应体一律流式写出，不做整体缓冲。
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpListener;

use super::provider;
use super::store::{
    LocalStore, MAX_MESSAGES_PER_SESSION, MAX_MESSAGE_BYTES, MAX_SESSIONS, MAX_TOTAL_SESSION_BYTES,
};

/// API 请求体上限：聊天文本本身另有 32 KiB 上限，这里防异常大请求。
const MAX_API_BODY_BYTES: usize = 1024 * 1024;
/// 协议版本：内置页与壳据此判断能力集。
const PROTOCOL_VERSION: u32 = 1;

type ApiBody = UnsyncBoxBody<Bytes, Infallible>;

pub struct LocalRuntime {
    pub port: u16,
    store: Arc<Mutex<LocalStore>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitsDto {
    max_sessions: usize,
    max_messages_per_session: usize,
    max_message_bytes: usize,
    max_total_bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDto {
    ok: bool,
    runtime: &'static str,
    protocol: u32,
    active_provider: Option<super::store::RedactedProvider>,
    provider_count: usize,
    session_count: usize,
    limits: LimitsDto,
}

fn now_string() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

fn json_response(status: StatusCode, value: &impl Serialize) -> Response<ApiBody> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(BodyExt::boxed_unsync(Full::new(Bytes::from(body))))
        .expect("json response")
}

fn error_json(status: StatusCode, message: &str) -> Response<ApiBody> {
    json_response(status, &serde_json::json!({ "error": message }))
}

fn empty_response(status: StatusCode) -> Response<ApiBody> {
    Response::builder()
        .status(status)
        .body(BodyExt::boxed_unsync(Full::new(Bytes::new())))
        .expect("empty response")
}

/// 读请求体并限制大小；超限返回 None 由路由回 413。
async fn read_json_body(body: Incoming) -> Result<Option<serde_json::Value>, String> {
    let collected = body
        .collect()
        .await
        .map_err(|_| "读取请求体失败。".to_string())?;
    let bytes = collected.to_bytes();
    if bytes.len() > MAX_API_BODY_BYTES {
        return Ok(None);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "请求体不是有效 JSON。".to_string())
}

fn body_string<'a>(body: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(|value| value.as_str())
}

/// SSE 帧编码：event 名 + 单条 JSON data。
fn sse_frame(event: &str, data: &impl Serialize) -> Result<Frame<Bytes>, Infallible> {
    let payload = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
    Ok(Frame::data(Bytes::from(format!(
        "event: {event}\ndata: {payload}\n\n"
    ))))
}

/// 流式对话：先落用户消息与助手占位，再逐增量回写；失败时按是否有内容
/// 决定保留部分回复还是清掉占位。帧通过 unbounded 通道流入响应体。
async fn handle_chat(store: &Arc<Mutex<LocalStore>>, body: serde_json::Value) -> Response<ApiBody> {
    let session_id = body_string(&body, "sessionId")
        .unwrap_or_default()
        .to_string();
    let text = body_string(&body, "text").unwrap_or_default().to_string();
    if text.trim().is_empty() {
        return error_json(StatusCode::BAD_REQUEST, "消息内容不能为空。");
    }
    if text.len() > MAX_MESSAGE_BYTES {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, "消息过长（上限 32 KiB）。");
    }

    // 一次性完成前置写入：用户消息 + 空助手占位 + 快照历史与 Provider。
    let (history, profile, assistant_id) = {
        let Ok(mut guard) = store.lock() else {
            return error_json(StatusCode::INTERNAL_SERVER_ERROR, "本地存储不可用。");
        };
        if guard.session(&session_id).is_none() {
            return error_json(StatusCode::NOT_FOUND, "会话不存在。");
        }
        let Some(profile) = guard.active_provider() else {
            return error_json(StatusCode::CONFLICT, "尚未配置 Provider。");
        };
        let now = now_string();
        if let Err(error) = guard.append_message(&session_id, "user", text, now.clone()) {
            return error_json(StatusCode::BAD_REQUEST, &error);
        }
        let history = guard
            .session(&session_id)
            .map(|session| session.messages)
            .unwrap_or_default();
        let assistant = match guard.append_message(&session_id, "assistant", String::new(), now) {
            Ok(message) => message,
            Err(error) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, &error),
        };
        (history, profile, assistant.id)
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<Frame<Bytes>, Infallible>>();
    let _ = tx.send(sse_frame(
        "meta",
        &serde_json::json!({ "sessionId": session_id, "messageId": assistant_id }),
    ));

    let task_store = store.clone();
    let task_session = session_id.clone();
    let task_assistant = assistant_id.clone();
    tokio::spawn(async move {
        let deltas = std::sync::Mutex::new(String::new());
        let result = {
            let deltas_ref = &deltas;
            provider::stream_chat(&profile, &history, |text: &str| {
                if let Ok(mut buffer) = deltas_ref.lock() {
                    buffer.push_str(text);
                }
                let _ = tx.send(sse_frame("delta", &serde_json::json!({ "text": text })));
            })
            .await
        };
        let content = deltas
            .lock()
            .map(|buffer| buffer.clone())
            .unwrap_or_default();
        let now = now_string();
        match result {
            Ok(_) => {
                if let Ok(mut guard) = task_store.lock() {
                    let _ = guard.finalize_message(&task_session, &task_assistant, content, now);
                }
                let _ = tx.send(sse_frame(
                    "done",
                    &serde_json::json!({ "messageId": task_assistant }),
                ));
            }
            Err(error) => {
                if let Ok(mut guard) = task_store.lock() {
                    if content.is_empty() {
                        let _ = guard.remove_message(&task_session, &task_assistant);
                    } else {
                        let _ =
                            guard.finalize_message(&task_session, &task_assistant, content, now);
                    }
                }
                let _ = tx.send(sse_frame(
                    "error",
                    &serde_json::json!({ "message": error, "messageId": task_assistant }),
                ));
            }
        }
    });

    let stream = futures_util::stream::poll_fn(move |context| rx.poll_recv(context));
    let body = StreamBody::new(stream).boxed_unsync();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap_or_else(|_| error_json(StatusCode::INTERNAL_SERVER_ERROR, "构造响应失败。"))
}

async fn route(
    runtime: &Arc<LocalRuntime>,
    request: Request<Incoming>,
) -> Result<Response<ApiBody>, Infallible> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();

    // 内置页与静态健康检查不需要请求体。
    if method == Method::GET && path == "/" {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            // 单文件页内含内联脚本与样式；页面只与本服务同源通信。
            .header(
                "Content-Security-Policy",
                "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'",
            )
            .body(BodyExt::boxed_unsync(Full::new(Bytes::from(
                include_str!("ui.html"),
            ))))
            .expect("ui response"));
    }
    if method == Method::GET && path == "/api/health" {
        return Ok(json_response(
            StatusCode::OK,
            &serde_json::json!({ "ok": true, "runtime": "mobile-local", "protocol": PROTOCOL_VERSION }),
        ));
    }

    let respond = match (method.clone(), path.as_str()) {
        (Method::GET, "/api/local/state") => {
            let result = runtime.store.lock().map(|store| StateDto {
                ok: true,
                runtime: "mobile-local",
                protocol: PROTOCOL_VERSION,
                active_provider: store.active_provider().map(|p| p.redacted()),
                provider_count: store.providers().len(),
                session_count: store.session_summaries().len(),
                limits: LimitsDto {
                    max_sessions: MAX_SESSIONS,
                    max_messages_per_session: MAX_MESSAGES_PER_SESSION,
                    max_message_bytes: MAX_MESSAGE_BYTES,
                    max_total_bytes: MAX_TOTAL_SESSION_BYTES,
                },
            });
            match result {
                Ok(dto) => json_response(StatusCode::OK, &dto),
                Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "本地存储不可用。"),
            }
        }
        (Method::GET, "/api/local/providers") => {
            let result = runtime.store.lock().map(|store| {
                serde_json::json!({
                    "providers": store.providers(),
                    "activeId": store.active_provider_id(),
                })
            });
            match result {
                Ok(value) => json_response(StatusCode::OK, &value),
                Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "本地存储不可用。"),
            }
        }
        (Method::GET, "/api/local/sessions") => {
            let result = runtime
                .store
                .lock()
                .map(|store| serde_json::json!({ "sessions": store.session_summaries() }));
            match result {
                Ok(value) => json_response(StatusCode::OK, &value),
                Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "本地存储不可用。"),
            }
        }
        (Method::GET, "/api/local/sessions/get") => {
            let id = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("id="))
                .unwrap_or_default()
                .to_string();
            let result = runtime.store.lock().map(|store| store.session(&id));
            match result {
                Ok(Some(session)) => {
                    json_response(StatusCode::OK, &serde_json::json!({ "session": session }))
                }
                Ok(None) => error_json(StatusCode::NOT_FOUND, "会话不存在。"),
                Err(_) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "本地存储不可用。"),
            }
        }
        (Method::POST, _) => {
            let body = match read_json_body(request.into_body()).await {
                Ok(Some(body)) => body,
                Ok(None) => return Ok(error_json(StatusCode::PAYLOAD_TOO_LARGE, "请求体过大。")),
                Err(message) => return Ok(error_json(StatusCode::BAD_REQUEST, &message)),
            };
            match path.as_str() {
                "/api/local/providers/upsert" => {
                    // 直接透传 store 的策略错误（地址协议、空模型名等），不回显密钥。
                    let result = runtime
                        .store
                        .lock()
                        .map_err(|_| "本地存储不可用。".to_string())
                        .and_then(|mut store| {
                            store.upsert_provider(
                                body_string(&body, "id").map(str::to_string),
                                body_string(&body, "name").unwrap_or_default().to_string(),
                                body_string(&body, "baseUrl")
                                    .unwrap_or_default()
                                    .to_string(),
                                body_string(&body, "apiKey").map(str::to_string),
                                body_string(&body, "model").unwrap_or_default().to_string(),
                                now_string(),
                            )
                        });
                    match result {
                        Ok(provider) => json_response(
                            StatusCode::OK,
                            &serde_json::json!({ "provider": provider }),
                        ),
                        Err(message) => error_json(StatusCode::BAD_REQUEST, &message),
                    }
                }
                "/api/local/providers/select" => {
                    let id = body_string(&body, "id").unwrap_or_default();
                    let result = runtime
                        .store
                        .lock()
                        .map_err(|_| "本地存储不可用。".to_string())
                        .and_then(|mut store| store.select_provider(id));
                    match result {
                        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
                        Err(message) => error_json(StatusCode::BAD_REQUEST, &message),
                    }
                }
                "/api/local/providers/delete" => {
                    let id = body_string(&body, "id").unwrap_or_default();
                    let result = runtime
                        .store
                        .lock()
                        .map_err(|_| "本地存储不可用。".to_string())
                        .and_then(|mut store| store.delete_provider(id));
                    match result {
                        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
                        Err(message) => error_json(StatusCode::BAD_REQUEST, &message),
                    }
                }
                "/api/local/providers/test" => {
                    let id = body_string(&body, "id").unwrap_or_default();
                    let profile = runtime.store.lock().ok().and_then(|store| {
                        let _ = id;
                        store.active_provider()
                    });
                    let Some(profile) = profile else {
                        return Ok(error_json(StatusCode::CONFLICT, "尚未配置 Provider。"));
                    };
                    match provider::list_models(&profile).await {
                        Ok(models) => json_response(
                            StatusCode::OK,
                            &serde_json::json!({ "ok": true, "models": models }),
                        ),
                        Err(message) => json_response(
                            StatusCode::OK,
                            &serde_json::json!({ "ok": false, "error": message }),
                        ),
                    }
                }
                "/api/local/sessions/create" => {
                    let result = runtime
                        .store
                        .lock()
                        .map_err(|_| "本地存储不可用。".to_string())
                        .and_then(|mut store| store.create_session(now_string()));
                    match result {
                        Ok(session) => json_response(
                            StatusCode::OK,
                            &serde_json::json!({ "session": session }),
                        ),
                        Err(message) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &message),
                    }
                }
                "/api/local/sessions/delete" => {
                    let id = body_string(&body, "id").unwrap_or_default();
                    let result = runtime
                        .store
                        .lock()
                        .map_err(|_| "本地存储不可用。".to_string())
                        .and_then(|mut store| store.delete_session(id));
                    match result {
                        Ok(()) => json_response(StatusCode::OK, &serde_json::json!({ "ok": true })),
                        Err(message) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &message),
                    }
                }
                "/api/local/chat" => return Ok(handle_chat(&runtime.store, body).await),
                _ => error_json(StatusCode::NOT_FOUND, "接口不存在。"),
            }
        }
        _ => error_json(StatusCode::NOT_FOUND, "接口不存在。"),
    };
    Ok(respond)
}

/// 启动本机 Runtime。调用方持有返回的 Arc 以保持服务运行。
pub async fn start_runtime(dir: &Path) -> Result<Arc<LocalRuntime>, String> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .map_err(|error| format!("本机 Runtime 监听失败：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let runtime = Arc::new(LocalRuntime {
        port,
        store: Arc::new(Mutex::new(LocalStore::load(dir))),
    });
    let server = runtime.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let runtime = server.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |request| {
                    let runtime = runtime.clone();
                    async move { route(&runtime, request).await }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    //! 服务端到端测试：真实绑定回环端口，用 reqwest 走完整 HTTP/SSE 链路。
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_dir() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pisper-local-server-{}-{sequence}",
            std::process::id()
        ))
    }

    async fn spawn() -> (Arc<LocalRuntime>, std::path::PathBuf) {
        let dir = test_dir();
        let runtime = start_runtime(&dir).await.unwrap();
        (runtime, dir)
    }

    fn base(runtime: &LocalRuntime) -> String {
        format!("http://127.0.0.1:{}", runtime.port)
    }

    /// mock OpenAI 上游：按固定脚本返回两帧 SSE + [DONE]。
    async fn spawn_provider_mock(error: bool) -> String {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let service = service_fn(move |_request: Request<Incoming>| async move {
                        if error {
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(500)
                                    .body(BodyExt::boxed_unsync(Full::new(Bytes::from(
                                        "{\"error\":{\"message\":\"boom\"}}",
                                    ))))
                                    .unwrap(),
                            );
                        }
                        let frames: Vec<Result<Frame<Bytes>, Infallible>> = vec![
                            Ok(Frame::data(Bytes::from(
                                "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n",
                            ))),
                            Ok(Frame::data(Bytes::from(
                                "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\ndata: [DONE]\n\n",
                            ))),
                        ];
                        let body = StreamBody::new(futures_util::stream::iter(frames));
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header("content-type", "text/event-stream")
                                .body(BodyExt::boxed_unsync(body))
                                .unwrap(),
                        )
                    });
                    let io = TokioIo::new(stream);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn health_and_unknown_route() {
        let (runtime, dir) = spawn().await;
        let client = reqwest::Client::new();
        let health: serde_json::Value = client
            .get(format!("{}/api/health", base(&runtime)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["runtime"], "mobile-local");

        let missing = client
            .get(format!("{}/api/nope", base(&runtime)))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ui_page_served_same_origin() {
        let (runtime, dir) = spawn().await;
        let response = reqwest::Client::new()
            .get(base(&runtime))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.unwrap();
        assert!(body.contains("Pisper Local"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn provider_crud_redacts_key_and_enforces_https() {
        let (runtime, dir) = spawn().await;
        let client = reqwest::Client::new();
        // 非回环 http 必须被拒绝。
        let rejected = client
            .post(format!("{}/api/local/providers/upsert", base(&runtime)))
            .json(&serde_json::json!({
                "baseUrl": "http://192.168.1.8:11434/v1",
                "apiKey": "x",
                "model": "m",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

        let created = client
            .post(format!("{}/api/local/providers/upsert", base(&runtime)))
            .json(&serde_json::json!({
                "name": "Kimi",
                "baseUrl": "https://api.moonshot.cn/v1",
                "apiKey": "sk-super-secret-key",
                "model": "kimi-k3",
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let body = created.text().await.unwrap();
        assert!(!body.contains("sk-super-secret-key"), "响应不得回显密钥");

        let list = client
            .get(format!("{}/api/local/providers", base(&runtime)))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(list.contains("\"hasKey\":true"));
        assert!(!list.contains("sk-super-secret-key"));

        let state: serde_json::Value = client
            .get(format!("{}/api/local/state", base(&runtime)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state["providerCount"], 1);
        assert_eq!(state["activeProvider"]["model"], "kimi-k3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sessions_crud_roundtrip() {
        let (runtime, dir) = spawn().await;
        let client = reqwest::Client::new();
        let created: serde_json::Value = client
            .post(format!("{}/api/local/sessions/create", base(&runtime)))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["session"]["id"].as_str().unwrap().to_string();

        let list: serde_json::Value = client
            .get(format!("{}/api/local/sessions", base(&runtime)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(), 1);

        let got: serde_json::Value = client
            .get(format!("{}/api/local/sessions/get?id={id}", base(&runtime)))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(got["session"]["id"], id);

        let deleted = client
            .post(format!("{}/api/local/sessions/delete", base(&runtime)))
            .json(&serde_json::json!({ "id": id }))
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        let missing = client
            .get(format!("{}/api/local/sessions/get?id={id}", base(&runtime)))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chat_requires_provider() {
        let (runtime, dir) = spawn().await;
        let client = reqwest::Client::new();
        let created: serde_json::Value = client
            .post(format!("{}/api/local/sessions/create", base(&runtime)))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["session"]["id"].as_str().unwrap();
        let response = client
            .post(format!("{}/api/local/chat", base(&runtime)))
            .json(&serde_json::json!({ "sessionId": id, "text": "hi" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chat_streams_and_persists() {
        let (runtime, dir) = spawn().await;
        let provider_base = spawn_provider_mock(false).await;
        let client = reqwest::Client::new();
        client
            .post(format!("{}/api/local/providers/upsert", base(&runtime)))
            .json(&serde_json::json!({
                "baseUrl": provider_base,
                "apiKey": "sk-x",
                "model": "mock",
            }))
            .send()
            .await
            .unwrap();
        let created: serde_json::Value = client
            .post(format!("{}/api/local/sessions/create", base(&runtime)))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["session"]["id"].as_str().unwrap().to_string();

        let body = client
            .post(format!("{}/api/local/chat", base(&runtime)))
            .json(&serde_json::json!({ "sessionId": id.clone(), "text": "你好" }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("event: meta"), "缺少 meta 帧: {body}");
        assert!(body.contains("\"text\":\"你\""), "缺少增量: {body}");
        assert!(body.contains("event: done"), "缺少 done 帧: {body}");

        // 持久化：用户 + 助手两条消息，标题取自首条用户消息。
        let session = runtime.store.lock().unwrap().session(&id).unwrap();
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].content, "你好");
        assert_eq!(session.title, "你好");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chat_provider_error_removes_empty_placeholder() {
        let (runtime, dir) = spawn().await;
        let provider_base = spawn_provider_mock(true).await;
        let client = reqwest::Client::new();
        client
            .post(format!("{}/api/local/providers/upsert", base(&runtime)))
            .json(&serde_json::json!({
                "baseUrl": provider_base,
                "apiKey": "sk-x",
                "model": "mock",
            }))
            .send()
            .await
            .unwrap();
        let created: serde_json::Value = client
            .post(format!("{}/api/local/sessions/create", base(&runtime)))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["session"]["id"].as_str().unwrap().to_string();

        let body = client
            .post(format!("{}/api/local/chat", base(&runtime)))
            .json(&serde_json::json!({ "sessionId": id.clone(), "text": "hi" }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("event: error"), "缺少 error 帧: {body}");
        assert!(body.contains("boom"), "应透传 Provider 错误: {body}");

        // 零增量失败：占位助手消息必须被清掉，只剩用户消息。
        let session = runtime.store.lock().unwrap().session(&id).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, "user");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn oversized_body_rejected() {
        let (runtime, dir) = spawn().await;
        let huge = "x".repeat(MAX_API_BODY_BYTES + 1);
        let response = reqwest::Client::new()
            .post(format!("{}/api/local/sessions/create", base(&runtime)))
            .body(serde_json::json!({ "pad": huge }).to_string())
            .header("content-type", "application/json")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
