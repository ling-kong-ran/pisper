use super::*;
#[path = "test_support.rs"]
mod support;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Uri},
    Json, Router,
};
#[derive(Clone)]
struct HttpState {
    ws: String,
    requests: Arc<support::Recorder>,
}
async fn http(
    State(state): State<HttpState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    state.requests.push(json!({"path":uri.to_string(),"authorization":headers.get("authorization").and_then(|value|value.to_str().ok()),"body":serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null)}));
    Json(match uri.path() {
        "/token" => json!({"access_token":"synthetic-qq-token","expires_in":"7200"}),
        "/users/@me" => json!({"id":"bot","username":"fixture"}),
        "/gateway" => json!({"url":state.ws}),
        path if path.ends_with("/files") => json!({"file_info":"synthetic-upload-info"}),
        _ => json!({"id":"sent"}),
    })
}
async fn receive(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Value {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(value) => return serde_json::from_str(&value).unwrap(),
            Message::Ping(value) => socket.send(Message::Pong(value)).await.unwrap(),
            _ => {}
        }
    }
}
#[tokio::test]
async fn qq_loopback_identify_resume_sequence_heartbeat_ack_dedupe_and_group_reply() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws = format!("ws://{}", listener.local_addr().unwrap());
    let (done_send, done_recv) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket
            .send(Message::Text(
                json!({"op":10,"d":{"heartbeat_interval":1000}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let identify = receive(&mut socket).await;
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], "QQBot synthetic-qq-token");
        assert_eq!(
            identify["d"]["intents"],
            (1u64 << 25) | (1u64 << 26) | (1u64 << 30)
        );
        for packet in [
            json!({"op":0,"t":"READY","s":1,"d":{"session_id":"session-fixture"}}),
            json!({"op":0,"t":"C2C_MESSAGE_CREATE","s":2,"d":{"id":"m1","author":{"user_openid":"user1","username":"friend"},"content":" hi "}}),
            json!({"op":0,"t":"GROUP_AT_MESSAGE_CREATE","s":3,"d":{"id":"m2","group_openid":"group1","author":{"user_openid":"user2"},"content":"group"}}),
            json!({"op":0,"t":"GROUP_AT_MESSAGE_CREATE","s":3,"d":{"id":"m2","group_openid":"group1","author":{"user_openid":"user2"},"content":"group"}}),
            json!({"op":1}),
        ] {
            socket
                .send(Message::Text(packet.to_string().into()))
                .await
                .unwrap()
        }
        let heartbeat = receive(&mut socket).await;
        assert_eq!(heartbeat, json!({"op":1,"d":3}));
        socket
            .send(Message::Text(json!({"op":11}).to_string().into()))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket
            .send(Message::Text(
                json!({"op":10,"d":{"heartbeat_interval":41250}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let resume = receive(&mut socket).await;
        assert_eq!(
            resume,
            json!({"op":6,"d":{"token":"QQBot synthetic-qq-token","session_id":"session-fixture","seq":3}})
        );
        socket
            .send(Message::Text(
                json!({"op":0,"t":"RESUMED","s":4,"d":{}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket.send(Message::Text(json!({"op":0,"t":"AT_MESSAGE_CREATE","s":5,"d":{"id":"m3","channel_id":"channel1","author":{"id":"user3"},"content":"channel"}}).to_string().into())).await.unwrap();
        let _ = done_send.send(());
        while let Some(value) = socket.next().await {
            if matches!(value, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    });
    let requests = Arc::new(support::Recorder::default());
    let mut server = support::Server::new(Router::new().fallback(http).with_state(HttpState {
        ws,
        requests: requests.clone(),
    }))
    .await;
    let (callbacks, messages, _) = support::callbacks();
    let gateway = new_protocol_with_endpoints(
        callbacks,
        Endpoints {
            api: server.url.clone(),
            token: format!("{}/token", server.url),
        },
        true,
    );
    gateway
        .connect(json!({"appId":"synthetic-app","appSecret":"synthetic-secret"}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), done_recv)
        .await
        .unwrap()
        .unwrap();
    messages.wait(3).await;
    let received = messages.values.lock().unwrap().clone();
    assert_eq!(received.len(), 3);
    assert_eq!(received[0]["content"], "hi");
    assert_eq!(received[1]["chatType"], "group");
    assert_eq!(received[2]["chatType"], "channel");
    gateway
        .send(received[1].clone(), json!({"text":"reply"}))
        .await
        .unwrap();
    let sent = requests
        .values
        .lock()
        .unwrap()
        .iter()
        .find(|value| value["path"] == "/v2/groups/group1/messages")
        .cloned()
        .unwrap();
    assert_eq!(sent["authorization"], "QQBot synthetic-qq-token");
    assert_eq!(
        sent["body"],
        json!({"content":"reply","msg_type":0,"msg_id":"m2"})
    );
    gateway.disconnect().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    server.close().await;
}
#[test]
fn qq_normalized_messages_and_credential_error_redaction_are_stable() {
    assert!(map_message("READY", &json!({})).is_none());
    let value=map_message("C2C_MESSAGE_CREATE",&json!({"id":"m","author":{"id":"user","username":"U"},"content":" hello ","attachments":[{"url":"//example.invalid/a.png","content_type":"image/png","filename":"a.png"}]})).unwrap();
    assert_eq!(value["peerId"], "user");
    assert_eq!(value["content"], "hello");
    assert_eq!(value["resources"], json!([]));
    let native=map_protocol_message("C2C_MESSAGE_CREATE",&json!({"id":"m","author":{"id":"user"},"attachments":[{"url":"//example.invalid/a.png","content_type":"image/png"}]}),true).unwrap();
    assert_eq!(
        native["resources"][0]["url"],
        "https://example.invalid/a.png"
    );
    assert_eq!(
        safe_error("invalid Bot 123.opaque-secret"),
        "invalid Bot ***"
    );
}

#[tokio::test]
async fn default_qq_wrapper_oracle_preserves_paths_assets_resources_and_reidentify() {
    let oracle: Value = serde_json::from_str(include_str!("qq-wrapper-oracle.json")).unwrap();
    for case in oracle["messageCases"].as_array().unwrap() {
        assert_eq!(
            map_message(case["type"].as_str().unwrap(), &case["raw"]).unwrap_or(Value::Null),
            case["value"]
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws = format!("ws://{}", listener.local_addr().unwrap());
    let (done_send, done_receive) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        for index in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket
                .send(Message::Text(
                    json!({"op":10,"d":{"heartbeat_interval":41250}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let identity = receive(&mut socket).await;
            assert_eq!(identity["op"], 2);
            if index == 0 {
                socket
                    .send(Message::Text(
                        json!({"op":0,"t":"READY","s":4,"d":{"session_id":"ignored"}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                socket.close(None).await.unwrap();
            } else {
                let _ = done_send.send(());
                while let Some(value) = socket.next().await {
                    if matches!(value, Ok(Message::Close(_)) | Err(_)) {
                        break;
                    }
                }
                break;
            }
        }
    });
    let requests = Arc::new(support::Recorder::default());
    let mut server = support::Server::new(Router::new().fallback(http).with_state(HttpState {
        ws,
        requests: requests.clone(),
    }))
    .await;
    let (callbacks, _, _) = support::callbacks();
    let gateway = new_with_endpoints(
        callbacks,
        Endpoints {
            api: server.url.clone(),
            token: format!("{}/token", server.url),
        },
    );
    gateway
        .connect(json!({"appId":"synthetic-app","appSecret":"synthetic-secret","intents":123}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), done_receive)
        .await
        .unwrap()
        .unwrap();
    for case in oracle["sendCases"].as_array().unwrap() {
        gateway
            .send_to_peer(
                "peer/a".into(),
                json!({"markdown":"reply"}),
                case["scope"].clone(),
            )
            .await
            .unwrap();
        let sent = requests.values.lock().unwrap().last().unwrap().clone();
        let expected = reqwest::Url::parse(case["value"]["url"].as_str().unwrap()).unwrap();
        assert_eq!(sent["path"], expected.path());
        assert_eq!(sent["body"], case["value"]["body"]);
    }
    for case in oracle["assetCases"].as_array().unwrap() {
        gateway
            .send_asset(
                "peer".into(),
                case["asset"].clone(),
                json!({"chatType":"group"}),
            )
            .await
            .unwrap();
        let sent = requests.values.lock().unwrap().last().unwrap().clone();
        assert_eq!(sent["body"], case["value"]["body"]);
    }
    assert!(gateway
        .download_resources(json!([{"url":"http://127.0.0.1:1/not-fetched"}]))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        requests
            .values
            .lock()
            .unwrap()
            .iter()
            .filter(|value| value["path"] == "/token")
            .count(),
        2
    );
    gateway.disconnect().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    server.close().await;
}

#[tokio::test]
async fn disconnect_cancels_and_settles_an_inflight_real_http_send() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws = format!("ws://{}", listener.local_addr().unwrap());
    let socket = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    });
    let waiting = Arc::new(tokio::sync::Notify::new());
    let release = tokio_util::sync::CancellationToken::new();
    let entered = waiting.clone();
    let released = release.clone();
    let mut server = support::Server::new(
        Router::new()
            .route(
                "/gateway",
                axum::routing::get(move || {
                    let ws = ws.clone();
                    async move { Json(json!({"url":ws})) }
                }),
            )
            .route(
                "/users/@me",
                axum::routing::get(|| async { Json(json!({"id":"bot"})) }),
            )
            .route(
                "/v2/users/peer/messages",
                axum::routing::post(move || {
                    let entered = entered.clone();
                    let released = released.clone();
                    async move {
                        entered.notify_one();
                        released.cancelled().await;
                        Json(json!({"id":"sent"}))
                    }
                }),
            ),
    )
    .await;
    let (callbacks, _, _) = support::callbacks();
    let gateway = new_with_endpoints(
        callbacks,
        Endpoints {
            api: server.url.clone(),
            token: format!("{}/unused-token", server.url),
        },
    );
    gateway
        .connect(json!({"appId":"fixture-app","token":"synthetic-token"}))
        .await
        .unwrap();
    let sender = gateway.clone();
    let send = tokio::spawn(async move {
        sender
            .send_to_peer("peer".into(), json!({"text":"blocked"}), Value::Null)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), waiting.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), gateway.disconnect())
        .await
        .unwrap()
        .unwrap();
    assert!(send.await.unwrap().is_err());
    assert_eq!(gateway.get_status()["state"], "idle");
    release.cancel();
    socket.await.unwrap();
    server.close().await;
}
