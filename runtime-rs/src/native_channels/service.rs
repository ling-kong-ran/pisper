//! 双向渠道生命周期与会话编排；所有持久化操作由共享 StatePort 在最新值上原子完成。
#[path = "core/messages.rs"]
mod messages;
#[path = "core/state.rs"]
mod state;
#[cfg(test)]
#[path = "core/tests.rs"]
mod tests;

use super::*;
use futures::future::join_all;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::oneshot;
use tokio_util::{
    sync::CancellationToken,
    task::{task_tracker::TaskTrackerToken, TaskTracker},
};

pub(crate) struct ChannelService {
    cwd: String,
    agent: AgentPort,
    state: StatePort,
    gateways: HashMap<String, Arc<dyn Gateway>>,
    onboardings: HashMap<String, Arc<dyn Onboarding>>,
    initialized: AtomicBool,
    initialization: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    admission: Mutex<()>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    queues: Mutex<HashMap<String, (u64, oneshot::Receiver<()>)>>,
    sequence: AtomicU64,
}
struct QueueCompletion(Option<oneshot::Sender<()>>);
impl Drop for QueueCompletion {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for ChannelService {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.tasks.close();
    }
}
impl ChannelService {
    pub(crate) fn new(
        cwd: String,
        agent: AgentPort,
        state: StatePort,
        gateway_factories: HashMap<String, GatewayFactory>,
        onboarding_factories: HashMap<String, OnboardingFactory>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let gateways = gateway_factories
                .into_iter()
                .map(|(platform, factory)| {
                    let owner = weak.clone();
                    let message_platform = platform.clone();
                    let sync_owner = weak.clone();
                    let sync_platform = platform.clone();
                    let callbacks = GatewayCallbacks {
                        on_message: Arc::new(move |message| {
                            if let Some(service) = owner.upgrade() {
                                service.enqueue(&message_platform, message);
                            }
                        }),
                        on_status: Arc::new(|_| {}),
                        on_sync: Arc::new(move |value| {
                            if sync_platform == "weixin" {
                                if let Some(service) = sync_owner.upgrade() {
                                    let _ = service.update_weixin_sync(value);
                                }
                            }
                        }),
                    };
                    (platform, factory(callbacks))
                })
                .collect();
            let onboardings = onboarding_factories
                .into_iter()
                .map(|(platform, factory)| {
                    let owner = weak.clone();
                    let completed_platform = platform.clone();
                    let completed: CompletedSink = Arc::new(move |credentials| {
                        let owner = owner.clone();
                        let platform = completed_platform.clone();
                        Box::pin(async move {
                            let service = owner
                                .upgrade()
                                .ok_or_else(|| ChannelError::new("渠道服务已停止。"))?;
                            service.complete_onboarding(&platform, credentials).await
                        })
                    });
                    (platform, factory(completed))
                })
                .collect();
            Self {
                cwd,
                agent,
                state,
                gateways,
                onboardings,
                initialized: AtomicBool::new(false),
                initialization: tokio::sync::Mutex::new(()),
                closed: AtomicBool::new(false),
                admission: Mutex::new(()),
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                queues: Mutex::new(HashMap::new()),
                sequence: AtomicU64::new(1),
            }
        })
    }
    fn enter(&self) -> Result<TaskTrackerToken> {
        let _admission = self
            .admission
            .lock()
            .map_err(|_| ChannelError::new("渠道准入锁无效。"))?;
        if self.closed.load(Ordering::Acquire) {
            return Err(ChannelError::new("渠道服务已停止。"));
        }
        Ok(self.tasks.token())
    }
    fn gateway(&self, platform: &str) -> Result<Arc<dyn Gateway>> {
        state::check_platform(platform)?;
        self.gateways
            .get(platform)
            .cloned()
            .ok_or_else(|| ChannelError::new("渠道网关尚未接入。"))
    }
    fn onboarding(&self, platform: &str) -> Result<Arc<dyn Onboarding>> {
        self.onboardings
            .get(platform)
            .cloned()
            .ok_or_else(|| ChannelError::new("渠道接入引导尚未接入。"))
    }
    pub(crate) async fn init(self: &Arc<Self>) -> Result<()> {
        let _owned = self.enter()?;
        let _initialization = self.initialization.lock().await;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        (self.state.update)(Box::new(|value| {
            *value = state::normalize(value);
            Ok(())
        }))?;
        self.initialized.store(true, Ordering::Release);
        let value = (self.state.read)()?;
        for platform in state::PLATFORMS {
            if state::truthy(&value["connections"][platform])
                && value["connections"][platform]["enabled"] != false
            {
                let admission = self
                    .admission
                    .lock()
                    .map_err(|_| ChannelError::new("渠道准入锁无效。"))?;
                if self.closed.load(Ordering::Acquire) {
                    break;
                }
                let weak = Arc::downgrade(self);
                self.tasks.spawn(async move {
                    if let Some(service) = weak.upgrade() {
                        let _ = service.connect(platform).await;
                    }
                });
                drop(admission);
            }
        }
        Ok(())
    }
    pub(crate) fn get_state(&self) -> Result<Value> {
        let value = (self.state.read)()?;
        let mut connections = json!({});
        for platform in state::PLATFORMS {
            let live = self
                .gateways
                .get(platform)
                .map(|gateway| gateway.get_status())
                .unwrap_or_else(|| json!({"state":"unavailable","lastError":"渠道网关尚未接入。"}));
            connections[platform] =
                state::connection(platform, &value["connections"][platform], &live, &self.cwd);
        }
        let mut scopes = state::entries(&value["scopes"])
            .into_iter()
            .map(|(key, value)| state::scope(&key, &value))
            .collect::<Vec<_>>();
        scopes.sort_by(state::scope_order);
        Ok(
            json!({"providers":state::providers(),"connections":connections,"scopes":scopes,"templates":state::catalog(&value["templates"])}),
        )
    }
    pub(crate) async fn start_onboarding(
        self: &Arc<Self>,
        platform: &str,
        input: Value,
    ) -> Result<Value> {
        let _owned = self.enter()?;
        state::check_platform(platform)?;
        if input["mode"] == "personal" {
            return Err(ChannelError::new("不支持个人账号接入，请使用官方机器人。"));
        }
        if matches!(platform, "telegram" | "qq")
            && (input.is_object() || input.is_array())
            && !state::entries(&input).is_empty()
        {
            return self.complete_onboarding(platform, input).await;
        }
        let options = if platform == "weixin" {
            let value = (self.state.read)()?;
            let token = &value["connections"]["weixin"]["token"];
            json!({"localTokens":if state::truthy(token){vec![token.clone()]}else{vec![]}})
        } else {
            Value::Null
        };
        self.onboarding(platform)?.start(options).await
    }
    pub(crate) fn get_onboarding(&self, platform: &str, id: &str) -> Option<Value> {
        self.onboardings
            .get(platform)
            .and_then(|onboarding| onboarding.get(id))
    }
    pub(crate) fn cancel_onboarding(&self, platform: &str, id: &str) -> bool {
        self.onboardings
            .get(platform)
            .is_some_and(|onboarding| onboarding.cancel(id))
    }
    pub(crate) fn verify_onboarding(
        &self,
        platform: &str,
        id: &str,
        code: Value,
    ) -> Result<Option<Value>> {
        let _owned = self.enter()?;
        if platform != "weixin" {
            return Err(ChannelError::new("该渠道不需要配对码。"));
        }
        self.onboarding(platform)?.verify(id, code)
    }
    pub(crate) async fn complete_onboarding(
        self: &Arc<Self>,
        platform: &str,
        credentials: Value,
    ) -> Result<Value> {
        let _owned = self.enter()?;
        state::check_platform(platform)?;
        let platform = platform.to_owned();
        let saved_platform = platform.clone();
        let cwd = self.cwd.clone();
        (self.state.update)(Box::new(move |value| {
            let current = &value["connections"][&saved_platform];
            let now = state::now();
            let default_name = match saved_platform.as_str() {
                "feishu" => "Pisper Agent",
                "weixin" => "微信机器人",
                "qq" => "QQ 官方机器人",
                _ => "Telegram Bot",
            };
            let mut connection = json!({"name":default_name,"accessMode":if saved_platform=="telegram"{"all"}else{"owner"},"defaultCwd":state::fallback(&current["defaultCwd"],&cwd),
                "replyModel":state::first(&[&current["replyModel"]]),"executionMode":state::mode(&current["executionMode"]),"runMode":state::run_mode(&current["runMode"]),"enabled":true,"createdAt":state::fallback(&current["createdAt"],&now),"updatedAt":now});
            match saved_platform.as_str() {
                "feishu" => {
                    connection["appId"] = credentials["appId"].clone();
                    connection["appSecret"] = credentials["appSecret"].clone();
                    connection["ownerOpenId"] = state::fallback(&credentials["ownerOpenId"], "");
                    connection["domain"] = state::fallback(&credentials["domain"], "feishu");
                }
                "weixin" => {
                    for field in ["accountId", "token", "baseUrl", "cdnBaseUrl"] {
                        connection[field] = credentials[field].clone();
                    }
                    connection["ownerUserId"] = state::fallback(&credentials["ownerUserId"], "");
                    connection["syncBuf"] = json!("");
                }
                "qq" => {
                    connection["appId"] = json!(state::trim(&state::text(&state::first(&[
                        &credentials["appId"],
                        &credentials["app_id"]
                    ]))));
                    connection["appSecret"] = json!(state::trim(&state::text(&state::first(&[
                        &credentials["appSecret"],
                        &credentials["appSecretOrToken"]
                    ]))));
                    connection["token"] = json!(state::trim(&state::text(&state::first(&[
                        &credentials["token"],
                        &credentials["accessToken"]
                    ]))));
                    connection["ownerUserId"] =
                        json!(state::trim(&state::text(&credentials["ownerUserId"])));
                    connection["baseUrl"] = credentials["baseUrl"].clone();
                }
                _ => {
                    connection["accountId"] =
                        json!(state::trim(&state::text(&credentials["accountId"])));
                    connection["token"] = json!(state::trim(&state::text(&state::first(&[
                        &credentials["token"],
                        &credentials["botToken"]
                    ]))));
                    connection["ownerUserId"] =
                        json!(state::trim(&state::text(&credentials["ownerUserId"])));
                    for field in ["baseUrl", "fileBaseUrl"] {
                        connection[field] = credentials[field].clone();
                    }
                }
            }
            value["connections"][&saved_platform] = connection;
            state::scopes_mut(value)?.retain(|_, scope| scope["platform"] != saved_platform);
            Ok(())
        }))?;
        let status = self.connect(&platform).await?;
        let name = state::first(&[
            &status["bot"]["name"],
            &status["bot"]["username"],
            &status["bot"]["nickname"],
        ]);
        if state::truthy(&name) {
            (self.state.update)(Box::new(move |value| {
                if state::truthy(&value["connections"][&platform]) {
                    value["connections"][&platform]["name"] = name;
                }
                Ok(())
            }))?;
        }
        self.get_state()
    }
    pub(crate) async fn connect(&self, platform: &str) -> Result<Value> {
        let _owned = self.enter()?;
        let gateway = self.gateway(platform)?;
        let value = (self.state.read)()?;
        let connection = value["connections"][platform].clone();
        if !state::truthy(&connection) {
            let name = match platform {
                "feishu" => "飞书",
                "weixin" => "微信",
                "qq" => "QQ",
                _ => "Telegram",
            };
            return Err(ChannelError::new(format!(
                "请先{}连接{name}。",
                if platform == "telegram" {
                    "填写凭据"
                } else {
                    "扫码"
                }
            )));
        }
        if connection["enabled"] == false {
            return Ok(gateway.get_status());
        }
        gateway.connect(connection).await
    }
    pub(crate) async fn reconnect(&self, platform: &str) -> Result<Value> {
        self.connect(platform).await?;
        self.get_state()
    }
    pub(crate) async fn update(&self, platform: &str, input: Value) -> Result<Value> {
        let _owned = self.enter()?;
        state::check_platform(platform)?;
        if !state::truthy(&(self.state.read)()?["connections"][platform]) {
            return Err(ChannelError::new("渠道尚未连接。"));
        }
        if input.get("runMode").is_some()
            && !["plan", "goal", "team"].contains(&state::text(&input["runMode"]).as_str())
        {
            return Err(ChannelError::new("运行模式无效。"));
        }
        let directory = if input.get("defaultCwd").is_some() {
            Some((self.agent.validate_directory)(state::text(&input["defaultCwd"])).await?)
        } else {
            None
        };
        let platform = platform.to_owned();
        let changed = input.get("enabled").is_some();
        let changed_platform = platform.clone();
        let saved = (self.state.update)(Box::new(move |value| {
            let connection = &mut value["connections"][&changed_platform];
            if !state::truthy(connection) {
                return Err(ChannelError::new("渠道尚未连接。"));
            }
            if input.get("enabled").is_some() {
                connection["enabled"] = json!(state::truthy(&input["enabled"]));
            }
            if input.get("accessMode").is_some() {
                connection["accessMode"] = json!(if input["accessMode"] == "all" {
                    "all"
                } else {
                    "owner"
                });
            }
            if input.get("executionMode").is_some() {
                connection["executionMode"] = json!(state::mode(&input["executionMode"]));
            }
            if input.get("runMode").is_some() {
                let mode = state::text(&input["runMode"]);
                if !["plan", "goal", "team"].contains(&mode.as_str()) {
                    return Err(ChannelError::new("运行模式无效。"));
                }
                connection["runMode"] = json!(mode);
            }
            if let Some(directory) = directory {
                connection["defaultCwd"] = json!(directory);
            }
            if input.get("replyModel").is_some() {
                let provider = state::text(&input["replyModel"]["provider"]);
                let model = state::text(&input["replyModel"]["model"]);
                connection["replyModel"] = if provider.is_empty() || model.is_empty() {
                    Value::Null
                } else {
                    json!({"provider":provider,"model":model})
                };
            }
            connection["updatedAt"] = json!(state::now());
            Ok(())
        }))?;
        if changed {
            if state::truthy(&saved["connections"][&platform]["enabled"]) {
                self.connect(&platform).await?;
            } else {
                self.gateway(&platform)?.disconnect().await?;
            }
        }
        self.get_state()
    }
    pub(crate) async fn remove(&self, platform: &str) -> Result<bool> {
        let _owned = self.enter()?;
        self.gateway(platform)?.disconnect().await?;
        let platform = platform.to_owned();
        (self.state.update)(Box::new(move |value| {
            value["connections"][&platform] = Value::Null;
            state::scopes_mut(value)?.retain(|_, scope| scope["platform"] != platform);
            Ok(())
        }))?;
        Ok(true)
    }
    pub(crate) fn reset_scope(&self, key: &str) -> Result<bool> {
        let _owned = self.enter()?;
        let key = key.to_owned();
        let existed = Arc::new(AtomicBool::new(false));
        let marker = existed.clone();
        (self.state.update)(Box::new(move |value| {
            marker.store(
                state::scopes_mut(value)?.remove(&key).is_some(),
                Ordering::Release,
            );
            Ok(())
        }))?;
        Ok(existed.load(Ordering::Acquire))
    }
    pub(crate) fn latest_scope(&self, platform: &str) -> Result<Option<Value>> {
        Ok(state::latest_scope(&(self.state.read)()?, platform))
    }
    pub(crate) fn update_weixin_sync(&self, buffer: Value) -> Result<()> {
        let _owned = self.enter()?;
        (self.state.update)(Box::new(move |value| {
            if state::truthy(&value["connections"]["weixin"])
                && value["connections"]["weixin"]["syncBuf"] != buffer
            {
                value["connections"]["weixin"]["syncBuf"] = buffer;
            }
            Ok(())
        }))?;
        Ok(())
    }
    pub(crate) fn enqueue(self: &Arc<Self>, platform: &str, message: Value) {
        let Ok(_admission) = self.admission.lock() else {
            return;
        };
        if self.closed.load(Ordering::Acquire) || !state::PLATFORMS.contains(&platform) {
            return;
        }
        let bypass =
            messages::approval_command(state::trim(&state::text(&message["content"]))).is_some();
        let key = format!("{platform}:{}", state::string(&message["peerId"]));
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (previous, completion) = if bypass {
            (None, None)
        } else {
            let (sender, receiver) = oneshot::channel();
            let Ok(mut queues) = self.queues.lock() else {
                return;
            };
            (
                queues
                    .insert(key.clone(), (id, receiver))
                    .map(|(_, receiver)| receiver),
                Some(sender),
            )
        };
        let weak = Arc::downgrade(self);
        let platform = platform.to_owned();
        let cancellation = self.shutdown.clone();
        self.tasks.spawn(async move {
            let _completion = QueueCompletion(completion);
            if let Some(previous) = previous {
                tokio::select! {_=cancellation.cancelled()=>return,_=previous=>{}}
            }
            if !cancellation.is_cancelled() {
                if let Some(service) = weak.upgrade() {
                    let _ = service.handle_message(&platform, message).await;
                }
            }
            if !bypass {
                if let Some(service) = weak.upgrade() {
                    if let Ok(mut queues) = service.queues.lock() {
                        if queues
                            .get(&key)
                            .is_some_and(|(sequence, _)| *sequence == id)
                        {
                            queues.remove(&key);
                        }
                    }
                }
            }
        });
    }
    pub(crate) async fn handle_message(
        self: &Arc<Self>,
        platform: &str,
        message: Value,
    ) -> Result<()> {
        let _owned = self.enter()?;
        messages::handle(self, platform, message).await
    }
    pub(crate) fn update_template(
        &self,
        event: &str,
        platform: &str,
        input: Value,
    ) -> Result<Value> {
        let _owned = self.enter()?;
        if !crate::native_notifications::templates::EVENTS.contains(&event)
            || !["feishu", "weixin", "qq", "telegram", "browser"].contains(&platform)
        {
            return Err(ChannelError::new("通知模板类型不存在。"));
        }
        let event = event.to_owned();
        let platform = platform.to_owned();
        (self.state.update)(Box::new(move |value| {
            if input.get("enabled").is_some() {
                value["templates"][&event]["enabled"] = json!(state::truthy(&input["enabled"]));
            }
            if input.get("content").is_some() {
                let content = state::text(&input["content"]);
                let content = state::trim(&content);
                if content.is_empty() {
                    return Err(ChannelError::new("通知模板不能为空。"));
                }
                value["templates"][&event]["channels"][&platform]["content"] =
                    json!(state::clip(content, 12_000));
            }
            Ok(())
        }))?;
        self.get_state()
    }
    pub(crate) fn render_notification(
        &self,
        event: &str,
        platform: &str,
        data: &Value,
    ) -> Result<Value> {
        state::render(&(self.state.read)()?, event, platform, data)
    }
    pub(crate) async fn notify(&self, event: &str, data: &Value, options: &Value) -> Result<Value> {
        let _owned = self.enter()?;
        let value = (self.state.read)()?;
        if !state::truthy(&value["templates"][event]["enabled"]) {
            return Ok(json!([]));
        }
        let selected = options
            .get("platforms")
            .filter(|value| state::truthy(value));
        let selected = match selected {
            None => state::PLATFORMS
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Some(Value::String(value)) => value.chars().map(|value| value.to_string()).collect(),
            _ => {
                return Err(ChannelError::new(
                    "object is not iterable (cannot read property Symbol(Symbol.iterator))",
                ))
            }
        };
        let content = options["content"]
            .as_str()
            .map(|value| state::clip(value, 12_000));
        let mut pending = Vec::new();
        for platform in state::PLATFORMS {
            if !selected.iter().any(|value| value == platform)
                || !state::truthy(&value["connections"][platform]["enabled"])
            {
                continue;
            }
            let Some(scope) = state::latest_scope(&value, platform) else {
                continue;
            };
            let rendered = state::render(&value, event, platform, data)?;
            let content = content
                .clone()
                .unwrap_or_else(|| state::text(&rendered["content"]));
            let gateway = self.gateway(platform)?;
            pending.push(async move {
                let result = gateway
                    .send_to_peer(
                        state::string(&scope["peerId"]),
                        if platform == "feishu" {
                            json!({"markdown":content})
                        } else {
                            json!({"text":content})
                        },
                        scope,
                    )
                    .await;
                match result {
                    Ok(()) => json!({"platform":platform,"status":"fulfilled"}),
                    Err(error) => {
                        json!({"platform":platform,"status":"rejected","error":error.message})
                    }
                }
            });
        }
        Ok(json!(join_all(pending).await))
    }
    pub(crate) async fn test_notification(&self, event: &str, platform: &str) -> Result<Value> {
        let _owned = self.enter()?;
        let value = (self.state.read)()?;
        if !state::truthy(&value["templates"][event]["channels"][platform]) {
            return Err(ChannelError::new("通知模板不存在。"));
        }
        let scope = state::latest_scope(&value, platform)
            .ok_or_else(|| ChannelError::new("该渠道还没有可接收通知的历史会话。"))?;
        let rendered = state::render(&value, event, platform, &state::sample())?;
        let content = rendered["content"].clone();
        self.gateway(platform)?
            .send_to_peer(
                state::string(&scope["peerId"]),
                if platform == "feishu" {
                    json!({"markdown":content})
                } else {
                    json!({"text":content})
                },
                scope,
            )
            .await?;
        Ok(json!({"sent":1,"preview":content}))
    }
    pub(crate) async fn send_to_peer(
        &self,
        platform: &str,
        peer_id: String,
        payload: Value,
        scope: Value,
    ) -> Result<()> {
        let _owned = self.enter()?;
        self.gateway(platform)?
            .send_to_peer(peer_id, payload, scope)
            .await
    }
    pub(crate) async fn dispose(&self) -> Result<()> {
        {
            let _admission = self
                .admission
                .lock()
                .map_err(|_| ChannelError::new("渠道准入锁无效。"))?;
            self.closed.store(true, Ordering::Release);
            self.shutdown.cancel();
            self.tasks.close();
        }
        let mut first = None;
        for result in join_all(
            self.onboardings
                .values()
                .map(|onboarding| onboarding.dispose()),
        )
        .await
        {
            if let Err(error) = result {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        for result in join_all(self.gateways.values().map(|gateway| gateway.disconnect())).await {
            if let Err(error) = result {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        self.tasks.wait().await;
        self.queues
            .lock()
            .map_err(|_| ChannelError::new("渠道队列锁无效。"))?
            .clear();
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    #[cfg(test)]
    async fn wait_idle(&self) {
        self.tasks.close();
        self.tasks.wait().await;
        self.tasks.reopen();
    }
}

#[cfg(test)]
pub(crate) async fn compare_actual_release_oracle(rows: &Value) {
    tests::compare_oracle_cases(rows).await;
}
