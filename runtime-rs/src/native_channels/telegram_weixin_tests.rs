//! 实际 HTTP 夹具与 release Node oracle，所有地址、凭据和媒体都由测试独占。
use super::super::{
    telegram::TelegramGateway, weixin::WeixinGateway, weixin_onboarding::WeixinOnboardingService,
    Gateway, GatewayCallbacks, Onboarding,
};
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Request},
    response::Response,
    routing::any,
    Router,
};
use futures::future::BoxFuture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::task::JoinHandle;

#[derive(Clone)]
struct Record {
    path: String,
    query: String,
    headers: HeaderMap,
    body: Vec<u8>,
}
type Handler = Arc<dyn Fn(Record) -> BoxFuture<'static, Response> + Send + Sync>;
struct Fixture {
    base: String,
    calls: Arc<Mutex<Vec<Record>>>,
    changed: Arc<Notify>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}
fn response(value: Value) -> Response {
    Response::builder()
        .header("Content-Type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}
impl Fixture {
    async fn new(handler: Handler) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let changed = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let state = (handler, calls.clone(), changed.clone());
        let app = Router::new().fallback(any(move |request: Request<Body>| {
            let (handler, calls, changed) = state.clone();
            async move {
                let uri = request.uri().clone();
                let headers = request.headers().clone();
                let bytes = to_bytes(request.into_body(), 32 * 1024 * 1024)
                    .await
                    .unwrap();
                let record = Record {
                    path: uri.path().to_owned(),
                    query: uri.query().unwrap_or("").to_owned(),
                    headers,
                    body: bytes.to_vec(),
                };
                calls.lock().unwrap().push(record.clone());
                changed.notify_waiters();
                handler(record).await
            }
        }));
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            base,
            calls,
            changed,
            cancel,
            task,
        }
    }
    async fn wait(&self, path: &str, count: usize) {
        loop {
            let changed = self.changed.notified();
            if self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.path == path)
                .count()
                >= count
            {
                return;
            }
            changed.await;
        }
    }
    fn records(&self, path: &str) -> Vec<Record> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .cloned()
            .collect()
    }
    async fn close(self) {
        self.cancel.cancel();
        self.task.abort();
        let result = self.task.await;
        assert!(result.is_ok() || result.unwrap_err().is_cancelled());
    }
}
fn callbacks(messages: Arc<Mutex<Vec<Value>>>, sync: Arc<Mutex<Vec<Value>>>) -> GatewayCallbacks {
    GatewayCallbacks {
        on_message: Arc::new(move |v| messages.lock().unwrap().push(v)),
        on_status: Arc::new(|_| {}),
        on_sync: Arc::new(move |v| sync.lock().unwrap().push(v)),
    }
}
fn oracle() -> Value {
    serde_json::from_str(include_str!("telegram_weixin_oracle.json")).unwrap()
}

// 网络驱动仍使用真实 loopback；由测试显式推进计时器，防止 Tokio 在 IO 唤醒前自动跳到超时。
fn manual_clock_driver() -> JoinHandle<()> {
    tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    })
}

#[test]
fn release_message_and_crypto_vectors_match() {
    let vector = oracle();
    assert_eq!(
        super::super::telegram::map_message(&vector["telegramRaw"]).unwrap(),
        vector["telegramMapped"]
    );
    assert_eq!(
        super::super::weixin::map_message(&vector["weixinRaw"]).unwrap(),
        vector["weixinMapped"]
    );
    assert!(
        super::super::telegram::map_message(&json!({"chat":{"id":1},"from":{"id":0}})).is_none()
    );
    assert!(
        super::super::weixin::map_message(&json!({"message_type":2,"from_user_id":"bot"}))
            .is_none()
    );
    let key = unhex("000102030405060708090a0b0c0d0e0f");
    let plain = unhex(vector["crypto"]["plaintextHex"].as_str().unwrap());
    let cipher = encrypt_aes_ecb(&plain, &key).unwrap();
    assert_eq!(hex(&cipher), vector["crypto"]["ciphertextHex"]);
    assert_eq!(decrypt_aes_ecb(&cipher, &key).unwrap(), plain);
    assert_eq!(parse_aes_key(&STANDARD.encode(&key)).unwrap(), key);
    assert_eq!(parse_aes_key(&STANDARD.encode(hex(&key))).unwrap(), key);
    assert!(parse_aes_key("wrong").is_err());
    assert!(decrypt_aes_ecb(&[1; 16], &key).is_err());
    assert_eq!(
        text_from_items(
            &json!([{"type":1,"text_item":{"text":" hello "}},{"type":3,"voice_item":{"text":"voice"}},{"type":"1","text_item":{"text":"ignored"}}])
        ),
        "hello \nvoice"
    );
}

#[tokio::test]
async fn telegram_bot_api_roundtrip_offset_multipart_download_and_cancel_join() {
    let polls = Arc::new(AtomicUsize::new(0));
    let polls_ = polls.clone();
    let fixture=Fixture::new(Arc::new(move|record|{let polls=polls_.clone();Box::pin(async move{
        match record.path.as_str(){
            "/bot12345:synthetic/getMe"=>response(json!({"ok":true,"result":{"id":42,"first_name":"Pisper","username":"pisper_bot"}})),
            "/bot12345:synthetic/getUpdates"=>{if polls.fetch_add(1,Ordering::SeqCst)==0{response(json!({"ok":true,"result":[{"update_id":11,"message":{"message_id":12,"chat":{"id":-100,"type":"supergroup"},"from":{"id":7,"first_name":"Ada"},"caption":"请看图","photo":[{"file_id":"small"},{"file_id":"large"}]}},{"update_id":12,"message":{"chat":{"id":1}}}]}))}else{std::future::pending().await}},
            "/bot12345:synthetic/sendMessage"=>{let body:Value=serde_json::from_slice(&record.body).unwrap();if body["text"]=="hold"{std::future::pending().await}else{response(json!({"ok":true,"result":{"message_id":22}}))}},
            "/bot12345:synthetic/sendPhoto"|"/bot12345:synthetic/sendDocument"=>response(json!({"ok":true,"result":{"message_id":22}})),
            "/bot12345:synthetic/getFile"=>response(json!({"ok":true,"result":{"file_path":"file.bin"}})),
            "/file/bot12345:synthetic/file.bin"=>Response::builder().header("Content-Type","image/gif").body(Body::from(vec![1,2,3,4])).unwrap(),
            _=>panic!("unexpected owned HTTP path {}",record.path),
        }
    })})).await;
    let messages = Arc::new(Mutex::new(Vec::new()));
    let gateway = TelegramGateway::new(callbacks(messages.clone(), Arc::default()));
    let status=gateway.connect(json!({"token":"12345:synthetic","baseUrl":fixture.base,"fileBaseUrl":format!("{}/file",fixture.base),"offset":3})).await.unwrap();
    assert_eq!(status["bot"]["id"], "42");
    fixture.wait("/bot12345:synthetic/getUpdates", 2).await;
    assert_eq!(messages.lock().unwrap().len(), 1);
    let polls = fixture.records("/bot12345:synthetic/getUpdates");
    assert_eq!(
        serde_json::from_slice::<Value>(&polls[0].body).unwrap(),
        json!({"offset":3,"timeout":30,"allowed_updates":["message"]})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&polls[1].body).unwrap()["offset"],
        13
    );
    gateway
        .send(
            json!({"peerId":"-100","messageId":"12"}),
            json!({"markdown":"x".repeat(4100)}),
        )
        .await
        .unwrap();
    let sent = fixture.records("/bot12345:synthetic/sendMessage");
    let sent: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(sent["text"].as_str().unwrap().len(), 4096);
    assert_eq!(sent["reply_parameters"]["message_id"], 12);
    let root =
        std::env::temp_dir().join(format!("pisper-native-telegram-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&root).await.unwrap();
    let path = root.join("fixture.png");
    tokio::fs::write(&path, [1, 2, 3, 4]).await.unwrap();
    for mime in ["image/png", "application/octet-stream"] {
        gateway
            .send_asset(
                "-100".into(),
                json!({"path":path,"name":"fixture.png","mimeType":mime}),
                json!({}),
            )
            .await
            .unwrap();
    }
    for (method, field) in [("sendPhoto", "photo"), ("sendDocument", "document")] {
        let records = fixture.records(&format!("/bot12345:synthetic/{method}"));
        assert!(records[0].headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8_lossy(&records[0].body);
        assert!(body.contains(&format!("name=\"{field}\"; filename=\"fixture.png\"")));
        assert!(records[0].body.windows(4).any(|v| v == [1, 2, 3, 4]));
    }
    let resources = gateway
        .download_resources(json!([{"fileId":"large","type":"image","name":"actual.gif"}]))
        .await
        .unwrap();
    assert_eq!(resources[0].bytes, [1, 2, 3, 4]);
    assert_eq!(resources[0].mime_type.as_deref(), Some("image/gif"));
    let sending = gateway.clone();
    let pending = tokio::spawn(async move {
        sending
            .send_to_peer("-100".into(), json!({"text":"hold"}), json!({}))
            .await
    });
    fixture.wait("/bot12345:synthetic/sendMessage", 2).await;
    gateway.disconnect().await.unwrap();
    assert!(pending.await.unwrap().is_err());
    assert_eq!(gateway.get_status()["state"], "idle");
    assert!(gateway
        .send_to_peer("x".into(), json!({}), json!({}))
        .await
        .is_err());
    tokio::fs::remove_dir_all(&root).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn weixin_real_headers_context_sync_media_encryption_and_gateway_join() {
    let upload_key = Arc::new(Mutex::new(Vec::new()));
    let key_ = upload_key.clone();
    let plaintext = b"real local media bytes".to_vec();
    let plain_ = plaintext.clone();
    let updates = Arc::new(AtomicUsize::new(0));
    let updates_ = updates.clone();
    let fixture=Fixture::new(Arc::new(move|record|{let key=key_.clone();let plain=plain_.clone();let updates=updates_.clone();Box::pin(async move{match record.path.as_str(){
        "/ilink/bot/msg/notifystart"|"/ilink/bot/msg/notifystop"|"/ilink/bot/sendmessage"=>response(json!({"ret":0})),
        "/ilink/bot/getupdates"=>if updates.fetch_add(1,Ordering::SeqCst)==0{response(json!({"ret":0,"longpolling_timeout_ms":12345,"get_updates_buf":"cursor-next","msgs":[{"message_id":9,"from_user_id":"wx-owner","context_token":"reply-context","item_list":[{"type":1,"text_item":{"text":"hello"}}]},{"message_type":2,"from_user_id":"bot"}]}))}else{std::future::pending().await},
        "/ilink/bot/getuploadurl"=>{let body:Value=serde_json::from_slice(&record.body).unwrap();assert_eq!(body["rawfilemd5"],hex(&Md5::digest(&plain)));assert_eq!(body["rawsize"],plain.len());assert_eq!(body["filesize"],padded_size(plain.len()));*key.lock().unwrap()=unhex(body["aeskey"].as_str().unwrap());response(json!({"upload_param":"upload param+/="}))},
        "/cdn/upload"=>{assert!(record.query.starts_with("encrypted_query_param=upload%20param%2B%2F%3D&filekey="));assert_eq!(decrypt_aes_ecb(&record.body,&key.lock().unwrap()).unwrap(),plain);Response::builder().header("x-encrypted-param","download-param").body(Body::empty()).unwrap()},
        "/cdn/download"=>{assert_eq!(record.query,"encrypted_query_param=download-param");Response::builder().body(Body::from(encrypt_aes_ecb(&plain,&key.lock().unwrap()).unwrap())).unwrap()},
        _=>panic!("unexpected owned HTTP path {}",record.path),
    }})})).await;
    let protocol = Arc::new(WeixinProtocol::with_endpoints(
        Client::new(),
        fixture.base.clone(),
        format!("{}/cdn", fixture.base),
    ));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let sync = Arc::new(Mutex::new(Vec::new()));
    let gateway =
        WeixinGateway::with_protocol(callbacks(messages.clone(), sync.clone()), protocol.clone());
    let connection = json!({"token":" synthetic-token ","baseUrl":fixture.base,"cdnBaseUrl":format!("{}/cdn",fixture.base),"syncBuf":"cursor-old"});
    gateway.connect(connection.clone()).await.unwrap();
    fixture.wait("/ilink/bot/getupdates", 2).await;
    assert_eq!(*sync.lock().unwrap(), [json!("cursor-next")]);
    assert_eq!(messages.lock().unwrap().len(), 1);
    let polls = fixture.records("/ilink/bot/getupdates");
    assert_eq!(
        serde_json::from_slice::<Value>(&polls[1].body).unwrap()["get_updates_buf"],
        "cursor-next"
    );
    gateway
        .send(messages.lock().unwrap()[0].clone(), json!({"text":"reply"}))
        .await
        .unwrap();
    gateway
        .send_to_peer(
            "wx-owner".into(),
            json!({"text":"notification"}),
            json!({"contextToken":"latest-persisted"}),
        )
        .await
        .unwrap();
    let sent = fixture.records("/ilink/bot/sendmessage");
    assert_eq!(
        serde_json::from_slice::<Value>(&sent[0].body).unwrap()["msg"]["context_token"],
        "reply-context"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&sent[1].body).unwrap()["msg"]["context_token"],
        "latest-persisted"
    );
    let root = std::env::temp_dir().join(format!("pisper-native-weixin-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&root).await.unwrap();
    let path = root.join("fixture.txt");
    tokio::fs::write(&path, &plaintext).await.unwrap();
    gateway
        .send_asset(
            "wx-owner".into(),
            json!({"path":path,"name":"fixture.txt"}),
            json!({"contextToken":"latest-persisted"}),
        )
        .await
        .unwrap();
    let sent = fixture.records("/ilink/bot/sendmessage");
    let sent: Value = serde_json::from_slice(&sent[2].body).unwrap();
    let item = &sent["msg"]["item_list"][0];
    assert_eq!(item["file_item"]["len"], plaintext.len().to_string());
    assert_eq!(item["file_item"]["media"]["encrypt_type"], 1);
    let resources = gateway.download_resources(json!([item])).await.unwrap();
    assert_eq!(resources[0].bytes, plaintext);
    assert_eq!(resources[0].mime_type.as_deref(), Some("text/plain"));
    for (mime, kind, size_field) in [("image/png", 2, "mid_size"), ("video/mp4", 5, "video_size")] {
        gateway
            .send_asset(
                "wx-owner".into(),
                json!({"path":path,"name":"fixture.bin","mimeType":mime}),
                json!({"contextToken":"latest-persisted"}),
            )
            .await
            .unwrap();
        let sent = fixture.records("/ilink/bot/sendmessage");
        let last: Value = serde_json::from_slice(&sent.last().unwrap().body).unwrap();
        let item = &last["msg"]["item_list"][0];
        assert_eq!(item["type"], kind);
        let object = if kind == 2 {
            &item["image_item"]
        } else {
            &item["video_item"]
        };
        assert_eq!(object[size_field], padded_size(plaintext.len()));
        assert_eq!(
            parse_aes_key(object["media"]["aes_key"].as_str().unwrap()).unwrap(),
            *upload_key.lock().unwrap()
        );
    }
    for record in fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.starts_with("/ilink/"))
    {
        assert_eq!(record.headers["ilink-app-id"], "bot");
        assert_eq!(record.headers["ilink-app-clientversion"], "132102");
        assert_eq!(record.headers["authorizationtype"], "ilink_bot_token");
        assert_eq!(record.headers["authorization"], "Bearer synthetic-token");
        let uin = STANDARD
            .decode(record.headers["x-wechat-uin"].as_bytes())
            .unwrap();
        assert!(uin.iter().all(u8::is_ascii_digit));
        assert_eq!(
            serde_json::from_slice::<Value>(&record.body).unwrap()["base_info"],
            json!({"channel_version":"2.4.6","bot_agent":"Pisper/0.0.0"})
        );
    }
    gateway.disconnect().await.unwrap();
    assert_eq!(fixture.records("/ilink/bot/msg/notifystop").len(), 1);
    assert_eq!(gateway.get_status()["state"], "idle");
    tokio::fs::remove_dir_all(&root).await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn weixin_expired_context_and_http_errors_remain_actionable() {
    let fixture = Fixture::new(Arc::new(|_| {
        Box::pin(async { response(json!({"ret":-2,"errmsg":"prepare failed"})) })
    }))
    .await;
    let protocol =
        WeixinProtocol::with_endpoints(Client::new(), fixture.base.clone(), fixture.base.clone());
    let result = protocol
        .send_text(
            &json!({"baseUrl":fixture.base,"token":"synthetic"}),
            json!("owner"),
            json!("message"),
            json!("expired"),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(result.message, oracle()["expiredContextError"]);
    fixture.close().await;
}

#[tokio::test]
async fn weixin_onboarding_pairing_confirmation_cancel_and_public_credentials() {
    let polls = Arc::new(AtomicUsize::new(0));
    let polls_ = polls.clone();
    let fixture=Fixture::new(Arc::new(move|record|{let polls=polls_.clone();Box::pin(async move{match record.path.as_str(){
        "/ilink/bot/get_bot_qrcode"=>response(json!({"qrcode":"qr-id","qrcode_img_content":"https://fixture.invalid/qr"})),
        "/ilink/bot/get_qrcode_status"=>{assert!(!record.headers.contains_key("authorization"));assert!(!record.headers.contains_key("authorizationtype"));let result=match polls.fetch_add(1,Ordering::SeqCst){0=>json!({"status":"need_verifycode"}),_=>{assert!(record.query.ends_with("verify_code=123456"));json!({"status":"confirmed","bot_token":"synthetic-private-token","ilink_bot_id":"bot","ilink_user_id":"owner"})}};response(result)},
        _=>panic!("unexpected QR path"),
    }})})).await;
    let complete = Arc::new(Mutex::new(Vec::new()));
    let captured = complete.clone();
    let sink: super::super::CompletedSink = Arc::new(move |value| {
        captured.lock().unwrap().push(value);
        Box::pin(async { Ok(json!({"connected":true})) })
    });
    let service = WeixinOnboardingService::with_protocol(
        sink,
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            format!("{}/cdn", fixture.base),
        )),
    );
    let job = service
        .start(json!({"localTokens":(0..12).map(|i|i.to_string()).collect::<Vec<_>>()}))
        .await
        .unwrap();
    let id = job["id"].as_str().unwrap();
    assert_eq!(job["status"], "waiting");
    assert!(job["qrDataUrl"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    fixture.wait("/ilink/bot/get_qrcode_status", 1).await;
    loop {
        if service.get(id).unwrap()["status"] == "verification_required" {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(service.verify(id, json!("abc")).is_err());
    service.verify(id, json!(" 123456 ")).unwrap();
    loop {
        let value = service.get(id).unwrap();
        if value["status"] == "completed" {
            assert!(!value.to_string().contains("synthetic-private-token"));
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        complete.lock().unwrap()[0]["token"],
        "synthetic-private-token"
    );
    assert_eq!(complete.lock().unwrap()[0]["baseUrl"], fixture.base);
    let start = fixture.records("/ilink/bot/get_bot_qrcode");
    assert_eq!(
        serde_json::from_slice::<Value>(&start[0].body).unwrap()["local_token_list"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert!(!start[0].headers.contains_key("authorization"));
    assert_eq!(start[0].headers["authorizationtype"], "ilink_bot_token");
    assert!(service.cancel(id));
    assert_eq!(service.get(id).unwrap()["status"], "cancelled");
    service.dispose().await.unwrap();
    assert!(service.get(id).is_none());
    fixture.close().await;
}

#[tokio::test(start_paused = true)]
async fn weixin_onboarding_pairing_wait_expires_with_fake_clock() {
    let clock_driver = manual_clock_driver();
    let fixture = Fixture::new(Arc::new(|record| {
        Box::pin(async move {
            response(if record.path.ends_with("get_bot_qrcode") {
                json!({"qrcode":"qr-id","qrcode_img_content":"https://fixture.invalid/qr"})
            } else {
                json!({"status":"need_verifycode"})
            })
        })
    }))
    .await;
    let sink: super::super::CompletedSink =
        Arc::new(|_| Box::pin(async { panic!("Expired job must never complete") }));
    let service = WeixinOnboardingService::with_protocol(
        sink,
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            fixture.base.clone(),
        )),
    );
    let job = service.start(json!({})).await.unwrap();
    let id = job["id"].as_str().unwrap();
    fixture.wait("/ilink/bot/get_qrcode_status", 1).await;
    loop {
        if service.get(id).unwrap()["status"] == "verification_required" {
            break;
        }
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(301)).await;
    loop {
        let state = service.get(id).unwrap();
        if state["status"] == "failed" {
            assert!(state["error"].as_str().unwrap().contains("已超时"));
            break;
        }
        tokio::task::yield_now().await;
    }
    service.dispose().await.unwrap();
    fixture.close().await;
    clock_driver.abort();
    assert!(clock_driver.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn telegram_reconnect_backoff_and_status_redaction_use_fake_clock() {
    let clock_driver = manual_clock_driver();
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_ = attempts.clone();
    let fixture = Fixture::new(Arc::new(move |record| {
        let attempts = attempts_.clone();
        Box::pin(async move {
            if record.path.ends_with("getMe") {
                response(json!({"ok":true,"result":{"id":42}}))
            } else {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                response(json!({"ok":false,"description":format!("failure{attempt} /bot12345:synthetic")}))
            }
        })
    })).await;
    let gateway = TelegramGateway::new(callbacks(Arc::default(), Arc::default()));
    gateway
        .connect(json!({"token":"12345:synthetic","baseUrl":fixture.base}))
        .await
        .unwrap();
    for attempt in 1..=3 {
        fixture
            .wait("/bot12345:synthetic/getUpdates", attempt)
            .await;
        loop {
            if gateway.get_status()["lastError"]
                .as_str()
                .unwrap()
                .contains(&format!("failure{attempt}"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!gateway.get_status().to_string().contains("12345:synthetic"));
        if attempt < 3 {
            tokio::time::advance(Duration::from_secs((attempt * 2) as u64)).await;
        }
    }
    assert_eq!(gateway.get_status()["state"], "reconnecting");
    let now = tokio::time::Instant::now();
    gateway.disconnect().await.unwrap();
    assert_eq!(tokio::time::Instant::now(), now);
    fixture.close().await;
    clock_driver.abort();
    assert!(clock_driver.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn weixin_reconnect_backoff_and_cancel_join_use_fake_clock() {
    let clock_driver = manual_clock_driver();
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_ = attempts.clone();
    let fixture = Fixture::new(Arc::new(move |record| {
        let attempts = attempts_.clone();
        Box::pin(async move {
            if record.path.ends_with("getupdates") {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                response(json!({"ret":7,"errmsg":format!("failure{attempt}")}))
            } else {
                response(json!({"ret":0}))
            }
        })
    }))
    .await;
    let gateway = WeixinGateway::with_protocol(
        callbacks(Arc::default(), Arc::default()),
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            fixture.base.clone(),
        )),
    );
    gateway
        .connect(json!({"token":"synthetic","baseUrl":fixture.base}))
        .await
        .unwrap();
    for attempt in 1..=3 {
        fixture.wait("/ilink/bot/getupdates", attempt).await;
        loop {
            if gateway.get_status()["lastError"] == format!("failure{attempt}") {
                break;
            }
            tokio::task::yield_now().await;
        }
        if attempt < 3 {
            tokio::time::advance(Duration::from_secs(2)).await;
        }
    }
    assert_eq!(gateway.get_status()["state"], "reconnecting");
    gateway.disconnect().await.unwrap();
    assert_eq!(gateway.get_status()["state"], "idle");
    fixture.close().await;
    clock_driver.abort();
    assert!(clock_driver.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn weixin_resource_count_and_24mb_total_limits_follow_release() {
    let fixture = Fixture::new(Arc::new(|record| {
        Box::pin(async move {
            if record.path.ends_with("getupdates") {
                std::future::pending().await
            } else if record.path == "/cdn/download" {
                Response::builder()
                    .body(Body::from(if record.query.contains("large") {
                        vec![42; 13 * 1024 * 1024]
                    } else {
                        vec![42]
                    }))
                    .unwrap()
            } else {
                response(json!({"ret":0}))
            }
        })
    }))
    .await;
    let gateway = WeixinGateway::with_protocol(
        callbacks(Arc::default(), Arc::default()),
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            format!("{}/cdn", fixture.base),
        )),
    );
    gateway
        .connect(json!({"token":"synthetic","baseUrl":fixture.base}))
        .await
        .unwrap();
    let small = json!({"type":4,"file_item":{"file_name":"fixture.bin","media":{"encrypt_query_param":"small"}}});
    let items = Value::Array(vec![small; 9]);
    assert_eq!(gateway.download_resources(items).await.unwrap().len(), 8);
    let large = json!({"type":4,"file_item":{"file_name":"fixture.bin","media":{"encrypt_query_param":"large"}}});
    let failure = gateway
        .download_resources(json!([large, large]))
        .await
        .err()
        .unwrap();
    assert_eq!(failure.message, "微信附件总大小超过 24 MB。");
    gateway.disconnect().await.unwrap();
    fixture.close().await;
}

#[tokio::test(start_paused = true)]
async fn weixin_qr_refreshes_twice_then_fails_and_cancel_joins_pending_poll() {
    let clock_driver = manual_clock_driver();
    let starts = Arc::new(AtomicUsize::new(0));
    let starts_ = starts.clone();
    let fixture = Fixture::new(Arc::new(move |record| {
        let starts=starts_.clone();Box::pin(async move {
            if record.path.ends_with("get_bot_qrcode") {
                let n=starts.fetch_add(1,Ordering::SeqCst)+1;
                response(json!({"qrcode":format!("qr-{n}"),"qrcode_img_content":format!("https://fixture.invalid/qr-{n}")}))
            } else { response(json!({"status":"expired"})) }
        })
    })).await;
    let completed: super::super::CompletedSink =
        Arc::new(|_| Box::pin(async { panic!("Expired QR must not complete") }));
    let service = WeixinOnboardingService::with_protocol(
        completed,
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            fixture.base.clone(),
        )),
    );
    let job = service.start(json!({})).await.unwrap();
    let id = job["id"].as_str().unwrap();
    for count in 2..=3 {
        fixture.wait("/ilink/bot/get_bot_qrcode", count).await;
        loop {
            if service.get(id).unwrap()["qrUrl"] == format!("https://fixture.invalid/qr-{count}") {
                break;
            }
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_millis(800)).await;
    }
    loop {
        let job = service.get(id).unwrap();
        if job["status"] == "failed" {
            assert_eq!(job["error"], "微信二维码已多次过期，请重新开始。");
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(starts.load(Ordering::SeqCst), 3);
    service.dispose().await.unwrap();
    fixture.close().await;
    let fixture=Fixture::new(Arc::new(|record|Box::pin(async move{if record.path.ends_with("get_bot_qrcode"){response(json!({"qrcode":"owned-pending","qrcode_img_content":"https://fixture.invalid/owned-pending"}))}else{std::future::pending().await}}))).await;
    let completed: super::super::CompletedSink =
        Arc::new(|_| Box::pin(async { panic!("Cancelled QR must not complete") }));
    let service = WeixinOnboardingService::with_protocol(
        completed,
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            fixture.base.clone(),
        )),
    );
    let job = service.start(json!({})).await.unwrap();
    let id = job["id"].as_str().unwrap();
    fixture.wait("/ilink/bot/get_qrcode_status", 1).await;
    assert!(service.cancel(id));
    assert_eq!(service.get(id).unwrap()["status"], "cancelled");
    service.dispose().await.unwrap();
    fixture.close().await;
    clock_driver.abort();
    assert!(clock_driver.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn weixin_protocol_timeout_and_abort_boundaries_follow_release() {
    let clock_driver = manual_clock_driver();
    let fixture = Fixture::new(Arc::new(|_| {
        Box::pin(async { std::future::pending().await })
    }))
    .await;
    let protocol = Arc::new(WeixinProtocol::with_endpoints(
        Client::new(),
        fixture.base.clone(),
        fixture.base.clone(),
    ));
    let owned = protocol.clone();
    let connection = json!({"baseUrl":fixture.base,"token":"synthetic"});
    let connection_ = connection.clone();
    let starting = tokio::spawn(async move {
        owned
            .notify_start(&connection_, &CancellationToken::new())
            .await
    });
    fixture.wait("/ilink/bot/msg/notifystart", 1).await;
    tokio::time::advance(Duration::from_millis(14999)).await;
    tokio::task::yield_now().await;
    assert!(!starting.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        starting.await.unwrap().unwrap_err().message,
        "渠道请求已超时。"
    );
    let owned = protocol.clone();
    let connection_ = connection.clone();
    let polling = tokio::spawn(async move {
        owned
            .get_updates(
                &connection_,
                &json!("unchanged"),
                &CancellationToken::new(),
                35000,
            )
            .await
    });
    fixture.wait("/ilink/bot/getupdates", 1).await;
    tokio::time::advance(Duration::from_millis(34999)).await;
    tokio::task::yield_now().await;
    assert!(!polling.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        polling.await.unwrap().unwrap(),
        json!({"ret":0,"msgs":[],"get_updates_buf":"unchanged"})
    );
    let owned = protocol.clone();
    let base = fixture.base.clone();
    let polling = tokio::spawn(async move {
        owned
            .poll_qr("qr-owned", &base, "123456", &CancellationToken::new())
            .await
    });
    fixture.wait("/ilink/bot/get_qrcode_status", 1).await;
    tokio::time::advance(Duration::from_secs(35)).await;
    assert_eq!(polling.await.unwrap().unwrap(), json!({"status":"wait"}));
    let poll = fixture.records("/ilink/bot/get_qrcode_status");
    assert_eq!(poll[0].query, "qrcode=qr-owned&verify_code=123456");
    assert!(!poll[0].headers.contains_key("content-type"));
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let owned = protocol.clone();
    let polling = tokio::spawn(async move {
        owned
            .get_updates(&connection, &json!("aborted-buffer"), &token, 35000)
            .await
    });
    fixture.wait("/ilink/bot/getupdates", 2).await;
    cancellation.cancel();
    assert_eq!(
        polling.await.unwrap().unwrap(),
        json!({"ret":0,"msgs":[],"get_updates_buf":"aborted-buffer"})
    );
    fixture.close().await;
    clock_driver.abort();
    assert!(clock_driver.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn weixin_disconnect_cancels_and_joins_an_active_encrypted_upload() {
    let fixture = Fixture::new(Arc::new(|record| {
        Box::pin(async move {
            match record.path.as_str() {
                "/ilink/bot/getupdates" | "/cdn/upload" => std::future::pending().await,
                "/ilink/bot/getuploadurl" => response(json!({"upload_param":"controlled-upload"})),
                _ => response(json!({"ret":0})),
            }
        })
    }))
    .await;
    let gateway = WeixinGateway::with_protocol(
        callbacks(Arc::default(), Arc::default()),
        Arc::new(WeixinProtocol::with_endpoints(
            Client::new(),
            fixture.base.clone(),
            format!("{}/cdn", fixture.base),
        )),
    );
    gateway
        .connect(json!({"token":"synthetic","baseUrl":fixture.base}))
        .await
        .unwrap();
    let root = std::env::temp_dir().join(format!(
        "pisper-native-weixin-upload-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir(&root).await.unwrap();
    let path = root.join("fixture.bin");
    tokio::fs::write(&path, [1, 2, 3, 4]).await.unwrap();
    let sender = gateway.clone();
    let pending = tokio::spawn(async move {
        sender
            .send_asset(
                "owner".into(),
                json!({"path":path}),
                json!({"contextToken":"owned-context"}),
            )
            .await
    });
    fixture.wait("/cdn/upload", 1).await;
    gateway.disconnect().await.unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap_err().message,
        "渠道请求已取消。"
    );
    assert_eq!(gateway.get_status()["state"], "idle");
    tokio::fs::remove_dir_all(root).await.unwrap();
    fixture.close().await;
}
