use super::*;
#[path = "test_support.rs"]
mod support;
use axum::{extract::State, http::Uri, Json, Router};
#[test]
fn qq_aes_gcm_matches_pinned_actual_connector_and_rejects_tampered_authentication() {
    let oracle: Value = serde_json::from_str(include_str!("sdk-oracle.json")).unwrap();
    let key = STANDARD
        .decode(oracle["crypto"]["key"].as_str().unwrap())
        .unwrap();
    let encoded = oracle["crypto"]["encrypted"].as_str().unwrap();
    assert_eq!(
        decrypt_secret(encoded, &key).unwrap(),
        oracle["crypto"]["plain"]
    );
    let mut damaged = STANDARD.decode(encoded).unwrap();
    *damaged.last_mut().unwrap() ^= 1;
    assert!(decrypt_secret(&STANDARD.encode(damaged), &key).is_err());
}
#[derive(Clone)]
struct HttpState {
    key: Arc<Mutex<Vec<u8>>>,
    calls: Arc<Mutex<usize>>,
}
async fn http(State(state): State<HttpState>, uri: Uri, Json(body): Json<Value>) -> Json<Value> {
    if uri.path().ends_with("create_bind_task") {
        *state.key.lock().unwrap() = STANDARD.decode(body["key"].as_str().unwrap()).unwrap();
        Json(json!({"retcode":0,"data":{"task_id":"task-fixture"}}))
    } else {
        let mut calls = state.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            return Json(json!({"retcode":0,"data":{"status":1}}));
        }
        let key = state.key.lock().unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [7u8; 12];
        let cipher = cipher
            .encrypt(Nonce::from_slice(&nonce), b"synthetic-secret".as_slice())
            .unwrap();
        let mut bytes = nonce.to_vec();
        bytes.extend(cipher);
        Json(
            json!({"retcode":0,"data":{"status":2,"bot_appid":"synthetic-app","bot_encrypt_secret":STANDARD.encode(bytes),"user_openid":"owner"}}),
        )
    }
}
#[tokio::test]
async fn real_qq_qr_bind_poll_decrypt_complete_and_dispose_are_loopback_only() {
    let mut server = support::Server::new(Router::new().fallback(http).with_state(HttpState {
        key: Arc::new(Mutex::new(Vec::new())),
        calls: Arc::new(Mutex::new(0)),
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
            api: server.url.clone(),
            page: format!("{}/connect", server.url),
            poll_interval: Duration::from_millis(1),
        },
    );
    let job = service.start(Value::Null).await.unwrap();
    assert_eq!(job["status"], "waiting");
    assert!(job["qrDataUrl"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    assert!(job["qrUrl"].as_str().unwrap().contains("source=pisper"));
    completed.wait(1).await;
    assert_eq!(
        completed.values.lock().unwrap()[0],
        json!({"appId":"synthetic-app","appSecret":"synthetic-secret","ownerUserId":"owner"})
    );
    let id = job["id"].as_str().unwrap();
    assert!(!service
        .get(id)
        .unwrap()
        .to_string()
        .contains("synthetic-secret"));
    service.dispose().await.unwrap();
    assert!(service.get(id).is_none());
    server.close().await;
}
