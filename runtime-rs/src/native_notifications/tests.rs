use super::*;
use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
use futures::future::BoxFuture;
use std::{collections::HashSet, fs, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pisper-notification-fixture-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn service(&self) -> NotificationService {
        NotificationService::open(&self.0).unwrap()
    }
    fn seed_channels(&self) -> Value {
        let value = json!({"version":5,"connections":{"feishu":{"enabled":true,"appId":"fixture-app-id-long","appSecret":"synthetic-test-only","ownerOpenId":"fixture-owner"},"weixin":{"enabled":true,"token":"synthetic-test-only"},"qq":null,"telegram":null},"scopes":{"feishu:earlier":{"platform":"feishu","peerId":"earlier","updatedAt":"2026-01-01T00:00:00.000Z"},"feishu:latest":{"platform":"feishu","peerId":"latest","updatedAt":"2026-01-02T00:00:00.000Z","contextToken":"synthetic-test-only"},"weixin:fixture":{"platform":"weixin","peerId":"fixture","updatedAt":"2026-01-01T00:00:00.000Z"}},"futureRoot":{"retain":true},"templates":{"workflow.completed":{"futureTemplate":17,"enabled":true,"channels":{"browser":{"content":"old {{workflow.name}}","futureVariant":23}}},"future.event":{"unknown":"preserve"}}});
        store::write_json(&self.0.join("pisper-channels.json"), &value).unwrap();
        value
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn app(enabled: bool) -> Value {
    json!({"notifications":{"browser":{"enabled":enabled}}})
}

#[test]
fn channels_and_notification_templates_share_one_atomic_writer() {
    let fixture = Fixture::new();
    fixture.seed_channels();
    let service = Arc::new(fixture.service());
    let port = service.channel_state_port();
    let channel = port.clone();
    let template = service.clone();
    std::thread::scope(|threads| {
        let connections = threads.spawn(move || {
            for index in 0..40 {
                (channel.update)(Box::new(move |state| {
                    state["connections"]["telegram"] = json!({"enabled":true,"token":"synthetic-only-token","sequence":index});
                    state["scopes"]["telegram:peer"] = json!({"platform":"telegram","peerId":"peer","sessionId":"fixture-session"});
                    Ok(())
                })).unwrap();
            }
        });
        let templates = threads.spawn(move || {
            for index in 0..40 {
                template
                    .update_template(
                        "chat.completed",
                        "browser",
                        &json!({"content":format!("Template {index} {{{{chat.title}}}}")}),
                        &app(true),
                    )
                    .unwrap();
            }
        });
        connections.join().unwrap();
        templates.join().unwrap();
    });
    let state = (port.read)().unwrap();
    assert_eq!(state["connections"]["telegram"]["sequence"], 39);
    assert_eq!(
        state["scopes"]["telegram:peer"]["sessionId"],
        "fixture-session"
    );
    assert_eq!(
        state["templates"]["chat.completed"]["channels"]["browser"]["content"],
        "Template 39 {{chat.title}}"
    );
    assert_eq!(state["futureRoot"]["retain"], true);
    assert_eq!(
        state["connections"]["feishu"]["appSecret"],
        "synthetic-test-only"
    );
    let before = fs::read(fixture.0.join("pisper-channels.json")).unwrap();
    assert!((port.update)(Box::new(|state| {
        state["connections"] = Value::Null;
        Err(crate::native_channels::ChannelError::new("fixture reject"))
    }))
    .is_err());
    assert_eq!(
        fs::read(fixture.0.join("pisper-channels.json")).unwrap(),
        before
    );
}

#[test]
fn durable_uuid_ledger_release_bootstrap_restart_and_100_20_cursor_contract() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let initial = service.poll("").unwrap();
    assert_eq!(initial, json!({"events":[],"latestId":EMPTY_CURSOR}));
    assert!(!service
        .enqueue(&app(false), "ignored", "ignored", "chat.completed")
        .unwrap());
    assert!(!fixture.0.join("pisper-browser-notifications.json").exists());
    for index in 0..130 {
        service
            .enqueue(
                &app(true),
                "fixture",
                &index.to_string(),
                "schedule.completed",
            )
            .unwrap();
    }
    let reopened = fixture.service();
    let ledger = store::read_json(
        &fixture.0.join("pisper-browser-notifications.json"),
        json!({}),
    )
    .unwrap();
    let events = ledger["events"].as_array().unwrap();
    assert_eq!(events.len(), 100);
    assert_eq!(events[0]["body"], "30");
    for item in events {
        assert!(uuid::Uuid::parse_str(item["id"].as_str().unwrap()).is_ok());
        assert!(chrono::DateTime::parse_from_rfc3339(item["createdAt"].as_str().unwrap()).is_ok());
        assert_eq!(item["event"], "schedule.completed");
    }
    assert_eq!(reopened.poll("").unwrap()["events"], json!([]));
    for cursor in [EMPTY_CURSOR, "missing-uuid"] {
        let polled = reopened.poll(cursor).unwrap();
        assert_eq!(polled["events"].as_array().unwrap().len(), 20);
        assert_eq!(polled["events"][0]["body"], "110");
    }
    assert_eq!(
        reopened.poll(events[0]["id"].as_str().unwrap()).unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        99
    );
    assert_eq!(
        reopened
            .poll(events.last().unwrap()["id"].as_str().unwrap())
            .unwrap()["events"],
        json!([])
    );
}
#[test]
fn parallel_commits_preserve_ledger_metadata_and_do_not_lose_events() {
    let fixture = Fixture::new();
    store::write_json(
        &fixture.0.join("pisper-browser-notifications.json"),
        &json!({"events":[],"futureLedger":{"keep":true}}),
    )
    .unwrap();
    let service = Arc::new(fixture.service());
    let mut handles = vec![];
    for worker in 0..10 {
        let service = service.clone();
        handles.push(std::thread::spawn(move || {
            for index in 0..8 {
                service
                    .enqueue(
                        &app(true),
                        "fixture",
                        &format!("{worker}:{index}"),
                        "workflow.completed",
                    )
                    .unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let ledger = store::read_json(
        &fixture.0.join("pisper-browser-notifications.json"),
        json!({}),
    )
    .unwrap();
    assert_eq!(ledger["futureLedger"], json!({"keep":true}));
    let events = ledger["events"].as_array().unwrap();
    assert_eq!(events.len(), 80);
    assert_eq!(
        events
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        80
    );
    assert_eq!(
        events
            .iter()
            .map(|item| item["body"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        80
    );
}
#[tokio::test]
async fn canonical_settings_deep_merge_preserves_unknown_fields_and_other_documents() {
    let fixture = Fixture::new();
    let old = json!({"toolMode":"workspace","disabledProviders":["example"],"future":{"retain":true},"notifications":{"futureSetting":17,"browser":{"futureBrowser":23,"enabled":false},"weixin":{"keep":true}}});
    store::write_json(&fixture.0.join("pisper.json"), &old).unwrap();
    fs::write(fixture.0.join("settings.json"), b"{\"future\":true}\n").unwrap();
    let before = fs::read(fixture.0.join("settings.json")).unwrap();
    let provider = crate::provider_config::ProviderConfigStore::new(fixture.0.to_str().unwrap());
    let result = provider
        .update_browser_notifications_enabled(true)
        .await
        .unwrap();
    let mut expected = old;
    expected["notifications"]["browser"]["enabled"] = json!(true);
    assert_eq!(result, expected);
    assert_eq!(provider.app_preferences().unwrap(), expected);
    assert_eq!(fs::read(fixture.0.join("settings.json")).unwrap(), before);
    assert_eq!(
        fixture.service().get_state(&result).unwrap()["browser"]["enabled"],
        true
    );
    assert!(!fixture.0.join("rust-browser-notifications.json").exists());
}
#[test]
fn all_six_release_templates_sample_rendering_and_javascript_value_semantics() {
    let fixture = Fixture::new();
    let state = fixture.service().get_state(&app(true)).unwrap();
    let catalog = state["templates"].as_array().unwrap();
    assert_eq!(catalog.len(), 6);
    let expected = [
        ("chat.completed", "对话完成"),
        ("chat.waiting", "等待用户确认"),
        ("schedule.completed", "定时任务完成"),
        ("schedule.failed", "定时任务失败"),
        ("workflow.completed", "工作流完成"),
        ("workflow.failed", "工作流失败"),
    ];
    for (index, (id, name)) in expected.into_iter().enumerate() {
        assert_eq!(catalog[index]["id"], id);
        assert_eq!(catalog[index]["name"], name);
        assert_eq!(catalog[index]["enabled"], true);
        assert_eq!(catalog[index]["channels"].as_object().unwrap().len(), 5);
        assert!(!catalog[index]["variables"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        templates::render(
            catalog[0]["channels"]["browser"]["content"]
                .as_str()
                .unwrap(),
            &templates::sample()
        ),
        "💬 对话「修复渠道通知」已完成\n\n实现已完成，测试和构建均已通过。\n\n模型：openai/gpt-5.4"
    );
    assert_eq!(
        templates::render(
            "{{ value }} {{missing.x}} {{nil}} {{array.0}} {{object}}",
            &json!({"value":false,"nil":null,"array":[42],"object":{"k":true}})
        ),
        "false {{missing.x}} {{nil}} 42 [object Object]"
    );
    assert!(templates::truthy(&json!([])));
    assert!(templates::truthy(&json!({})));
    assert!(templates::truthy(&json!("false")));
    assert!(!templates::truthy(&json!(0)));
    assert!(!templates::truthy(&Value::Null));
}
#[test]
fn explicit_notification_title_content_and_unknown_event_render_guard() {
    let fixture = Fixture::new();
    let service = fixture.service();
    assert_eq!(
        service
            .render_event(
                "workflow.completed",
                &json!({"workflow":{"name":"fixture"}}),
                &json!({"title":"explicit title","content":"explicit content"})
            )
            .unwrap(),
        ("explicit title".into(), "explicit content".into())
    );
    assert_eq!(
        service
            .render_event("workflow.completed", &json!({}), &json!({"content":""}))
            .unwrap()
            .1,
        ""
    );
    assert!(service
        .render_event(
            "unknown",
            &json!({}),
            &json!({"title":"override","content":"override"})
        )
        .is_err());
}
#[tokio::test]
async fn template_edits_preserve_unknowns_and_credentials_disable_delivery_and_test_without_queue()
{
    let fixture = Fixture::new();
    let old = fixture.seed_channels();
    let service = fixture.service();
    let state = service
        .update_template(
            "workflow.completed",
            "browser",
            &json!({"enabled":false,"content":" custom {{workflow.name}} "}),
            &app(true),
        )
        .unwrap();
    assert_eq!(state["templates"][4]["enabled"], false);
    let stored = store::read_json(&fixture.0.join("pisper-channels.json"), json!({})).unwrap();
    assert_eq!(stored["futureRoot"], old["futureRoot"]);
    assert_eq!(stored["connections"], old["connections"]);
    assert_eq!(stored["scopes"], old["scopes"]);
    assert_eq!(
        stored["templates"]["future.event"],
        old["templates"]["future.event"]
    );
    assert_eq!(
        stored["templates"]["workflow.completed"]["futureTemplate"],
        17
    );
    assert_eq!(
        stored["templates"]["workflow.completed"]["channels"]["browser"]["futureVariant"],
        23
    );
    let public = state.to_string();
    assert!(!public.contains("synthetic-test-only"));
    assert_eq!(state["connections"]["weixin"]["supported"], false);
    assert_eq!(state["connections"]["weixin"]["status"], "disconnected");
    assert_eq!(
        service
            .notify(
                &app(true),
                "workflow.completed",
                &json!({"workflow":{"name":"fixture"}}),
                &json!({"platforms":["browser"]})
            )
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(service.poll("missing").unwrap()["events"], json!([]));
    let test = service
        .test_template("workflow.completed", "browser", &app(true))
        .await
        .unwrap();
    assert_eq!(
        test,
        json!({"sent":1,"title":"工作流完成","body":"custom 发布前检查","preview":"custom 发布前检查"})
    );
    assert_eq!(service.poll("missing").unwrap()["events"], json!([]));
    assert_eq!(
        service
            .test_template("workflow.completed", "browser", &app(false))
            .await
            .unwrap_err()
            .message,
        "请先启用通知。"
    );
    let before = fs::read(fixture.0.join("pisper-channels.json")).unwrap();
    assert!(service
        .update_template(
            "workflow.completed",
            "browser",
            &json!({"content":"  "}),
            &app(true)
        )
        .is_err());
    assert_eq!(
        fs::read(fixture.0.join("pisper-channels.json")).unwrap(),
        before
    );
    assert_eq!(
        service
            .update_template("unknown", "browser", &json!({}), &app(true))
            .unwrap_err()
            .message,
        "通知模板类型不存在。"
    );
}
#[tokio::test]
async fn actual_missing_gateway_is_reported_after_browser_commit_without_claiming_delivery() {
    let fixture = Fixture::new();
    fixture.seed_channels();
    let service = fixture.service();
    let error = service
        .notify(
            &app(true),
            "schedule.completed",
            &json!({"task":{"name":"fixture"}}),
            &json!({"platforms":["weixin","browser"],"title":"override","content":"override body"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(error.code, "notification_channel_unavailable");
    assert!(error.message.starts_with("通知发送失败：weixin:"));
    let event = &service.poll("missing").unwrap()["events"][0];
    assert_eq!(event["title"], "override");
    assert_eq!(event["body"], "override body");
    assert_eq!(event["event"], "schedule.completed");
    assert_eq!(
        service
            .notify(
                &app(true),
                "schedule.completed",
                &json!({}),
                &json!({"platforms":["qq","telegram"]})
            )
            .await
            .unwrap(),
        json!([])
    );
}
struct ControlledTransport {
    calls: Mutex<Vec<ChannelDelivery>>,
}
impl NotificationTransport for ControlledTransport {
    fn supports(&self, platform: &str) -> bool {
        ["feishu", "weixin"].contains(&platform)
    }
    fn send(&self, delivery: ChannelDelivery) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let fail = delivery.platform == "weixin";
            self.calls.lock().unwrap().push(delivery);
            if fail {
                Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "notification_fixture_failure",
                    "controlled fixture transport failure",
                ))
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn all_selected_transports_settle_latest_scope_and_overrides_before_error() {
    let fixture = Fixture::new();
    fixture.seed_channels();
    let transport = Arc::new(ControlledTransport {
        calls: Mutex::new(vec![]),
    });
    let service = fixture.service().with_transport(transport.clone());
    let error = service
        .notify(
            &app(true),
            "workflow.completed",
            &json!({"workflow":{"name":"fixture"}}),
            &json!({"platforms":["feishu","weixin","browser"],"content":"custom override"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert_eq!(error.code, "notification_fixture_failure");
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].peer_id, "latest");
    assert_eq!(calls[0].payload, json!({"markdown":"custom override"}));
    assert_eq!(calls[1].payload, json!({"text":"custom override"}));
    assert_eq!(calls[0].scope["peerId"], "latest");
    assert_eq!(calls[0].connection["appSecret"], "synthetic-test-only");
    drop(calls);
    assert_eq!(
        service.poll("missing").unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let state = service.get_state(&app(true)).unwrap();
    assert_eq!(
        state["supportedChannels"],
        json!(["feishu", "weixin", "browser"])
    );
    assert_eq!(state["unsupportedChannels"], json!(["qq", "telegram"]));
}
#[tokio::test]
async fn tui_http_202_reports_normalized_context_and_transport_errors_without_browser_queue() {
    let fixture = Fixture::new();
    fixture.seed_channels();
    let transport = Arc::new(ControlledTransport {
        calls: Mutex::new(vec![]),
    });
    let service = fixture.service().with_transport(transport.clone());
    let response = crate::notification_api::chat_report_response(
        &service,
        &app(true),
        false,
        &json!({"title":" TUI session ","summary":" Finished ","model":"provider/model"}),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["systemNotificationEnabled"], true);
    assert!(body["channelError"].as_str().unwrap().contains("weixin:"));
    assert_eq!(service.poll("missing").unwrap()["events"], json!([]));
    assert_eq!(
        transport.calls.lock().unwrap()[0].payload,
        json!({"markdown":"💬 对话「TUI session」已完成\n\nFinished\n\n模型：provider/model"})
    );
    transport.calls.lock().unwrap().clear();
    let response=crate::notification_api::chat_report_response(&service,&app(false),true,&json!({"title":"Release audit","tool":"bash","reason":"Runs outside workspace.","model":"provider/model"})).await.into_response();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["systemNotificationEnabled"], false);
    assert!(body["channelError"].as_str().unwrap().contains("weixin:"));
    assert!(transport.calls.lock().unwrap()[0].payload["markdown"]
        .as_str()
        .unwrap()
        .contains("操作：bash\n原因：Runs outside workspace."));
}
#[test]
fn legacy_v2_channel_scope_projection_and_empty_template_input_are_safe() {
    let fixture = Fixture::new();
    store::write_json(&fixture.0.join("pisper-channels.json"),&json!({"version":2,"connection":{"enabled":true,"appId":"fixture"},"scopes":{"legacy-peer":{"title":"legacy","updatedAt":"2026-01-01T00:00:00Z"}},"unknown":17})).unwrap();
    let service = fixture.service();
    let state = service.get_state(&app(false)).unwrap();
    assert_eq!(state["scopes"][0]["peerId"], "legacy-peer");
    assert_eq!(state["connections"]["feishu"]["supported"], false);
    assert_eq!(state["templates"].as_array().unwrap().len(), 6);
    service
        .update_template("chat.completed", "browser", &Value::Null, &app(false))
        .unwrap();
    let stored = store::read_json(&fixture.0.join("pisper-channels.json"), json!({})).unwrap();
    assert_eq!(stored["version"], 5);
    assert_eq!(stored["unknown"], 17);
    assert_eq!(stored["scopes"]["feishu:legacy-peer"]["platform"], "feishu");
}
