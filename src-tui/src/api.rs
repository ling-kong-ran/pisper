//! 与 sidecar HTTP API 交互的客户端：REST 调用、SSE 对话流解码、附件准备。
//!
//! 所有请求都以 cookie（`__pisper_desktop=<token>`）与 Origin 头完成鉴权；
//! 对话流是长连接 SSE，其余请求为普通 JSON。路径段一律经过百分号编码，
//! 因为会话 id / Provider id 可能包含 URL 保留字符。

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
        ContextUsage, ExecutionModeUpdate, McpCatalog, MessagePage, ModelOption, PluginCatalog,
        ProviderConnectionUpdate, ProviderOption, RuntimeEvent, SessionCwdUpdate,
        SessionModelUpdate, SessionSummary, SessionsResponse, SkillDefinition, SkillsCatalog,
        StreamEvent, ThinkingLevelUpdate, ToolDefinition, VcsChanges,
    },
    workspace::validate_session_workspace,
};

/// sidecar HTTP API 客户端（可廉价克隆，内部共享连接池）。
/// `client` 用于普通 REST 请求，`stream_client` 用于长连接 SSE 对话流，
/// 两者的超时策略不同，避免短请求的超时设置误杀长时间空闲的对话流。
#[derive(Clone)]
pub struct ApiClient {
    base: Url,
    token: String,
    client: Client,
    stream_client: Client,
}

impl ApiClient {
    /// 创建客户端：解析 base URL、注入鉴权头，并按用途构造两类连接池。
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
        // 本机 Runtime 默认保持连接五秒；缩短连接池空闲时间可复用启动请求，
        // 同时避免继续持有已被 Runtime 回收的连接。
        let client = Client::builder()
            .default_headers(headers.clone())
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(4))
            .build()
            .context("failed to create HTTP client")?;
        // 对话流可能持续很久，因此只限制建连时间，由 SSE 心跳和终态协议决定结束。
        let stream_client = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(4))
            .build()
            .context("failed to create streaming HTTP client")?;
        Ok(Self {
            base,
            token: token.to_owned(),
            client,
            stream_client,
        })
    }

    /// 构造一次性的浏览器引导 URL（`pisper web` 用）：
    /// 携带桌面令牌，浏览器借此完成对本地 Runtime 的一次性鉴权。
    pub fn bootstrap_url(&self, next: &str) -> Result<String> {
        let mut url = self.url("/_pisper/desktop/bootstrap")?;
        url.query_pairs_mut()
            .append_pair("token", &self.token)
            .append_pair("next", next);
        Ok(url.to_string())
    }

    /// 拼接 API 路径（去掉前导斜杠后 join，确保 base 上的子路径被保留）。
    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path.trim_start_matches('/'))
            .context("invalid API path")
    }

    /// 从错误响应中提取人类可读的 `error` 字段；取不到时退回状态码描述。
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

    /// GET JSON 并反序列化（响应非 2xx 时统一转成错误）。
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.client.get(self.url(path)?).send().await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        response.json::<T>().await.context("invalid API response")
    }

    /// 发送 JSON 请求体并反序列化响应。
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

    /// 列出全部会话。
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self
            .get_json::<SessionsResponse>("/api/sessions")
            .await?
            .sessions)
    }

    /// 创建会话；返回前校验 sidecar 确认的工作区与请求一致，
    /// 防止 sidecar 把会话挂到了别的工作区。
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

    /// 拉取运行时诊断信息（doctor 命令用）。
    pub async fn runtime_diagnostics(&self) -> Result<Value> {
        self.get_json("/api/runtime/diagnostics").await
    }

    /// 上报「对话完成」通知事件（是否弹系统通知由 Runtime 决定）。
    pub async fn notify_chat_completed(
        &self,
        title: &str,
        summary: &str,
        model: &str,
    ) -> Result<NotificationDispatch> {
        self.send_json(
            reqwest::Method::POST,
            "/api/settings/notifications/chat-completed",
            &json!({
                "title": title,
                "summary": summary,
                "model": model,
            }),
        )
        .await
    }

    /// 上报「等待审批」通知事件。
    pub async fn notify_chat_waiting(
        &self,
        title: &str,
        tool: &str,
        reason: &str,
        model: &str,
    ) -> Result<NotificationDispatch> {
        self.send_json(
            reqwest::Method::POST,
            "/api/settings/notifications/chat-waiting",
            &json!({
                "title": title,
                "tool": tool,
                "reason": reason,
                "model": model,
            }),
        )
        .await
    }

    /// 读取运行配置，返回默认模型、默认思考级别、模型与 Provider 目录。
    pub async fn runtime_preferences(
        &self,
    ) -> Result<(String, String, Vec<ModelOption>, Vec<ProviderOption>)> {
        let config = self.get_json::<RuntimeConfig>("/api/config").await?;
        Ok(runtime_options(config))
    }

    /// 更新 Provider 连接（协议/Base URL/API Key）。
    pub async fn set_provider_connection(
        &self,
        provider: &str,
        api: &str,
        base_url: &str,
        api_key: &str,
    ) -> Result<ProviderConnectionUpdate> {
        let provider = encode_segment(provider);
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/providers/{provider}/connection"),
            &json!({ "api": api, "baseUrl": base_url, "apiKey": api_key }),
        )
        .await
    }

    /// 获取会话最新一页消息。
    pub async fn messages(&self, session_id: &str) -> Result<MessagePage> {
        self.messages_page(session_id, None).await
    }

    /// 分页获取历史消息（`before` 为游标）。
    pub async fn messages_page(
        &self,
        session_id: &str,
        before: Option<u64>,
    ) -> Result<MessagePage> {
        let id = encode_segment(session_id);
        let mut path = format!("/api/sessions/{id}/messages?limit={MESSAGE_PAGE_LIMIT}");
        if let Some(before) = before {
            path.push_str(&format!("&before={before}"));
        }
        self.get_json(&path).await
    }

    /// 合并拉取工具/Skill 目录：插件工具、MCP 工具、技能；
    /// 只保留已启用且可用的项，MCP 工具统一映射为 `ToolDefinition`。
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

    /// 切换会话工作区。
    pub async fn set_session_cwd(&self, session_id: &str, cwd: &Path) -> Result<SessionCwdUpdate> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/sessions/{id}/cwd"),
            &json!({ "cwd": cwd.to_string_lossy() }),
        )
        .await
    }

    /// 切换会话模型。
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

    /// 查询会话当前思考级别状态。
    pub async fn thinking_state(&self, session_id: &str) -> Result<ThinkingLevelUpdate> {
        let id = encode_segment(session_id);
        self.get_json(&format!("/api/sessions/{id}/thinking-level"))
            .await
    }

    /// 设置会话思考级别。
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

    /// 查询工作区版本控制变更。
    pub async fn vcs_changes(&self, session_id: &str) -> Result<VcsChanges> {
        let id = encode_segment(session_id);
        self.get_json(&format!("/api/sessions/{id}/vcs/changes"))
            .await
    }

    /// 提交工作区变更。
    pub async fn commit_vcs(&self, session_id: &str, message: &str) -> Result<VcsChanges> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/sessions/{id}/vcs/commit"),
            &json!({ "message": message }),
        )
        .await
    }

    /// 推送工作区变更。
    pub async fn push_vcs(&self, session_id: &str) -> Result<VcsChanges> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/sessions/{id}/vcs/push"),
            &json!({}),
        )
        .await
    }

    /// 回退工作区变更。
    pub async fn revert_vcs(&self, session_id: &str) -> Result<VcsChanges> {
        let id = encode_segment(session_id);
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/sessions/{id}/vcs/revert"),
            &json!({}),
        )
        .await
    }

    /// 设置会话执行模式。
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

    /// 压缩会话上下文，返回压缩后的上下文占用（可能为空）。
    pub async fn compact_session(&self, session_id: &str) -> Result<Option<ContextUsage>> {
        let id = encode_segment(session_id);
        Ok(self
            .send_json::<CompactSessionResponse, _>(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/compact"),
                &json!({}),
            )
            .await?
            .context_usage)
    }

    /// 向运行中的会话排队追加输入，返回当前排队数。
    pub async fn queue_session_input(&self, session_id: &str, message: &str) -> Result<usize> {
        let id = encode_segment(session_id);
        let response: QueueInputResponse = self
            .send_json(
                reqwest::Method::POST,
                &format!("/api/sessions/{id}/input"),
                &json!({ "message": message, "behavior": "steer" }),
            )
            .await?;
        Ok(response.queued_inputs.len())
    }

    /// 中止会话当前运行。
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

    /// 解析一个待审批的权限请求。
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

    /// 建立对话流：POST `/api/chat` 后逐块消费 SSE，
    /// 每个事件（含终态 `done`/`error`）经 sender 转发回主事件循环。
    pub async fn stream_chat(
        &self,
        session_id: String,
        workspace: PathBuf,
        message: String,
        requested_tool: Option<String>,
        attachment_paths: Vec<PathBuf>,
        sender: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Result<()> {
        let attachments = prepare_attachments(&workspace, &attachment_paths).await?;
        let response = self
            .stream_client
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
            if forward_stream_events(decoder.push(&chunk?, false)?, &sender) {
                return Ok(());
            }
        }
        if forward_stream_events(decoder.push(&[], true)?, &sender) {
            Ok(())
        } else {
            bail!("response stream ended before a terminal event")
        }
    }
}

/// 通知分发结果：系统通知是否开启、渠道错误信息（供诊断）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDispatch {
    #[serde(default)]
    pub system_notification_enabled: bool,
    #[serde(default)]
    pub channel_error: String,
}

/// 排队输入响应（内部）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueInputResponse {
    #[serde(default)]
    queued_inputs: Vec<Value>,
}

/// 压缩响应（内部）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompactSessionResponse {
    #[serde(default)]
    context_usage: Option<ContextUsage>,
}

/// 运行时全局配置（内部契约，对应 `/api/config`）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeConfig {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    thinking_level: String,
    #[serde(default)]
    providers: Vec<RuntimeProvider>,
}

/// Provider 配置（内部契约）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeProvider {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    provider_type: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    api: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    models: Vec<RuntimeModel>,
}

/// 模型配置（内部契约）。
#[derive(Deserialize)]
struct RuntimeModel {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    reasoning: bool,
}

/// 把运行时配置整理为 UI 可直接使用的选项：
/// 默认模型（`provider/model` 形式）、默认思考级别（空时取 `medium`），
/// 只保留已启用且已配置的 chat 型 Provider 及其模型。
fn runtime_options(
    config: RuntimeConfig,
) -> (String, String, Vec<ModelOption>, Vec<ProviderOption>) {
    let default_model = match (config.provider.as_str(), config.model.as_str()) {
        ("", _) | (_, "") => String::new(),
        (provider, model) => format!("{provider}/{model}"),
    };
    let models = config
        .providers
        .iter()
        .filter(|provider| {
            provider.enabled && provider.configured && provider.provider_type == "chat"
        })
        .flat_map(|provider| {
            provider.models.iter().map(move |model| ModelOption {
                provider: provider.id.clone(),
                id: model.id.clone(),
                name: model.name.clone(),
                reasoning: model.reasoning,
            })
        })
        .collect();
    let providers = config
        .providers
        .into_iter()
        .map(|provider| ProviderOption {
            id: provider.id,
            name: provider.name,
            provider_type: provider.provider_type,
            enabled: provider.enabled,
            configured: provider.configured,
            api: provider.api,
            base_url: provider.base_url,
        })
        .collect();
    (
        default_model,
        if config.thinking_level.is_empty() {
            "medium".to_owned()
        } else {
            config.thinking_level
        },
        models,
        providers,
    )
}

/// 转发一批 SSE 事件到主循环；返回是否出现了终态事件（`done`/`error`）。
/// 即使发生终态事件也继续转发同批其余事件，保证收尾状态完整。
fn forward_stream_events(
    events: Vec<StreamEvent>,
    sender: &mpsc::UnboundedSender<RuntimeEvent>,
) -> bool {
    let mut terminal = false;
    for event in events {
        terminal |= matches!(event.name.as_str(), "done" | "error");
        let _ = sender.send(RuntimeEvent::Stream(event));
    }
    terminal
}

/// 准备聊天附件：校验数量/大小/工作区边界后，按扩展名转成三种类型之一——
/// 图片（base64 data）、文本（UTF-8，超长截断）、文档（base64 data）。
/// 任何校验失败都会拒绝整个请求，避免半成品附件进入对话。
async fn prepare_attachments(workspace: &Path, paths: &[PathBuf]) -> Result<Vec<Value>> {
    if paths.len() > 8 {
        bail!("attachment limit exceeded (maximum 8)");
    }
    let workspace = tokio::fs::canonicalize(workspace)
        .await
        .with_context(|| format!("failed to resolve workspace {}", workspace.display()))?;
    let mut result = Vec::with_capacity(paths.len());
    let mut total_size = 0u64;
    for requested_path in paths {
        // 附件可能在排队期间被替换，因此必须在真正读取前重新验证工作区边界。
        let path = tokio::fs::canonicalize(requested_path)
            .await
            .with_context(|| {
                format!("failed to resolve attachment {}", requested_path.display())
            })?;
        if !path.starts_with(&workspace) {
            bail!(
                "attachment is outside the current workspace: {}",
                path.display()
            );
        }
        let metadata = tokio::fs::metadata(&path)
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
        let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("failed to read attachment {}", path.display()))?;
        if bytes.len() as u64 != metadata.len() || bytes.len() > 10 * 1024 * 1024 {
            bail!("attachment changed while being read: {}", path.display());
        }
        let name = requested_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_owned());
        let extension = attachment_extension(&path);
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
            let text = String::from_utf8(bytes)
                .with_context(|| format!("text attachment is not UTF-8: {}", path.display()))?;
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

/// 提取小写扩展名（无扩展名返回空串）。
fn attachment_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// 图片扩展名 → MIME 类型映射。
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

/// 判断扩展名是否属于可按文本读取的文件类型。
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

/// 判断扩展名是否属于可按文档（二进制）上传的文件类型。
fn is_document_extension(extension: &str) -> bool {
    matches!(
        extension,
        "pdf" | "docx" | "pptx" | "xlsx" | "odt" | "odp" | "ods" | "rtf" | "epub"
    )
}

/// 历史消息每页条数（也是内存中保留消息的计量基数）。
pub const MESSAGE_PAGE_LIMIT: usize = 40;

/// URL 路径段编码：非字母数字一律百分号编码，避免会话/Provider id
/// 中的保留字符破坏路径结构。
fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

// SSE 单行/单事件的字节上限，防止恶意或损坏的流撑爆内存。
const MAX_SSE_PENDING_BYTES: usize = 8 * 1024 * 1024;

/// SSE 解码器：累积字节流，按行解析 `event:`/`data:` 字段，
/// 空行触发事件分发。支持 CRLF 与分块到达，末尾兜底处理残余行。
#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    event: String,
    data_lines: Vec<String>,
    pending_event_bytes: usize,
}

impl SseDecoder {
    /// 推入一块字节；返回本块内完成解码的事件。
    /// `final_chunk` 为真时强制处理残余数据并分发最后一个事件。
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
        if self.buffer.len() > MAX_SSE_PENDING_BYTES {
            bail!("SSE line exceeds the {} byte limit", MAX_SSE_PENDING_BYTES);
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

    /// 处理单行：空行分发事件；注释行（`:` 开头）跳过；
    /// 其余按 `event`/`data` 字段累积，同时累计单事件字节数。
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
            _ => return Ok(()),
        }
        self.pending_event_bytes = self.pending_event_bytes.saturating_add(line.len());
        if self.pending_event_bytes > MAX_SSE_PENDING_BYTES {
            bail!("SSE event exceeds the {} byte limit", MAX_SSE_PENDING_BYTES);
        }
        Ok(())
    }

    /// 分发当前事件：data 为空则复位；否则拼接多行 data、
    /// 解析 JSON（失败时尝试修复孤立代理项），构造 `StreamEvent`。
    fn dispatch(&mut self, events: &mut Vec<StreamEvent>) -> Result<()> {
        if self.data_lines.is_empty() {
            self.event.clear();
            self.pending_event_bytes = 0;
            return Ok(());
        }
        let raw = self.data_lines.join("\n");
        let data = serde_json::from_str(&raw)
            .or_else(|_| serde_json::from_str(&repair_json_surrogates(&raw)))
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
        self.pending_event_bytes = 0;
        Ok(())
    }
}

/// 旧版 Runtime 按 UTF-16 code unit 截断时，可能产生孤立的 `\uDxxx` 转义。
/// 浏览器会容忍该输入，但 serde_json 会拒绝并中止整个流；这里将孤立代理项
/// 修复为 U+FFFD，使版本不匹配时仍能保持流可用。
fn repair_json_surrogates(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_string = !in_string;
            out.push('"');
            i += 1;
            continue;
        }
        if b == b'\\' && in_string {
            if i + 1 < bytes.len() {
                match bytes[i + 1] {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                        out.push('\\');
                        out.push(bytes[i + 1] as char);
                        i += 2;
                        continue;
                    }
                    b'u' => {
                        if let Some(hex) = parse_hex4(bytes, i + 2) {
                            if is_high_surrogate(hex) {
                                if peek_low_surrogate(bytes, i + 6).is_some() {
                                    out.push_str(&input[i..i + 12]);
                                    i += 12;
                                } else {
                                    out.push_str("\\ufffd");
                                    i += 6;
                                }
                                continue;
                            }
                            if is_low_surrogate(hex) {
                                out.push_str("\\ufffd");
                                i += 6;
                                continue;
                            }
                            out.push_str(&input[i..i + 6]);
                            i += 6;
                            continue;
                        }
                    }
                    _ => {}
                }
            }
            out.push('\\');
            i += 1;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 解析 4 位十六进制（\uXXXX 的码点），非法字符或长度不足返回 None。
fn parse_hex4(bytes: &[u8], start: usize) -> Option<u16> {
    if start + 4 > bytes.len() {
        return None;
    }
    let mut value = 0u16;
    for &b in &bytes[start..start + 4] {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        value = value * 16 + u16::from(digit);
    }
    Some(value)
}

/// 探测 \uXXXX 转义是否是低位代理项（\uD800-\uDFFF 范围），用于合并代理对。
fn peek_low_surrogate(bytes: &[u8], start: usize) -> Option<u16> {
    if start + 6 > bytes.len() || bytes[start] != b'\\' || bytes[start + 1] != b'u' {
        return None;
    }
    let hex = parse_hex4(bytes, start + 2)?;
    is_low_surrogate(hex).then_some(hex)
}

/// 是否高位代理（\uD800-\uDBFF）。
fn is_high_surrogate(value: u16) -> bool {
    (0xD800..=0xDBFF).contains(&value)
}

/// 是否低位代理（\uDC00-\uDFFF）。
fn is_low_surrogate(value: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::{
        forward_stream_events, prepare_attachments, repair_json_surrogates, runtime_options,
        RuntimeConfig, SseDecoder, MAX_SSE_PENDING_BYTES,
    };
    use crate::model::RuntimeEvent;
    use tokio::sync::mpsc;

    /// 验证运行时选项只包含已配置 Provider 的模型（未配置的 openai 不产生可选项）。
    #[test]
    fn runtime_model_options_exclude_unconfigured_providers() {
        let config = serde_json::from_value::<RuntimeConfig>(serde_json::json!({
            "provider": "openai",
            "model": "gpt-5",
            "thinkingLevel": "high",
            "providers": [
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "type": "chat",
                    "enabled": true,
                    "configured": false,
                    "api": "openai-responses",
                    "baseUrl": "https://api.openai.com/v1",
                    "models": [{ "id": "gpt-5", "name": "GPT-5" }]
                },
                {
                    "id": "deepseek",
                    "name": "DeepSeek",
                    "type": "chat",
                    "enabled": true,
                    "configured": true,
                    "api": "openai-completions",
                    "baseUrl": "https://api.deepseek.com",
                    "models": [{ "id": "deepseek-chat", "name": "DeepSeek Chat" }]
                }
            ]
        }))
        .unwrap();

        let (default_model, thinking, models, providers) = runtime_options(config);
        assert_eq!(default_model, "openai/gpt-5");
        assert_eq!(thinking, "high");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider, "deepseek");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].base_url, "https://api.openai.com/v1");
        assert_eq!(providers[1].api, "openai-completions");
    }

    /// 验证文本/图片附件能按共享聊天协议序列化（base64 数据、mime 推断）。
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

        let attachments = prepare_attachments(&directory, &[text, image])
            .await
            .unwrap();
        assert_eq!(attachments[0]["kind"], "text");
        assert_eq!(attachments[0]["text"], "# Notes\nUTF-8 中文");
        assert_eq!(attachments[1]["kind"], "image");
        assert_eq!(attachments[1]["mimeType"], "image/png");
        assert_eq!(attachments[1]["data"], "AQID");
        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    /// 验证工作区外附件与非法 UTF-8 附件被拒绝。
    #[tokio::test]
    async fn rejects_attachments_outside_the_workspace_and_invalid_utf8() {
        let root = std::env::temp_dir().join(format!(
            "pisper-tui-attachment-boundary-{}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let outside = root.join("outside.md");
        tokio::fs::write(&outside, "secret").await.unwrap();
        let error = prepare_attachments(&workspace, &[outside])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("outside the current workspace"));

        let invalid = workspace.join("invalid.md");
        tokio::fs::write(&invalid, [0xff, 0xfe]).await.unwrap();
        let error = prepare_attachments(&workspace, &[invalid])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not UTF-8"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    /// 验证终端事件转发成功，且超长 SSE 行被拒绝（防止内存膨胀）。
    #[test]
    fn forwards_terminal_events_and_rejects_unbounded_sse_lines() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let terminal = forward_stream_events(
            vec![crate::model::StreamEvent {
                name: "done".to_owned(),
                data: serde_json::json!({}),
            }],
            &sender,
        );
        assert!(terminal);
        assert!(matches!(receiver.try_recv(), Ok(RuntimeEvent::Stream(_))));

        let mut decoder = SseDecoder::default();
        let oversized = vec![b'a'; MAX_SSE_PENDING_BYTES + 1];
        assert!(decoder.push(&oversized, false).is_err());
    }

    /// 验证 SSE 分块到达（CRLF + 拆包 + EOF final）能正确重组为完整事件。
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

    /// 验证 JSON 代理项修复：孤立高/低代理项替换为 U+FFFD，合法代理对
    /// 与转义引号/普通 \u 转义保持不变。
    #[test]
    fn repair_json_surrogates_normalises_lone_escapes_and_keeps_pairs() {
        // 孤立的 UTF-16 高代理项被修复为 U+FFFD。
        assert_eq!(
            repair_json_surrogates("{\"delta\":\"\\ud83d\"}"),
            "{\"delta\":\"\\ufffd\"}"
        );
        // 孤立的低代理项被修复为 U+FFFD。
        assert_eq!(
            repair_json_surrogates("{\"delta\":\"\\ude00\"}"),
            "{\"delta\":\"\\ufffd\"}"
        );
        // 合法的代理项对逐字节保留。
        let pair = "{\"delta\":\"\\ud83d\\ude00\"}";
        assert_eq!(repair_json_surrogates(pair), pair);
        // 转义的引号与反斜杠不会翻转字符串内/外状态。
        assert_eq!(
            repair_json_surrogates("{\"a\":\"\\\"\\ud83d\"}"),
            "{\"a\":\"\\\"\\ufffd\"}"
        );
        // 非代理项 \u 转义保持原样。
        assert_eq!(
            repair_json_surrogates("{\"a\":\"\\u4e2d\"}"),
            "{\"a\":\"\\u4e2d\"}"
        );
    }

    /// 验证 SSE 解码器能从孤立代理项转义中恢复（替换为 U+FFFD 而非崩溃）。
    #[test]
    fn sse_decoder_recovers_from_lone_surrogate_escapes() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(
                b"event: text_delta\ndata: {\"delta\":\"\\ud83d\"}\n\n",
                true,
            )
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "text_delta");
        assert_eq!(events[0].data["delta"], "\u{FFFD}");
    }
}
