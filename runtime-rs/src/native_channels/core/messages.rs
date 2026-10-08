//! 入站消息、交互命令与审批旁路；事件回调先保存归属，再异步发送通知。
use super::super::{AgentEventSink, ChannelError, PromptRequest, Result};
use super::{state, ChannelService};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub(super) fn approval_command(command: &str) -> Option<bool> {
    let command = command.to_lowercase();
    for (name, approved) in [
        ("/approve", true),
        ("/yes", true),
        ("/deny", false),
        ("/no", false),
    ] {
        if let Some(rest) = command.strip_prefix(name) {
            if rest.is_empty()
                || rest
                    .chars()
                    .next()
                    .is_some_and(|character| state::trim(&character.to_string()).is_empty())
            {
                return Some(approved);
            }
        }
    }
    None
}
async fn send(
    service: &ChannelService,
    platform: &str,
    message: &Value,
    payload: Value,
) -> Result<()> {
    service
        .gateway(platform)?
        .send(message.clone(), payload)
        .await
}
fn label(platform: &str) -> &str {
    match platform {
        "feishu" => "飞书",
        "weixin" => "微信",
        "qq" => "QQ",
        _ => "Telegram",
    }
}
fn choice(scope: &Value, connection: &Value, key: &str, fallback: &str) -> String {
    state::text(&state::first(&[
        &scope[key],
        &connection[key],
        &json!(fallback),
    ]))
}
pub(super) async fn handle(
    service: &Arc<ChannelService>,
    platform: &str,
    message: Value,
) -> Result<()> {
    let snapshot = (service.state.read)()?;
    let connection = &snapshot["connections"][platform];
    if !state::truthy(&connection["enabled"]) {
        return Ok(());
    }
    let owner = &connection[state::owner_key(platform)];
    if connection["accessMode"] != "all" && (!state::truthy(owner) || message["senderId"] != *owner)
    {
        return send(service,platform,&message,json!({"text":if state::truthy(owner){"当前机器人仅允许扫码创建者使用。"}else{"未获取到创建者身份，请重新扫码或调整访问范围。"}})).await;
    }
    let key = format!("{platform}:{}", state::string(&message["peerId"]));
    let scope = &snapshot["scopes"][&key];
    let content = message["content"]
        .as_str()
        .ok_or_else(|| ChannelError::new("message.content.trim is not a function"))?;
    let command = state::trim(content);
    let normalized = command.to_lowercase();
    match normalized.as_str() {
        "/new"|"/reset" => {service.reset_scope(&key)?;return send(service,platform,&message,json!({"text":"已开始新的 Pisper 会话。"})).await;},
        "/status" => {
            let execution=choice(scope,connection,"executionMode","approval-required");let run=choice(scope,connection,"runMode","plan");
            let text=if state::truthy(&scope["sessionId"]) {format!("会话：{}\n模型：{}\n工作目录：{}\n审批模式：{execution}\n执行模式：{run}",state::string(&scope["sessionId"]),state::text(&state::fallback(&scope["model"],"默认")),choice(scope,connection,"cwd",&state::text(&connection["defaultCwd"])))}else{format!("当前聊天还没有绑定 Pisper 会话。\n审批模式：{execution}\n执行模式：{run}")};
            return send(service,platform,&message,json!({"text":text})).await;
        },
        "/mode" => return send(service,platform,&message,json!({"text":format!("当前审批模式：{}\n可选：approval-required、workspace-write、full-access\n用法：/mode <模式>",choice(scope,connection,"executionMode","approval-required"))})).await,
        "/run" => return send(service,platform,&message,json!({"text":format!("当前执行模式：{}\n可选：plan、goal、team\n用法：/run <模式>",choice(scope,connection,"runMode","plan"))})).await,
        "/dir" => return send(service,platform,&message,json!({"text":format!("当前工作目录：{}\n用法：/dir <path>",state::text(&state::first(&[&scope["cwd"],&connection["defaultCwd"]])))})).await,
        "/stop" => {let stopped=if state::truthy(&scope["sessionId"]){(service.agent.abort)(state::string(&scope["sessionId"])).await?}else{false};return send(service,platform,&message,json!({"text":if stopped{"已停止当前任务。"}else{"当前没有运行中的任务。"}})).await;},
        _ => {},
    }
    if normalized.starts_with("/mode ") {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let mode = parts
            .get(1)
            .map(|value| state::mode(&json!(value)))
            .unwrap_or("");
        if parts.len() != 2
            || !matches!(
                parts[1],
                "approval-required" | "workspace-write" | "workspace" | "full-access"
            )
        {
            return send(
                service,
                platform,
                &message,
                json!({"text":"用法：/mode approval-required|workspace-write|full-access"}),
            )
            .await;
        }
        if state::truthy(&scope["sessionId"]) {
            (service.agent.set_execution_mode)(state::string(&scope["sessionId"]), mode.into())
                .await?;
        }
        let key = key.clone();
        let saved_platform = platform.to_owned();
        let peer = message["peerId"].clone();
        (service.state.update)(Box::new(move |value| {
            state::merge_scope(
                value,
                &key,
                Map::from_iter([
                    ("platform".into(), json!(saved_platform)),
                    ("peerId".into(), peer),
                    ("executionMode".into(), json!(mode)),
                ]),
            )
        }))?;
        return send(
            service,
            platform,
            &message,
            json!({"text":format!("审批模式已切换为 {mode}。")}),
        )
        .await;
    }
    if normalized.starts_with("/run ") {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let run = parts.get(1).unwrap_or(&"").to_lowercase();
        if parts.len() != 2 || !["plan", "goal", "team"].contains(&run.as_str()) {
            return send(
                service,
                platform,
                &message,
                json!({"text":"用法：/run plan|goal|team"}),
            )
            .await;
        }
        if state::truthy(&scope["sessionId"]) {
            if let Err(error) =
                (service.agent.set_run_mode)(state::string(&scope["sessionId"]), run.clone()).await
            {
                return send(service, platform, &message, json!({"text":error.message})).await;
            }
        }
        let saved_key = key.clone();
        let saved_platform = platform.to_owned();
        let peer = message["peerId"].clone();
        let saved_run = run.clone();
        (service.state.update)(Box::new(move |value| {
            state::merge_scope(
                value,
                &saved_key,
                Map::from_iter([
                    ("platform".into(), json!(saved_platform)),
                    ("peerId".into(), peer),
                    ("runMode".into(), json!(saved_run)),
                ]),
            )
        }))?;
        return send(
            service,
            platform,
            &message,
            json!({"text":format!("执行模式已切换为 {run}。") }),
        )
        .await;
    }
    if normalized.starts_with("/dir ") {
        let requested = command
            .split_once(char::is_whitespace)
            .map(|(_, value)| state::trim(value))
            .unwrap_or_default();
        if requested.is_empty() {
            return send(
                service,
                platform,
                &message,
                json!({"text":"用法：/dir <path>"}),
            )
            .await;
        }
        let directory = (service.agent.validate_directory)(requested.into()).await?;
        if state::truthy(&scope["sessionId"]) {
            if let Err(error) =
                (service.agent.set_cwd)(state::string(&scope["sessionId"]), directory.clone()).await
            {
                return send(service, platform, &message, json!({"text":error.message})).await;
            }
        }
        let saved_key = key.clone();
        let saved_platform = platform.to_owned();
        let peer = message["peerId"].clone();
        let saved_directory = directory.clone();
        (service.state.update)(Box::new(move |value| {
            state::merge_scope(
                value,
                &saved_key,
                Map::from_iter([
                    ("platform".into(), json!(saved_platform)),
                    ("peerId".into(), peer),
                    ("cwd".into(), json!(saved_directory)),
                ]),
            )
        }))?;
        return send(
            service,
            platform,
            &message,
            json!({"text":format!("工作目录已切换为 {directory}。") }),
        )
        .await;
    }
    if let Some(approved) = approval_command(command) {
        let approval = command
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| state::text(&scope["pendingApprovalId"]));
        let result = if !approval.is_empty() && state::truthy(&scope["sessionId"]) {
            (service.agent.resolve_approval)(state::string(&scope["sessionId"]), approval, approved)
                .await?
        } else {
            json!({"found":false})
        };
        return send(service,platform,&message,json!({"text":if state::truthy(&result["found"]){if approved{"已批准，继续执行。"}else{"已拒绝本次操作。"}}else{"没有找到待处理的审批请求。"}})).await;
    }
    let gateway = service.gateway(platform)?;
    let attempted = async {
        let resources = gateway
            .download_resources(message["resources"].clone())
            .await?;
        let attachments = resources
            .into_iter()
            .filter_map(state::attachment)
            .collect::<Vec<_>>();
        let prompt = if command.is_empty() && !attachments.is_empty() {
            "请分析这些附件。".to_owned()
        } else {
            command.to_owned()
        };
        if prompt.is_empty() {
            return Ok(());
        }
        let owner = Arc::downgrade(service);
        let event_platform = platform.to_owned();
        let event_key = key.clone();
        let event_message = message.clone();
        let on_event: AgentEventSink = Arc::new(move |event, data| {
            let Some(service) = owner.upgrade() else {
                return;
            };
            if service.closed.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            if event == "permission_request" {
                let key = event_key.clone();
                let platform = event_platform.clone();
                let peer = event_message["peerId"].clone();
                let updated = data.clone();
                let saved = (service.state.update)(Box::new(move |value| {
                    let session = json!(state::text(&state::first(&[
                        &value["scopes"][&key]["sessionId"],
                        &updated["sessionId"],
                        &json!(""),
                    ])));
                    state::merge_scope(
                        value,
                        &key,
                        Map::from_iter([
                            ("platform".into(), json!(platform)),
                            ("peerId".into(), peer),
                            ("sessionId".into(), session),
                            ("pendingApprovalId".into(), updated["id"].clone()),
                        ]),
                    )
                }));
                if saved.is_err() {
                    return;
                }
                let message = event_message.clone();
                let platform = event_platform.clone();
                let owner = Arc::downgrade(&service);
                let text = format!(
                    "需要审批：{}\n{}\n请回复 /approve 批准，或 /deny 拒绝。",
                    state::text(&state::fallback(&data["toolName"], "工具")),
                    state::text(&data["reason"])
                );
                if let Ok(_admission) = service.admission.lock() {
                    if !service.closed.load(std::sync::atomic::Ordering::Acquire) {
                        service.tasks.spawn(async move {
                            if let Some(service) = owner.upgrade() {
                                let _ =
                                    send(&service, &platform, &message, json!({"text":text})).await;
                            }
                        });
                    }
                }
            } else if event == "permission_resolved" {
                let key = event_key.clone();
                let _ = (service.state.update)(Box::new(move |value| {
                    if let Some(scope) = state::scopes_mut(value)?
                        .get_mut(&key)
                        .and_then(Value::as_object_mut)
                    {
                        scope.remove("pendingApprovalId");
                    }
                    Ok(())
                }));
            }
        });
        let run = choice(scope, connection, "runMode", "plan");
        let result = (service.agent.prompt)(PromptRequest {
            session_id: state::text(&scope["sessionId"]),
            message: prompt.clone(),
            attachments,
            cwd: state::text(&state::first(&[
                &scope["cwd"],
                &connection["defaultCwd"],
                &json!(service.cwd),
            ])),
            title: format!(
                "{} · {}",
                label(platform),
                state::text(&state::fallback(
                    &message["senderName"],
                    if message["chatType"] == "p2p" {
                        "私聊"
                    } else {
                        "群聊"
                    }
                ))
            ),
            model: connection["replyModel"].clone(),
            execution_mode: choice(scope, connection, "executionMode", ""),
            goal_mode: run == "goal",
            team_mode: run == "team",
            on_event,
            cancellation: service.shutdown.child_token(),
        })
        .await?;
        let persisted_key = key.clone();
        let persisted_platform = platform.to_owned();
        let peer = message["peerId"].clone();
        let chat_type = message["chatType"].clone();
        let sender = message["senderName"].clone();
        let connection = connection.clone();
        let old_scope = scope.clone();
        let saved_session = result.session_id.clone();
        let saved_cwd = result.cwd.clone();
        let saved_model = result.model.clone();
        let saved_prompt = prompt.clone();
        let context = message["contextToken"].clone();
        let fallback_cwd = service.cwd.clone();
        let saved = (service.state.update)(Box::new(move |value| {
            let title = state::fallback(
                &sender,
                &format!(
                    "{}{}",
                    label(&persisted_platform),
                    if chat_type == "p2p" {
                        "私聊"
                    } else {
                        "群聊"
                    }
                ),
            );
            let model = if !saved_model.is_empty() {
                saved_model
            } else if state::truthy(&connection["replyModel"]) {
                format!(
                    "{}/{}",
                    state::string(&connection["replyModel"]["provider"]),
                    state::string(&connection["replyModel"]["model"])
                )
            } else {
                String::new()
            };
            let latest_context = value["scopes"][&persisted_key]["contextToken"].clone();
            let patch = Map::from_iter([
                ("platform".into(), json!(persisted_platform)),
                ("peerId".into(), peer),
                ("sessionId".into(), json!(saved_session)),
                ("chatType".into(), chat_type),
                ("title".into(), title),
                (
                    "cwd".into(),
                    state::first(&[
                        &json!(saved_cwd),
                        &connection["defaultCwd"],
                        &json!(fallback_cwd),
                    ]),
                ),
                ("model".into(), json!(model)),
                (
                    "contextToken".into(),
                    json!(state::text(&state::first(&[
                        &context,
                        &latest_context,
                        &old_scope["contextToken"],
                        &json!(""),
                    ]))),
                ),
                (
                    "executionMode".into(),
                    state::first(&[
                        &old_scope["executionMode"],
                        &connection["executionMode"],
                        &json!("approval-required"),
                    ]),
                ),
                (
                    "runMode".into(),
                    state::first(&[
                        &old_scope["runMode"],
                        &connection["runMode"],
                        &json!("plan"),
                    ]),
                ),
                ("lastMessage".into(), json!(state::clip(&saved_prompt, 120))),
                ("updatedAt".into(), json!(state::now())),
            ]);
            state::merge_scope(value, &persisted_key, patch)
        }))?;
        gateway
            .send(
                message.clone(),
                json!({"markdown":if result.text.is_empty(){"任务已完成。"}else{&result.text}}),
            )
            .await?;
        for asset in result.assets {
            gateway
                .send_asset(
                    state::string(&message["peerId"]),
                    asset,
                    saved["scopes"][&key].clone(),
                )
                .await?;
        }
        Ok::<_, ChannelError>(())
    }
    .await;
    if let Err(error) = attempted {
        let _ = send(
            service,
            platform,
            &message,
            json!({"text":format!("执行失败：{}",error.message)}),
        )
        .await;
    }
    Ok(())
}
