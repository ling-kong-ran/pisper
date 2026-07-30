use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::{header, Client, Response};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use url::Url;

use crate::model::{
    ExecutionModeUpdate, McpCatalog, MessagePage, PluginCatalog, RuntimeEvent, SessionSummary,
    SessionsResponse, SkillDefinition, SkillsCatalog, StreamEvent, ToolDefinition,
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
        self.send_json(
            reqwest::Method::POST,
            "/api/sessions",
            &json!({ "name": name, "cwd": cwd.to_string_lossy() }),
        )
        .await
    }

    pub async fn default_thinking_level(&self) -> Result<String> {
        let config = self.get_json::<Value>("/api/config").await?;
        Ok(config
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("medium")
            .to_owned())
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
        sender: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Result<()> {
        let response = self
            .client
            .post(self.url("/api/chat")?)
            .json(&json!({
                "sessionId": session_id,
                "message": message,
                "attachments": [],
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
    use super::SseDecoder;

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
