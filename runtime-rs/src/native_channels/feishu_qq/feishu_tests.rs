use super::*;
#[path = "test_support.rs"]
mod support;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Uri},
    Json, Router,
};
use base64::Engine;
#[test]
fn actual_node_sdk_frames_and_twenty_normalized_message_types_match() {
    let oracle: Value = serde_json::from_str(include_str!("sdk-oracle.json")).unwrap();
    assert_eq!(oracle["sdkVersion"], "1.72.0");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(oracle["frame"]["encoded"].as_str().unwrap())
        .unwrap();
    let frame = Frame::decode(bytes.as_slice()).unwrap();
    assert_eq!(frame.seq_id, 111);
    assert_eq!(frame.log_id, 222);
    assert_eq!(frame.service, 13);
    assert_eq!(frame.header("sum"), "2");
    assert_eq!(frame.payload.as_deref(), Some(b"{}".as_slice()));
    assert_eq!(frame.encode_to_vec(), bytes);
    for case in oracle["messages"].as_array().unwrap() {
        assert_eq!(
            normalize::normalize(&case["event"], "ou_bot").unwrap(),
            case["value"],
            "{}",
            case["event"]["message"]["message_type"]
        );
    }
    assert_eq!(
        optimize_markdown("# title\n## two\n```rust\n# protected\n```"),
        "#### title\n##### two\n```rust\n# protected\n```"
    );
}

#[test]
fn sdk_fixed_decimal_duration_uses_binary_number_ties_and_sender_nullable_fallback() {
    for (ms, text) in [(1250, "1.3s"), (1150, "1.1s"), (1200, "1.2s")] {
        let content = normalize::content(
            "audio",
            &json!({"file_key":"audio","duration":ms}).to_string(),
            &HashMap::new(),
        )
        .0;
        assert!(content.contains(text), "{ms}: {content}");
    }
    let value = normalize::normalize(
        &json!({"sender":{"sender_id":{"open_id":null,"user_id":"user"}},"message":{"message_id":"m","chat_id":"c","chat_type":"p2p","message_type":"text","content":"{\"text\":\"x\"}","mentions":[{"key":"@_user","id":{"open_id":"other"}}]}}),
        "bot",
    ).unwrap();
    assert_eq!(value["senderId"], "user");
    assert!(value["mentions"][0].get("name").is_none());
}
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
    state.requests.push(json!({"path":uri.to_string(),"authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),"body":serde_json::from_slice::<Value>(&body).unwrap_or_else(|_|json!(String::from_utf8_lossy(&body)))}));
    Json(match uri.path() {
        "/open-apis/auth/v3/tenant_access_token/internal" => {
            json!({"code":0,"tenant_access_token":"synthetic-feishu-token","expire":7200})
        }
        "/open-apis/bot/v3/info" => {
            json!({"code":0,"bot":{"open_id":"ou_bot","app_name":"fixture"}})
        }
        "/callback/ws/endpoint" => {
            json!({"code":0,"data":{"URL":state.ws,"ClientConfig":{"PingInterval":120,"ReconnectCount":2,"ReconnectInterval":0,"ReconnectNonce":0}}})
        }
        "/open-apis/im/v1/images" => json!({"code":0,"data":{"image_key":"img_uploaded"}}),
        _ => json!({"code":0,"data":{"message_id":"om_sent"}}),
    })
}
fn event_frame(event: &Value, seq: usize, sum: usize, bytes: Vec<u8>) -> Frame {
    Frame {
        seq_id: 9,
        log_id: 10,
        service: 17,
        method: 1,
        headers: vec![
            Header {
                key: "type".into(),
                value: "event".into(),
            },
            Header {
                key: "message_id".into(),
                value: common::text(&event["event"]["message"]["message_id"]),
            },
            Header {
                key: "sum".into(),
                value: sum.to_string(),
            },
            Header {
                key: "seq".into(),
                value: seq.to_string(),
            },
        ],
        payload: Some(bytes),
        ..Frame::default()
    }
}
#[tokio::test]
async fn loopback_feishu_real_ws_fragments_ack_dedupe_policy_and_reply_transport() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws = format!("ws://{}/?service_id=17", listener.local_addr().unwrap());
    let (done_send, done_recv) = tokio::sync::oneshot::channel();
    let websocket = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let ping = socket.next().await.unwrap().unwrap();
        let Message::Binary(ping) = ping else {
            panic!("expected protobuf ping")
        };
        let ping = Frame::decode(ping.as_ref()).unwrap();
        assert_eq!(ping.header("type"), "ping");
        assert_eq!(ping.service, 17);
        let event = json!({"schema":"2.0","header":{"event_type":"im.message.receive_v1"},"event":{"sender":{"sender_id":{"open_id":"ou_sender"}},"message":{"message_id":"om_event","chat_id":"oc_chat","chat_type":"group","message_type":"text","content":"{\"text\":\"@_user_1 hello\"}","create_time":"0","mentions":[{"key":"@_user_1","name":"bot","id":{"open_id":"ou_bot"}}]}}});
        let bytes = event.to_string().into_bytes();
        let split = bytes.len() / 2;
        for _ in 0..2 {
            for (seq, part) in [(1, bytes[split..].to_vec()), (0, bytes[..split].to_vec())] {
                socket
                    .send(Message::Binary(
                        event_frame(&event, seq, 2, part).encode_to_vec().into(),
                    ))
                    .await
                    .unwrap()
            }
            let Message::Binary(ack) = socket.next().await.unwrap().unwrap() else {
                panic!("expected ack")
            };
            let ack = Frame::decode(ack.as_ref()).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(ack.payload.as_ref().unwrap()).unwrap(),
                json!({"code":200})
            );
        }
        let _ = done_send.send(());
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
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
    let gateway = new_with_endpoints(
        callbacks,
        Endpoints {
            api: Some(server.url.clone()),
        },
    );
    let status = gateway
        .connect(json!({"appId":"cli_0123456789abcdef","appSecret":"synthetic-only"}))
        .await
        .unwrap();
    assert_eq!(status["state"], "connected");
    assert_eq!(status["bot"]["openId"], "ou_bot");
    tokio::time::timeout(Duration::from_secs(5), done_recv)
        .await
        .unwrap()
        .unwrap();
    messages.wait(1).await;
    let message = messages.values.lock().unwrap()[0].clone();
    assert_eq!(message["content"], "hello");
    assert_eq!(messages.values.lock().unwrap().len(), 1);
    gateway
        .send(message, json!({"text":"reply"}))
        .await
        .unwrap();
    gateway
        .send_to_peer("oc_chat".into(), json!({"markdown":"# title"}), Value::Null)
        .await
        .unwrap();
    let image=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgAAAAAgABVKJPXQAAAABJRU5ErkJggg==").unwrap();
    let path = std::env::temp_dir().join(format!("feishu-upload-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, &image).unwrap();
    gateway
        .send_asset(
            "oc_chat".into(),
            json!({"path":path,"name":"fixture.png","mimeType":"image/png"}),
            Value::Null,
        )
        .await
        .unwrap();
    std::fs::remove_file(path).unwrap();
    let requests = requests.values.lock().unwrap();
    let reply = requests
        .iter()
        .find(|value| value["path"] == "/open-apis/im/v1/messages/om_event/reply")
        .unwrap();
    assert_eq!(reply["authorization"], "Bearer synthetic-feishu-token");
    assert_eq!(
        serde_json::from_str::<Value>(reply["body"]["content"].as_str().unwrap()).unwrap(),
        json!({"text":"reply"})
    );
    assert!(requests
        .iter()
        .any(|value| value["path"] == "/open-apis/im/v1/messages?receive_id_type=chat_id"));
    assert!(requests
        .iter()
        .any(|value| value["path"] == "/open-apis/im/v1/images"
            && value["body"]
                .as_str()
                .is_some_and(|body| body.contains("image_type") && body.contains("message"))));
    drop(requests);
    gateway.disconnect().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), websocket)
        .await
        .unwrap()
        .unwrap();
    server.close().await;
}

#[tokio::test]
async fn sdk_numeric_reply_revocation_and_post_format_fallback_use_real_http() {
    async fn handler(
        State(requests): State<Arc<support::Recorder>>,
        uri: Uri,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        requests.push(json!({"path":uri.to_string(),"body":body}));
        Json(if uri.path().ends_with("/reply") {
            json!({"code":230020,"msg":"target gone"})
        } else if body["msg_type"] == "post" {
            json!({"code":230002,"msg":"post format rejected"})
        } else {
            json!({"code":0,"data":{"message_id":"sent"}})
        })
    }
    let requests = Arc::new(support::Recorder::default());
    let mut server =
        support::Server::new(Router::new().fallback(handler).with_state(requests.clone())).await;
    let session = Session {
        config: Value::Null,
        base: server.url.clone(),
        client: common::client(),
        token: AsyncMutex::new(Token {
            value: "synthetic-token".into(),
            until: Instant::now() + Duration::from_secs(60),
        }),
        live: Live::new(),
        bot: Mutex::new(String::new()),
        fragment_epoch: Mutex::new(tokio::time::Instant::now()),
    };
    session
        .send_one(
            "oc_chat",
            "post",
            json!({"zh_cn":{"content":[[{"tag":"md","text":"body"}]]}}),
            Some("om_reply"),
        )
        .await
        .unwrap();
    let requests = requests.values.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert!(requests[0]["path"]
        .as_str()
        .unwrap()
        .ends_with("/om_reply/reply"));
    assert_eq!(requests[1]["body"]["msg_type"], "post");
    assert_eq!(requests[2]["body"]["msg_type"], "text");
    assert_eq!(
        serde_json::from_str::<Value>(requests[2]["body"]["content"].as_str().unwrap()).unwrap(),
        json!({"text":"body"})
    );
    server.close().await;
}
