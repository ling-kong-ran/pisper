//! OpenAI 兼容 Provider 客户端：POST {base}/chat/completions（stream）与
//! GET {base}/models（连通性测试）。
//!
//! 只依赖 reqwest(rustls) + 手写增量 SSE 解析，不引入 eventsource 依赖。
//! 安全约定：错误信息一律类别化，绝不回显 apiKey 或请求体。
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;

use super::store::{LocalMessage, ProviderProfile};

/// Provider 连接超时：移动端网络抖动常见，宁快失败不长时间转圈。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 整体流式上限：防止异常 Provider 永不收尾把会话挂死。
const STREAM_TIMEOUT: Duration = Duration::from_secs(180);
/// 错误响应体读取上限：避免错误页/代理异常时读爆内存。
const ERROR_BODY_CAP: usize = 64 * 1024;
/// 模型列表最多回传条数（仅用于设置页展示与测试）。
const MODELS_CAP: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// 增量文本。
    Delta(String),
    /// Provider 宣告流结束（[DONE]）。
    Done,
}

/// 增量 SSE 解析器：数据可能沿 TCP 任意边界切开（包括 UTF-8 字符中间），
/// 因此按字节缓冲、只在完整帧（\n\n 分隔）上解码。
#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = find_frame_end(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..end).collect();
            // 跳过帧尾的分隔符本身（\n\n 或 \r\n\r\n）。
            let skip = if self.buffer.starts_with(b"\r\n\r\n") {
                4
            } else {
                2
            };
            let _ = self.buffer.drain(..skip.min(self.buffer.len()));
            if let Some(event) = parse_frame(&frame) {
                events.push(event);
            }
        }
        events
    }
}

fn find_frame_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| buffer.windows(2).position(|w| w == b"\n\n"))
}

/// 解析单个 SSE 帧：只关心 data: 负载；event:/注释/重试行一律忽略。
/// 返回 None 表示该帧不产生事件（心跳、usage-only 块、无法解析的负载）。
fn parse_frame(frame: &[u8]) -> Option<StreamEvent> {
    let text = String::from_utf8_lossy(frame);
    let mut data = String::new();
    for line in text.lines() {
        if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
        }
    }
    let data = data.trim();
    if data.is_empty() {
        return None;
    }
    if data == "[DONE]" {
        return Some(StreamEvent::Done);
    }
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    // 常规增量：choices[0].delta.content；role/refusal 等其他字段忽略。
    let choices = value.get("choices")?.as_array()?;
    let content = choices.first()?.get("delta")?.get("content")?.as_str()?;
    if content.is_empty() {
        None
    } else {
        Some(StreamEvent::Delta(content.to_string()))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

fn provider_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("初始化网络客户端失败：{error}"))
}

/// 发起流式对话。on_delta 每次收到增量文本即同步回调（实现方负责转发 SSE）。
/// 返回拼接完成的完整文本；即使中途出错，已回调的增量也算数，由调用方决定保留。
pub async fn stream_chat<F>(
    profile: &ProviderProfile,
    history: &[LocalMessage],
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let messages = history
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        // 空内容（例如尚未收尾的助手占位）不应进入请求上下文。
        .filter(|message| !message.content.is_empty())
        .map(|message| ChatMessage {
            role: message.role.as_str(),
            content: message.content.as_str(),
        })
        .collect::<Vec<_>>();
    let request = ChatRequest {
        model: &profile.model,
        messages,
        stream: true,
    };
    let client = provider_client()?;
    let url = format!("{}/chat/completions", profile.base_url);
    let mut builder = client.post(&url).json(&request).timeout(STREAM_TIMEOUT);
    if !profile.api_key.trim().is_empty() {
        builder = builder.bearer_auth(profile.api_key.trim());
    }
    let response = builder
        .send()
        .await
        .map_err(|error| classify_network_error(&error))?;
    if !response.status().is_success() {
        return Err(read_error_body(response).await);
    }

    let mut parser = SseParser::default();
    let mut content = String::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::time::timeout(STREAM_TIMEOUT, stream.next())
            .await
            .map_err(|_| "等待模型回复超时。".to_string())?;
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|error| classify_network_error(&error))?;
        for event in parser.push(&chunk) {
            match event {
                StreamEvent::Delta(text) => {
                    content.push_str(&text);
                    on_delta(&text);
                }
                StreamEvent::Done => return Ok(content),
            }
        }
    }
    // 连接正常关闭但没收到 [DONE]：按已完成处理（部分 Provider 不发 DONE）。
    Ok(content)
}

/// 连通性测试：GET /models，返回前若干个模型 ID。
pub async fn list_models(profile: &ProviderProfile) -> Result<Vec<String>, String> {
    let client = provider_client()?;
    let url = format!("{}/models", profile.base_url);
    let mut builder = client.get(&url).timeout(CONNECT_TIMEOUT);
    if !profile.api_key.trim().is_empty() {
        builder = builder.bearer_auth(profile.api_key.trim());
    }
    let response = builder
        .send()
        .await
        .map_err(|error| classify_network_error(&error))?;
    if !response.status().is_success() {
        return Err(read_error_body(response).await);
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Provider 返回的模型列表不是有效 JSON。".to_string())?;
    let models = body
        .get("data")
        .and_then(|data| data.as_array())
        .map(|data| {
            data.iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .take(MODELS_CAP)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(models)
}

/// 网络错误类别化：不把 reqwest 内部细节（可能含 URL 之外的信息）直接抛给界面。
fn classify_network_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "连接 Provider 超时，请检查网络或稍后再试。".into()
    } else if error.is_connect() {
        "无法连接 Provider，请检查地址与网络。".into()
    } else if error.is_request() {
        "请求被 Provider 拒绝（地址或参数有误）。".into()
    } else {
        "与 Provider 通信失败。".into()
    }
}

/// 非 2xx：读取受限大小的错误体，尽量提取 OpenAI 风格的 error.message。
async fn read_error_body(response: reqwest::Response) -> String {
    let status = response.status();
    let bytes = response.bytes().await.unwrap_or_default();
    let capped = &bytes[..bytes.len().min(ERROR_BODY_CAP)];
    let text = String::from_utf8_lossy(capped);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(message) = value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(|message| message.as_str())
        {
            return format!("Provider 错误（{status}）：{message}");
        }
    }
    format!("Provider 返回错误状态：{status}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full, StreamBody};
    use hyper::body::Frame;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::{convert::Infallible, net::SocketAddr, sync::Arc};
    use tokio::net::TcpListener;

    #[test]
    fn parser_handles_split_and_multi_frame_chunks() {
        let mut parser = SseParser::default();
        // 第一块把帧从 UTF-8 字符中间切开，第二块一次带两帧。
        let first = b"data: {\"choices\":[{\"delta\":{\"content\":\"\xE4\xBD".to_vec();
        assert!(parser.push(&first).is_empty());
        // 第二块一次带两帧 + DONE（字节串拼接，避免 UTF-8 转义限制）。
        let mut second = b"\xA0\xE5\xA5\xBD\"}}]}\n\n".to_vec();
        second.extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n");
        second.extend_from_slice(b"data: [DONE]\n\n");
        let events = parser.push(&second);
        assert_eq!(
            events,
            vec![
                StreamEvent::Delta("你好".into()),
                StreamEvent::Delta(" world".into()),
                StreamEvent::Done,
            ]
        );
    }

    #[test]
    fn parser_ignores_heartbeats_usage_and_bad_payloads() {
        let mut parser = SseParser::default();
        let chunk = concat!(
            ": keepalive\n\n",
            "data: {\"choices\":[],\"usage\":{\"total_tokens\":3}}\n\n",
            "data: not-json\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
        );
        let events = parser.push(chunk.as_bytes());
        assert_eq!(events, vec![StreamEvent::Delta("ok".into())]);
    }

    #[test]
    fn parser_ignores_empty_content_delta() {
        let mut parser = SseParser::default();
        let events = parser.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n");
        assert!(events.is_empty());
    }

    fn profile_to(url: &str) -> ProviderProfile {
        ProviderProfile {
            id: "p".into(),
            name: "mock".into(),
            base_url: url.into(),
            api_key: "sk-test".into(),
            model: "mock-model".into(),
            created_at: "0".into(),
        }
    }

    fn history(text: &str) -> Vec<LocalMessage> {
        vec![LocalMessage {
            id: "m1".into(),
            role: "user".into(),
            content: text.into(),
            created_at: "0".into(),
        }]
    }

    enum MockBehavior {
        Sse,
        Error,
        Models,
    }

    /// 最小 mock：按路径与行为脚本应答；SSE 分两帧写出验证增量到达。
    async fn spawn_mock(behavior: MockBehavior) -> String {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let behavior = Arc::new(behavior);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let behavior = behavior.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                        let behavior = behavior.clone();
                        async move {
                            match &*behavior {
                                MockBehavior::Sse => {
                                    assert_eq!(request.method(), hyper::Method::POST);
                                    let frames: Vec<Result<Frame<Bytes>, Infallible>> = vec![
                                        Ok(Frame::data(Bytes::from(
                                            "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n",
                                        ))),
                                        Ok(Frame::data(Bytes::from(
                                            "data: {\"choices\":[{\"delta\":{\"content\":\"，本机\"}}]}\n\ndata: [DONE]\n\n",
                                        ))),
                                    ];
                                    let body = StreamBody::new(futures_util::stream::iter(frames));
                                    Ok::<_, Infallible>(
                                        Response::builder()
                                            .header("content-type", "text/event-stream")
                                            .body(BodyExt::boxed_unsync(body))
                                            .unwrap(),
                                    )
                                }
                                MockBehavior::Error => Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(429)
                                        .body(BodyExt::boxed_unsync(Full::new(Bytes::from(
                                            "{\"error\":{\"message\":\"quota exceeded\"}}",
                                        ))))
                                        .unwrap(),
                                ),
                                MockBehavior::Models => Ok::<_, Infallible>(
                                    Response::builder()
                                        .body(BodyExt::boxed_unsync(Full::new(Bytes::from(
                                            "{\"data\":[{\"id\":\"m1\"},{\"id\":\"m2\"}]}",
                                        ))))
                                        .unwrap(),
                                ),
                            }
                        }
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
    async fn stream_chat_accumulates_and_reports_deltas() {
        let base = spawn_mock(MockBehavior::Sse).await;
        let profile = profile_to(&base);
        let mut deltas = Vec::new();
        let content = stream_chat(&profile, &history("hi"), |text| {
            deltas.push(text.to_string());
        })
        .await
        .unwrap();
        assert_eq!(content, "你好，本机");
        assert_eq!(deltas, vec!["你好".to_string(), "，本机".to_string()]);
    }

    #[tokio::test]
    async fn stream_chat_surfaces_provider_error_message() {
        let base = spawn_mock(MockBehavior::Error).await;
        let profile = profile_to(&base);
        let error = stream_chat(&profile, &history("hi"), |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("quota exceeded"), "unexpected: {error}");
        assert!(!error.contains("sk-test"), "错误不得泄露密钥: {error}");
    }

    #[tokio::test]
    async fn list_models_returns_ids() {
        let base = spawn_mock(MockBehavior::Models).await;
        let profile = profile_to(&base);
        let models = list_models(&profile).await.unwrap();
        assert_eq!(models, vec!["m1".to_string(), "m2".to_string()]);
    }
}
