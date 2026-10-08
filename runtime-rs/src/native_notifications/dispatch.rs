use super::{browser_enabled, store, templates, NotificationService, Result, PLATFORMS};
use crate::ApiError;
use axum::http::StatusCode;
use futures::future::{join_all, BoxFuture};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Credentials are supplied only to an injected, actual gateway adapter; never returned in HTTP snapshots.
#[derive(Clone)]
pub(crate) struct ChannelDelivery {
    pub(crate) platform: String,
    pub(crate) peer_id: String,
    pub(crate) payload: Value,
    pub(crate) scope: Value,
    pub(crate) connection: Value,
}
pub(crate) trait NotificationTransport: Send + Sync {
    fn supports(&self, platform: &str) -> bool;
    fn send(&self, delivery: ChannelDelivery) -> BoxFuture<'_, Result<()>>;
}
fn unavailable() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "notification_channel_unavailable",
        "原生通知渠道发送服务尚未接入。",
    )
}
fn channels(service: &NotificationService) -> Result<Value> {
    let _write = service
        .writes
        .lock()
        .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
    Ok(store::normalized_channels(&store::read_json(
        &service.channels_path,
        json!({}),
    )?))
}
fn delivery(
    channels: &Value,
    event: &str,
    platform: &str,
    data: &Value,
    override_content: Option<&str>,
) -> Result<Option<ChannelDelivery>> {
    let connection = &channels["connections"][platform];
    if !templates::truthy(&connection["enabled"]) {
        return Ok(None);
    }
    let Some(scope) = store::latest_scope(channels, platform) else {
        return Ok(None);
    };
    let (_, content) = templates::rendered(&channels["templates"], event, platform, data)?;
    let content = override_content.unwrap_or(&content).to_owned();
    Ok(Some(ChannelDelivery {
        platform: platform.into(),
        peer_id: templates::js_string(&scope["peerId"]),
        payload: if platform == "feishu" {
            json!({"markdown":content})
        } else {
            json!({"text":content})
        },
        scope,
        connection: connection.clone(),
    }))
}
pub(crate) async fn notify(
    service: &NotificationService,
    app: &Value,
    event: &str,
    data: &Value,
    options: &Value,
) -> Result<Value> {
    let channels = channels(service)?;
    if channels["templates"][event]["enabled"] != true {
        return Ok(json!([]));
    }
    let selected = if options.get("platforms").is_none_or(Value::is_null) {
        PLATFORMS
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<HashSet<_>>()
    } else {
        options["platforms"]
            .as_array()
            .ok_or_else(|| ApiError::bad_request("通知目标无效。"))?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let content = options["content"]
        .as_str()
        .map(|value| templates::clip(value, 12_000));
    let title = options["title"]
        .as_str()
        .map(|value| templates::clip(value, 160));
    let mut pending = Vec::new();
    for platform in &PLATFORMS[..4] {
        if !selected.contains(*platform) {
            continue;
        }
        if let Some(delivery) = delivery(&channels, event, platform, data, content.as_deref())? {
            let platform = (*platform).to_owned();
            pending.push(async move {
                let result = match service
                    .transport
                    .as_ref()
                    .filter(|transport| transport.supports(&platform))
                {
                    Some(transport) => transport.send(delivery).await,
                    None => Err(unavailable()),
                };
                (platform, result)
            });
        }
    }
    // Settle all selected actual transports before aggregating failures. The browser is still committed for mixed failures.
    let deliveries = join_all(pending).await;
    if selected.contains("browser") {
        let (default_title, default_content) =
            templates::rendered(&channels["templates"], event, "browser", data)?;
        service.enqueue(
            app,
            title
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(&default_title),
            content.as_deref().unwrap_or(&default_content),
            event,
        )?;
    }
    let results=deliveries.iter().map(|(platform,result)|match result{Ok(())=>json!({"platform":platform,"status":"fulfilled"}),Err(error)=>json!({"platform":platform,"status":"rejected","error":crate::security::redact_secret_text(&error.message)})}).collect::<Vec<_>>();
    let failures = deliveries
        .iter()
        .filter_map(|(platform, result)| result.as_ref().err().map(|error| (platform, error)))
        .collect::<Vec<_>>();
    if let Some((_, first)) = failures.first() {
        let message = failures
            .iter()
            .map(|(platform, error)| {
                format!(
                    "{platform}: {}",
                    crate::security::redact_secret_text(&error.message)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ApiError::new(
            first.status,
            first.code,
            format!("通知发送失败：{message}"),
        ));
    }
    Ok(json!(results))
}
pub(crate) async fn test(
    service: &NotificationService,
    event: &str,
    platform: &str,
    app: &Value,
) -> Result<Value> {
    if platform == "browser" && !browser_enabled(app) {
        return Err(ApiError::bad_request("请先启用通知。"));
    }
    if !templates::EVENTS.contains(&event) || !PLATFORMS.contains(&platform) {
        return Err(ApiError::bad_request("通知模板不存在。"));
    }
    let channels = channels(service)?;
    let (title, content) = templates::rendered(
        &channels["templates"],
        event,
        platform,
        &templates::sample(),
    )?;
    if platform == "browser" {
        return Ok(json!({"sent":1,"title":title,"body":content,"preview":content}));
    }
    let scope = store::latest_scope(&channels, platform)
        .ok_or_else(|| ApiError::bad_request("该渠道还没有可接收通知的历史会话。"))?;
    let transport = service
        .transport
        .as_ref()
        .filter(|transport| transport.supports(platform))
        .ok_or_else(unavailable)?;
    transport
        .send(ChannelDelivery {
            platform: platform.into(),
            peer_id: templates::js_string(&scope["peerId"]),
            payload: if platform == "feishu" {
                json!({"markdown":content})
            } else {
                json!({"text":content})
            },
            scope,
            connection: channels["connections"][platform].clone(),
        })
        .await?;
    Ok(json!({"sent":1,"preview":content}))
}

/// TUI dispatch uses external channels only; the TUI itself owns the system notification.
pub(crate) async fn chat_report(
    service: &NotificationService,
    app: &Value,
    waiting: bool,
    input: &Value,
) -> Value {
    let text = |key: &str, fallback: &str, limit: usize| {
        let value = &input[key];
        let string = if templates::truthy(value) {
            templates::js_string(value)
        } else {
            String::new()
        };
        let clipped = templates::clip(string.trim(), limit);
        if clipped.is_empty() {
            fallback.to_owned()
        } else {
            clipped
        }
    };
    let title = text("title", "Pisper conversation", 160);
    let model = text("model", "unknown", 160);
    let (event, data) = if waiting {
        (
            "chat.waiting",
            json!({"chat":{"title":title,"tool":text("tool","Agent action",160),"reason":text("reason","Your confirmation is required.",1_000),"model":model}}),
        )
    } else {
        (
            "chat.completed",
            json!({"chat":{"title":title,"summary":text("summary","The Agent has finished responding.",1_000),"model":model}}),
        )
    };
    let error = match service
        .notify(app, event, &data, &json!({"platforms":PLATFORMS[..4]}))
        .await
    {
        Ok(_) => String::new(),
        Err(error) => crate::security::redact_secret_text(&error.message),
    };
    json!({"accepted":true,"systemNotificationEnabled":browser_enabled(app),"channelError":error})
}
