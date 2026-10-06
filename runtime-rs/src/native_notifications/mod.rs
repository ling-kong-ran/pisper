//! Release-compatible notification settings, templates and durable browser event delivery.
//! External gateways are an explicit transport dependency; saved credentials never imply a live sender.
pub(crate) mod dispatch;
pub(crate) mod store;
pub(crate) mod templates;
#[cfg(test)]
mod tests;

use crate::ApiError;
pub(crate) use dispatch::{ChannelDelivery, NotificationTransport};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub(crate) type Result<T> = std::result::Result<T, ApiError>;
pub(crate) const PLATFORMS: [&str; 5] = ["feishu", "weixin", "qq", "telegram", "browser"];
pub(crate) const EMPTY_CURSOR: &str = "__pisper_browser_events_empty__";

pub(crate) struct NotificationService {
    channels_path: PathBuf,
    ledger_path: PathBuf,
    writes: Arc<Mutex<()>>,
    transport: Option<Arc<dyn NotificationTransport>>,
}
impl NotificationService {
    pub(crate) fn open(agent_dir: &Path) -> Result<Self> {
        store::check_path(agent_dir)?;
        Ok(Self {
            channels_path: agent_dir.join("pisper-channels.json"),
            ledger_path: agent_dir.join("pisper-browser-notifications.json"),
            writes: Arc::new(Mutex::new(())),
            transport: None,
        })
    }
    /// A real gateway adapter may be supplied by the runtime composition layer.
    pub(crate) fn with_transport(mut self, transport: Arc<dyn NotificationTransport>) -> Self {
        self.transport = Some(transport);
        self
    }
    /// 渠道和通知模板修改同一文档，必须复用这个锁；端口只持有锁与路径，
    /// 不持有 NotificationService，避免通知 transport 反向持有渠道形成循环。
    pub(crate) fn channel_state_port(&self) -> crate::native_channels::StatePort {
        let read_path = self.channels_path.clone();
        let read_lock = self.writes.clone();
        let update_path = self.channels_path.clone();
        let update_lock = self.writes.clone();
        crate::native_channels::StatePort {
            read: Arc::new(move || {
                let _write = read_lock
                    .lock()
                    .map_err(|_| crate::native_channels::ChannelError::new("通知存储锁无效。"))?;
                store::read_json(&read_path, json!({})).map_err(channel_error)
            }),
            update: Arc::new(move |mutation| {
                let _write = update_lock
                    .lock()
                    .map_err(|_| crate::native_channels::ChannelError::new("通知存储锁无效。"))?;
                let mut state = store::read_json(&update_path, json!({})).map_err(channel_error)?;
                mutation(&mut state)?;
                store::write_json(&update_path, &state).map_err(channel_error)?;
                Ok(state)
            }),
        }
    }
    pub(crate) fn get_state(&self, app: &Value) -> Result<Value> {
        let _write = self
            .writes
            .lock()
            .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
        let channels =
            store::normalized_channels(&store::read_json(&self.channels_path, json!({}))?);
        Ok(store::public_state(
            &channels,
            app,
            self.transport.as_deref(),
        ))
    }
    pub(crate) fn enqueue(
        &self,
        app: &Value,
        title: &str,
        body: &str,
        event: &str,
    ) -> Result<bool> {
        if !browser_enabled(app) {
            return Ok(false);
        }
        let _write = self
            .writes
            .lock()
            .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
        let mut ledger = store::read_json(&self.ledger_path, json!({"events":[]}))?;
        let object = ledger
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("浏览器通知记录无效。"))?;
        let mut events = object
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        events.push(json!({"id":uuid::Uuid::new_v4().to_string(),"title":if title.is_empty(){"Pisper"}else{title},"body":body,"event":event,"createdAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true)}));
        if events.len() > 100 {
            events.drain(..events.len() - 100);
        }
        object.insert("events".into(), json!(events));
        store::write_json(&self.ledger_path, &ledger)?;
        Ok(true)
    }
    pub(crate) fn poll(&self, after: &str) -> Result<Value> {
        let _write = self
            .writes
            .lock()
            .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
        let ledger = store::read_json(&self.ledger_path, json!({"events":[]}))?;
        let events = ledger["events"].as_array().cloned().unwrap_or_default();
        let latest = events
            .last()
            .and_then(|item| item["id"].as_str())
            .filter(|id| !id.is_empty())
            .unwrap_or(EMPTY_CURSOR);
        let selected = if after.is_empty() {
            vec![]
        } else if after == EMPTY_CURSOR {
            events[events.len().saturating_sub(20)..].to_vec()
        } else if let Some(index) = events.iter().position(|item| item["id"] == after) {
            events[index + 1..].to_vec()
        } else {
            events[events.len().saturating_sub(20)..].to_vec()
        };
        Ok(json!({"events":selected,"latestId":latest}))
    }
    pub(crate) fn update_template(
        &self,
        event: &str,
        platform: &str,
        input: &Value,
        app: &Value,
    ) -> Result<Value> {
        templates::validate_target(event, platform)?;
        let _write = self
            .writes
            .lock()
            .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
        let mut stored = store::read_json(&self.channels_path, json!({}))?;
        let normalized = store::normalized_channels(&stored);
        if !stored.is_object() {
            return Err(ApiError::internal("通知渠道记录无效。"));
        }
        let mut template = normalized["templates"][event].clone();
        if let Some(value) = input.get("enabled") {
            template["enabled"] = json!(templates::truthy(value));
        }
        if let Some(value) = input.get("content") {
            let content = templates::js_string(&if templates::truthy(value) {
                value.clone()
            } else {
                json!("")
            })
            .trim()
            .to_owned();
            if content.is_empty() {
                return Err(ApiError::bad_request("通知模板不能为空。"));
            }
            template["channels"][platform]["content"] = json!(templates::clip(&content, 12_000));
        }
        let object = stored.as_object_mut().unwrap();
        // Preserve unknown fields and existing credentials/scopes; upgrading only the release schema representation.
        for key in ["connections", "scopes"] {
            object.insert(key.into(), normalized[key].clone());
        }
        object.insert("version".into(), json!(5));
        if !object.get("templates").is_some_and(Value::is_object) {
            object.insert("templates".into(), json!({}));
        }
        let existing = object["templates"][event]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut merged = existing;
        merged.insert("enabled".into(), template["enabled"].clone());
        let mut variants = merged
            .get("channels")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for name in PLATFORMS {
            let mut variant = variants
                .get(name)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            variant.insert(
                "content".into(),
                template["channels"][name]["content"].clone(),
            );
            variants.insert(name.into(), Value::Object(variant));
        }
        merged.insert("channels".into(), Value::Object(variants));
        object.get_mut("templates").unwrap()[event] = Value::Object(merged);
        store::write_json(&self.channels_path, &stored)?;
        Ok(store::public_state(
            &store::normalized_channels(&stored),
            app,
            self.transport.as_deref(),
        ))
    }
    pub(crate) fn render_event(
        &self,
        event: &str,
        data: &Value,
        options: &Value,
    ) -> Result<(String, String)> {
        templates::validate_target(event, "browser")?;
        let _write = self
            .writes
            .lock()
            .map_err(|_| ApiError::internal("通知存储锁无效。"))?;
        let channels =
            store::normalized_channels(&store::read_json(&self.channels_path, json!({}))?);
        let (default_title, default_content) =
            templates::rendered(&channels["templates"], event, "browser", data)?;
        let title = options["title"]
            .as_str()
            .map(|value| templates::clip(value, 160))
            .filter(|value| !value.is_empty())
            .unwrap_or(default_title);
        let content = options["content"]
            .as_str()
            .map(|value| templates::clip(value, 12_000))
            .unwrap_or(default_content);
        Ok((title, content))
    }
    pub(crate) async fn test_template(
        &self,
        event: &str,
        platform: &str,
        app: &Value,
    ) -> Result<Value> {
        dispatch::test(self, event, platform, app).await
    }
    pub(crate) async fn notify(
        &self,
        app: &Value,
        event: &str,
        data: &Value,
        options: &Value,
    ) -> Result<Value> {
        dispatch::notify(self, app, event, data, options).await
    }
}
pub(crate) fn browser_enabled(app: &Value) -> bool {
    app["notifications"]["browser"]["enabled"] == true
}
fn channel_error(error: ApiError) -> crate::native_channels::ChannelError {
    crate::native_channels::ChannelError {
        message: error.message,
        status: Some(error.status.as_u16()),
    }
}
