use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::{header, Client, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use url::Url;

use crate::{
    model::{
        ExecutionModeUpdate, McpCatalog, MessagePage, ModelOption, PluginCatalog, RuntimeEvent,
        SessionModelUpdate, SessionSummary, SessionsResponse, SkillDefinition, SkillsCatalog,
        StreamEvent, ThinkingLevelUpdate, ToolDefinition,
    },
    workspace::validate_session_workspace,
};

#[derive(Clone)]
pub struct ApiClient {
    base: Url,
    client: Client,
}

impl ApiClient {
    pub fn new(base: &str, token: &str) -> Result<Self> {
        let base = Url::parse(base).context("invalid sidecar URL")?;
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            header::HeaderValue::from_str(&format!("__pisper_desktop={token}"))?,
        );
        headers.insert(
            header::ORIGIN,
            header::HeaderValue::from_str(base.as_str().trim_end_matches('/'))?,
        );
        // Keep localhost pooling below Node's default five-second keep-alive.
        // This reuses startup requests without retaining a socket the server
        // may already have expired.
        let client = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .pool_idle_timeout(Duration::from_secs(4))
            .build()
            .context("failed to create HTTP client")?;
        Ok(Self { base, client })
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path.trim_start_matches('/'))
            .context("invalid API path")
    }

    async fn response_error(response: Response) -> anyhow::Error {
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_default();
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("request failed ({status})"));
        anyhow!(message)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.client.get(self.url(path)?).send().await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        response.json::<T>().await.context("invalid API response")
    }

    async fn send_json<T: DeserializeOwned, B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .client
            .request(method, self.url(path)?)
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        response.json::<T>().await.context("invalid API response")
    }

    pub async fn sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self
            .get_json::<SessionsResponse>("/api/sessions")
            .await?
            .sessions)
    }

    pub async fn create_session(
        &self,
        name: &str,
        cwd: &std::path::Path,
    ) -> Result<SessionSummary> {
        let session = self
            .send_json(
                reqwest::Method::POST,
                "/api/sessions",
                &json!({ "name": name, "cwd": cwd.to_string_lossy() }),
            )
            .await?;
        validate_session_workspace(&session, Some(cwd))?;
        Ok(session)
    }

    pub async fn runtime_diagnostics(&self) -> Result<Value> {
        self.get_json("/api/runtime/diagnostics").await
    }

    pub async fn runtime_preferences(&self) -> Result<(String, Vec<ModelOption>)> {
        let config = self.get_json::<RuntimeConfig>("/api/config").await?;
        let models = config
            .providers
            .into_iter()
            .filter(|provider| provider.enabled && provider.provider_type == "chat")
            .flat_map(|provider| {
                provider.models.into_iter().map(move |model| ModelOption {
                    provider: provider.id.clone(),
                    id: model.id,
                    name: model.name,
                    reasoning: model.reasoning,
                })
            })
            .collect();
        Ok((
            if config.thinking_level.is_empty() {
                "medium".to_owned()
            } else {
                config.thinking_level
            },
            models,
        ))
    }

    pub async fn messages(&self, session_id: &str) -> Result<MessagePage> {
        let id = encode_segment(session_id);
        self.get_json(&format!("/api/sessions/{id}/messages?limit=100"))
            .await
    }

    pub async fn catalogs(&self) -> Result<(Vec<ToolDefinition>, Vec<SkillDefinition>)> {
        let (plugins, mcp, skills) = tokio::try_join!(
            self.get_json::<PluginCatalog>("/api/plugins"),
            self.get_json::<McpCatalog>("/api/mcp?refresh=0"),
            self.get_json::<SkillsCatalog>("/api/skills"),
        )?;
        let mut tools = plugins
            .tools
            .into_iter()
            .filter(|item| item.enabled)
            .collect::<Vec<_>>();
        tools.extend(
            mcp.tools
                .into_iter()
                .filter(|item| item.available)
                .map(|item| ToolDefinition {
                    id: item.pi_name,
                    name: item.title,
                    description: item.description,
                    enabled: true,
                }),
        );
        Ok((
            tools,
            skills
                .skills
                .into_iter()
                .filter(|item| item.enabled && !item.command.is_empty())
                .collect(),
        ))
    }

    pub async fn set_session_model(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<SessionModelUpdate> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/sessions/{id}/model"),
            &json!({ "provider": provider, "model": model }),
        )
        .await
    }

    pub async fn thinking_state(&self, session_id: &str) -> Result<ThinkingLevelUpdate> {
        let id = encode_segment(session_id);
        self.get_json(&format!("/api/sessions/{id}/thinking-level"))
            .await
    }

    pub async fn set_thinking_level(
        &self,
        session_id: &str,
        level: &str,
    ) -> Result<ThinkingLevelUpdate> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/sessions/{id}/thinking-level"),
            &json!({ "level": level }),
        )
        .await
    }

    pub async fn set_execution_mode(
        &self,
        session_id: &str,
        mode: &str,
    ) -> Result<ExecutionModeUpdate> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/sessions/{id}/execution-mode"),
            &json!({ "mode": mode }),
        )
        .await
    }

    pub async fn abort(&self, session_id: &str) -> Result<()> {
        let id = encode_segment(session_id);
        let _: Value = self
            .send_json(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/abort"),
                &json!({}),
            )
            .await?;
        Ok(())
    }

    pub async fn resolve_approval(
        &self,
        session_id: &str,
        approval_id: &str,
        approved: bool,
    ) -> Result<()> {
        let session = encode_segment(session_id);
        let approval = encode_segment(approval_id);
        let _: Value = self
            .send_json(
                reqwest::Method::POST,
                &format!("/api/sessions/{session}/approvals/{approval}"),
                &json!({ "approved": approved }),
            )
            .await?;
        Ok(())
    }

    pub async fn stream_chat(
        &self,
        session_id: String,
        message: String,
        requested_tool: Option<String>,
        attachment_paths: Vec<PathBuf>,
        sender: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Result<()> {
        let attachments = prepare_attachments(&attachment_paths).await?;
        let response = self
            .client
            .post(self.url("/api/chat")?)
            .json(&json!({
                "sessionId": session_id,
                "message": message,
                "attachments": attachments,
                "goalMode": false,
                "requestedToolNames": requested_tool.into_iter().collect::<Vec<_>>(),
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }

        let mut decoder = SseDecoder::default();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            for event in decoder.push(&chunk?, false)? {
                let terminal = matches!(event.name.as_str(), "done" | "error");
                let _ = sender.send(RuntimeEvent::Stream(event));
                if terminal {
                    return Ok(());
                }
            }
        }
        for event in decoder.push(&[], true)? {
            let _ = sender.send(RuntimeEvent::Stream(event));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeConfig {
    #[serde(default)]
    thinking_level: String,
    #[serde(default)]
    providers: Vec<RuntimeProvider>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeProvider {
    id: String,
    #[serde(rename = "type", default)]
    provider_type: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    models: Vec<RuntimeModel>,
}

#[derive(Deserialize)]
struct RuntimeModel {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    reasoning: bool,
}

async fn prepare_attachments(paths: &[PathBuf]) -> Result<Vec<Value>> {
    if paths.len() > 8 {
        bail!("attachment limit exceeded (maximum 8)");
    }
    let mut result = Vec::with_capacity(paths.len());
    let mut total_size = 0u64;
    for path in paths {
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to inspect attachment {}", path.display()))?;
        if !metadata.is_file() {
            bail!("attachment is not a file: {}", path.display());
        }
        if metadata.len() > 10 * 1024 * 1024 {
            bail!("attachment exceeds 10 MiB: {}", path.display());
        }
        total_size = total_size.saturating_add(metadata.len());
        if total_size > 20 * 1024 * 1024 {
            bail!("total attachment size exceeds 20 MiB");
        }
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("failed to read attachment {}", path.display()))?;
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_owned());
        let extension = attachment_extension(path);
        let common = json!({
            "id": format!("{}-{}", name, metadata.len()),
            "name": name,
            "size": metadata.len(),
        });
        let mut attachment = common.as_object().cloned().unwrap_or_default();
        if let Some(mime_type) = image_mime_type(&extension) {
            attachment.insert("kind".to_owned(), json!("image"));
            attachment.insert("mimeType".to_owned(), json!(mime_type));
            attachment.insert("data".to_owned(), json!(BASE64.encode(bytes)));
        } else if is_text_extension(&extension) {
            let text = String::from_utf8_lossy(&bytes);
            let truncated = text.chars().count() > 200_000;
            let text = text.chars().take(200_000).collect::<String>();
            attachment.insert("kind".to_owned(), json!("text"));
            attachment.insert("mimeType".to_owned(), json!("text/plain"));
            attachment.insert("text".to_owned(), json!(text));
            attachment.insert("truncated".to_owned(), json!(truncated));
        } else if is_document_extension(&extension) {
            attachment.insert("kind".to_owned(), json!("document"));
            attachment.insert("mimeType".to_owned(), json!("application/octet-stream"));
            attachment.insert("extension".to_owned(), json!(extension));
            attachment.insert("data".to_owned(), json!(BASE64.encode(bytes)));
        } else {
            bail!("unsupported attachment type: {}", path.display());
        }
        result.push(Value::Object(attachment));
    }
    Ok(result)
}

fn attachment_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn image_mime_type(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "json"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "css"
            | "html"
            | "xml"
            | "yaml"
            | "yml"
            | "csv"
            | "log"
            | "py"
            | "java"
            | "go"
            | "rs"
            | "sh"
            | "ps1"
            | "toml"
            | "sql"
    )
}

fn is_document_extension(extension: &str) -> bool {
    matches!(
        extension,
        "pdf" | "docx" | "pptx" | "xlsx" | "odt" | "odp" | "ods" | "rtf" | "epub"
    )
}

fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    event: String,
    data_lines: Vec<String>,
}

impl SseDecoder {
    fn push(&mut self, chunk: &[u8], final_chunk: bool) -> Result<Vec<StreamEvent>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        if final_chunk {
            if !self.buffer.is_empty() {
                let line = std::mem::take(&mut self.buffer);
                self.process_line(&line, &mut events)?;
            }
            self.dispatch(&mut events)?;
        }
        Ok(events)
    }

    fn process_line(&mut self, line: &[u8], events: &mut Vec<StreamEvent>) -> Result<()> {
        if line.is_empty() {
            return self.dispatch(events);
        }
        if line.first() == Some(&b':') {
            return Ok(());
        }
        let line = std::str::from_utf8(line).context("SSE stream is not UTF-8")?;
        let (field, mut value) = line.split_once(':').unwrap_or((line, ""));
        if value.starts_with(' ') {
            value = &value[1..];
        }
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data_lines.push(value.to_owned()),
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<StreamEvent>) -> Result<()> {
        if self.data_lines.is_empty() {
            self.event.clear();
            return Ok(());
        }
        let data = serde_json::from_str(&self.data_lines.join("\n"))
            .context("invalid JSON in SSE event")?;
        events.push(StreamEvent {
            name: if self.event.is_empty() {
                "message".to_owned()
            } else {
                std::mem::take(&mut self.event)
            },
            data,
        });
        self.data_lines.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{prepare_attachments, SseDecoder};

    #[tokio::test]
    async fn prepares_text_and_image_attachments_for_the_shared_chat_protocol() {
        let directory = std::env::temp_dir().join(format!(
            "pisper-tui-api-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let text = directory.join("notes.md");
        let image = directory.join("pixel.png");
        tokio::fs::write(&text, "# Notes\nUTF-8 中文")
            .await
            .unwrap();
        tokio::fs::write(&image, [1u8, 2, 3]).await.unwrap();

        let attachments = prepare_attachments(&[text, image]).await.unwrap();
        assert_eq!(attachments[0]["kind"], "text");
        assert_eq!(attachments[0]["text"], "# Notes\nUTF-8 中文");
        assert_eq!(attachments[1]["kind"], "image");
        assert_eq!(attachments[1]["mimeType"], "image/png");
        assert_eq!(attachments[1]["data"], "AQID");
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[test]
    fn parses_chunked_crlf_and_final_records() {
        let mut decoder = SseDecoder::default();
        assert!(decoder
            .push(b"event: text_patch\r\ndata: {\"start\":0,", false)
            .unwrap()
            .is_empty());
        let events = decoder
            .push(
                b"\"text\":\"hello\"}\r\n\r\nevent: done\ndata: {\"ok\":true}",
                true,
            )
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "text_patch");
        assert_eq!(events[0].data["text"], "hello");
        assert_eq!(events[1].name, "done");
    }
}
