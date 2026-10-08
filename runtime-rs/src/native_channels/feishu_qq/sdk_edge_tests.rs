use super::*;
#[path = "test_support.rs"]
mod support;
use axum::{
    body::Body,
    extract::State,
    http::{Response, StatusCode, Uri},
    Router,
};
fn oracle() -> Value {
    serde_json::from_str(include_str!("sdk-edge-oracle.json")).unwrap()
}

#[test]
fn actual_sdk_unusual_unclosed_and_chunked_markdown_matches() {
    let oracle = oracle();
    assert_eq!(oracle["sdkVersion"], "1.72.0");
    assert_eq!(oracle["nodeVersion"], "v24.19.0");
    for case in oracle["markdown"].as_array().unwrap() {
        let posts: Vec<_> = split_markdown(case["input"].as_str().unwrap(), case["limit"].as_u64().unwrap() as usize)
            .into_iter().map(|chunk| json!({"zh_cn":{"title":"","content":[[{"tag":"md","text":optimize_markdown(&chunk)}]]}})).collect();
        assert_eq!(json!(posts), case["posts"], "{}", case["name"]);
    }
}

#[test]
fn actual_sdk_missing_sender_errors_and_fragment_sweep_clock_match() {
    let oracle = oracle();
    for case in oracle["malformed"].as_array().unwrap() {
        let normalized = normalize::normalize(&case["event"], "");
        if case["error"].is_null() {
            assert_eq!(normalized.unwrap(), case["value"], "{}", case["name"]);
        } else {
            assert_eq!(
                normalized.unwrap_err().message,
                case["error"]["message"],
                "{}",
                case["name"]
            );
        }
        assert_eq!(case["ack"], json!({"code":200}));
    }
    assert_eq!(oracle["fragments"]["intervalMs"], 10000);
    let mut cache = HashMap::new();
    for operation in oracle["fragments"]["operations"].as_array().unwrap() {
        let id = operation["id"].as_str().unwrap();
        let now = operation["at"].as_i64().unwrap();
        let mut result = Value::Null;
        match operation["action"].as_str().unwrap() {
            "sweep" => sweep_fragments(&mut cache, now),
            "merge" => {
                let seq = operation["seq"].as_u64().unwrap() as usize;
                let first = !cache.contains_key(id);
                let merged = merge_fragment(
                    &mut cache,
                    id.into(),
                    2,
                    seq,
                    if seq == 0 {
                        b"{\"ok\":".to_vec()
                    } else {
                        b"true}".to_vec()
                    },
                );
                if first {
                    if let Some(entry) = cache.get_mut(id) {
                        entry.at = now;
                    }
                }
                if let Some(bytes) = merged {
                    result = serde_json::from_slice(&bytes).unwrap();
                }
            }
            "inspect" => {}
            action => panic!("unknown oracle operation {action}"),
        }
        assert_eq!(result, operation["result"], "{operation}");
        assert_eq!(
            cache.contains_key(id),
            operation["cached"].as_bool().unwrap(),
            "{operation}"
        );
    }
}
fn session(base: String) -> Arc<Session> {
    Arc::new(Session {
        config: json!({"appId":"cli_0123456789abcdef","appSecret":"synthetic-only"}),
        base,
        client: common::client(),
        token: AsyncMutex::new(Token {
            value: "synthetic-token".into(),
            until: Instant::now() + Duration::from_secs(60),
        }),
        live: Live::new(),
        bot: Mutex::new(String::new()),
        fragment_epoch: Mutex::new(tokio::time::Instant::now()),
    })
}
#[derive(Clone)]
struct RedirectState {
    base: Arc<Mutex<String>>,
    requests: Arc<support::Recorder>,
}
async fn redirect_http(State(state): State<RedirectState>, uri: Uri) -> Response<Body> {
    state.requests.push(json!(uri.to_string()));
    let redirect = |location: String| {
        Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", location)
            .body(Body::empty())
            .unwrap()
    };
    let base = state.base.lock().unwrap().clone();
    match uri.path() {
        "/relative" => redirect("/final".into()),
        "/absolute" => redirect(format!("{base}/final")),
        "/host-change" => redirect(format!(
            "http://redirect.invalid:{}/final",
            reqwest::Url::parse(&base).unwrap().port().unwrap()
        )),
        "/cross-protocol" => redirect(base.replacen("http:", "https:", 1) + "/final"),
        "/loop" => redirect("/loop".into()),
        "/chain" => {
            let remaining = uri
                .query()
                .unwrap()
                .strip_prefix("n=")
                .unwrap()
                .parse::<u32>()
                .unwrap();
            if remaining > 0 {
                redirect(format!("/chain?n={}", remaining - 1))
            } else {
                Response::new(Body::from("synthetic media body"))
            }
        }
        "/no-location" => Response::builder()
            .status(StatusCode::FOUND)
            .body(Body::from("redirect without location"))
            .unwrap(),
        "/failed" => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("absent"))
            .unwrap(),
        _ => Response::new(Body::from("synthetic media body")),
    }
}
#[tokio::test]
async fn actual_sdk_redirect_results_match_real_loopback_http() {
    let requests = Arc::new(support::Recorder::default());
    let base = Arc::new(Mutex::new(String::new()));
    let mut server = support::Server::new(Router::new().fallback(redirect_http).with_state(
        RedirectState {
            base: base.clone(),
            requests: requests.clone(),
        },
    ))
    .await;
    *base.lock().unwrap() = server.url.clone();
    let session = session(server.url.clone());
    for case in oracle()["redirects"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let offset = requests.values.lock().unwrap().len();
        let source = if path == "/dns-source" {
            server.url.replace("127.0.0.1", "localhost")
        } else {
            server.url.clone()
        };
        // The SDK oracle allowlists loopback; call the same private fetch path
        // with a pinned loopback IP. Production materialize still rejects it.
        let result = session
            .fetch_source_url_with_environment(
                reqwest::Url::parse(&(source + path)).unwrap(),
                Some(std::net::Ipv4Addr::LOCALHOST.into()),
                &HashMap::new(),
                None,
            )
            .await;
        if case["error"].is_null() {
            assert_eq!(
                String::from_utf8(result.unwrap()).unwrap(),
                case["body"],
                "{path}"
            );
        } else {
            assert_eq!(
                result.unwrap_err().message,
                case["error"]["message"],
                "{path}"
            );
        }
        let values = requests.values.lock().unwrap();
        assert_eq!(json!(values[offset..]), case["requests"], "{path}");
    }
    assert!(session
        .materialize(&(server.url.clone() + "/relative"))
        .await
        .unwrap_err()
        .message
        .contains("URL blocked"));
    session.live.stop().await;
    server.close().await;
}
fn frame(id: &str, seq: usize, sum: usize, payload: Vec<u8>) -> Frame {
    Frame {
        seq_id: 9,
        log_id: 10,
        service: 17,
        method: 1,
        headers: [
            ("type", "event".into()),
            ("message_id", id.into()),
            ("sum", sum.to_string()),
            ("seq", seq.to_string()),
        ]
        .into_iter()
        .map(|(key, value)| Header {
            key: key.into(),
            value,
        })
        .collect(),
        payload: Some(payload),
        ..Frame::default()
    }
}
async fn ack(socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
        panic!("expected event ACK")
    };
    let response = Frame::decode(bytes.as_ref()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(response.payload.as_ref().unwrap()).unwrap(),
        json!({"code":200})
    );
}
#[tokio::test]
async fn real_ws_missing_sender_ack_and_fragments_complete_after_ten_seconds() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws = format!("ws://{}/?service_id=17", listener.local_addr().unwrap());
    let (done_send, done_receive) = tokio::sync::oneshot::channel();
    let websocket = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        for case in oracle()["malformed"].as_array().unwrap() {
            let value = json!({"schema":"2.0","header":{"event_type":"im.message.receive_v1"},"event":case["event"]});
            socket
                .send(Message::Binary(
                    frame(
                        case["name"].as_str().unwrap(),
                        0,
                        1,
                        value.to_string().into_bytes(),
                    )
                    .encode_to_vec()
                    .into(),
                ))
                .await
                .unwrap();
            ack(&mut socket).await;
        }
        // The SDK sweep at 10s sees a 9s-old first fragment. A completing
        // fragment aged >10s still joins it until the next 20s sweep.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let value = json!({"schema":"2.0","header":{"event_type":"im.message.receive_v1"},"event":{"sender":{"sender_id":{"open_id":"ou_sender"}},"message":{"message_id":"om_late_fragment","chat_id":"oc_synthetic","chat_type":"p2p","message_type":"text","content":"{\"text\":\"late synthetic fragment\"}","create_time":"0"}}});
        let bytes = value.to_string().into_bytes();
        let split = bytes.len() / 2;
        socket
            .send(Message::Binary(
                frame("late_actual", 0, 2, bytes[..split].to_vec())
                    .encode_to_vec()
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10050)).await;
        socket
            .send(Message::Binary(
                frame("late_actual", 1, 2, bytes[split..].to_vec())
                    .encode_to_vec()
                    .into(),
            ))
            .await
            .unwrap();
        ack(&mut socket).await;
        let _ = done_send.send(());
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    });
    let (callbacks, messages, statuses) = support::callbacks();
    let status = common::Status::new(callbacks, json!({}));
    let session = session(String::new());
    let socket = session.open(&ws).await.unwrap();
    let running_session = session.clone();
    let running_status = status.clone();
    let running = tokio::spawn(async move {
        event_loop(
            running_session,
            running_status,
            socket,
            json!({"PingInterval":120}),
            17,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(15), done_receive)
        .await
        .unwrap()
        .unwrap();
    messages.wait(3).await;
    let received = messages.values.lock().unwrap().clone();
    assert_eq!(received.len(), 3);
    assert_eq!(received[2]["content"], "late synthetic fragment");
    let errors: Vec<_> = statuses
        .values
        .lock()
        .unwrap()
        .iter()
        .filter_map(|value| value["lastError"].as_str().map(str::to_owned))
        .collect();
    for case in oracle()["malformed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| !case["error"].is_null())
    {
        assert!(
            errors
                .iter()
                .any(|error| error == case["error"]["message"].as_str().unwrap()),
            "{}",
            case["name"]
        );
    }
    session.live.stop().await;
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), websocket)
        .await
        .unwrap()
        .unwrap();
}
