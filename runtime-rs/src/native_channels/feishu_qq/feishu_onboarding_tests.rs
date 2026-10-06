use super::*;
#[path = "test_support.rs"]
mod support;
use axum::{extract::State, http::Uri, Form, Json, Router};
#[derive(Clone)]
struct HttpState {
    calls: Arc<support::Recorder>,
    page: String,
}
async fn http(
    State(state): State<HttpState>,
    uri: Uri,
    Form(body): Form<HashMap<String, String>>,
) -> Json<Value> {
    state.calls.push(json!({"path":uri.path(),"body":body}));
    if body["action"] == "begin" {
        Json(
            json!({"verification_uri_complete":format!("{}/confirm?code=fixture",state.page),"device_code":"synthetic-device","expires_in":600,"interval":0}),
        )
    } else if uri.path().starts_with("/lark/") {
        Json(
            json!({"client_id":"cli_0123456789abcdef","client_secret":"synthetic-secret","user_info":{"open_id":"owner","tenant_brand":"lark"}}),
        )
    } else {
        Json(json!({"error":"authorization_pending","user_info":{"tenant_brand":"lark"}}))
    }
}
#[tokio::test]
async fn feishu_registration_qr_addons_device_poll_and_lark_switch_match_sdk() {
    let calls = Arc::new(support::Recorder::default());
    let mut server = support::Server::new(Router::new().fallback(http).with_state(HttpState {
        calls: calls.clone(),
        page: "http://127.0.0.1".into(),
    }))
    .await;
    let completed = Arc::new(support::Recorder::default());
    let received = completed.clone();
    let service = new_with_endpoints(
        Arc::new(move |value| {
            received.push(value);
            Box::pin(async { Ok(json!({"connections":[]})) })
        }),
        Endpoints {
            feishu: server.url.clone(),
            lark: format!("{}/lark", server.url),
        },
    );
    let job = service.start(Value::Null).await.unwrap();
    let url = reqwest::Url::parse(job["qrUrl"].as_str().unwrap()).unwrap();
    let params = url.query_pairs().collect::<HashMap<_, _>>();
    assert_eq!(params["source"], "node-sdk/pisper");
    assert_eq!(params["createOnly"], "true");
    let compressed = URL_SAFE_NO_PAD.decode(params["addons"].as_bytes()).unwrap();
    let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut text = String::new();
    std::io::Read::read_to_string(&mut decoder, &mut text).unwrap();
    let addons: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        addons["events"]["items"]["tenant"],
        json!(["im.message.receive_v1"])
    );
    assert_eq!(addons["scopes"]["tenant"].as_array().unwrap().len(), 4);
    completed.wait(1).await;
    assert_eq!(completed.values.lock().unwrap()[0]["domain"], "lark");
    let requests = calls.values.lock().unwrap();
    assert_eq!(requests[0]["body"]["archetype"], "PersonalAgent");
    assert!(requests
        .iter()
        .any(|call| call["path"] == "/lark/oauth/v1/app/registration"));
    drop(requests);
    service.dispose().await.unwrap();
    server.close().await;
}
