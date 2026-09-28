//! 桌面出站工作区的真实 HTTP/TLS 集成，不使用用户档案、网络主机或真实凭据。
use super::*;
use crate::mobile::store::ServerEndpoint;
use rcgen::generate_simple_self_signed;
use rustls::pki_types::PrivatePkcs8KeyDer;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

type Requests = Arc<Mutex<Vec<String>>>;

struct Upstream {
    url: String,
    fingerprint: String,
    requests: Requests,
    finish_stream: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn receive<T: tokio::io::AsyncRead + Unpin>(stream: &mut T) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let mut block = [0u8; 4096];
        let read = stream.read(&mut block).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&block[..read]);
        if bytes.windows(4).any(|value| value == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        if bytes.len() > 65536 {
            return None;
        }
    }
}

async fn upstream(tls: bool, name: &'static str) -> Upstream {
    let certificate = generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let fingerprint = Sha256::digest(certificate.cert.der())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.cert.der().clone()],
        PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der()).into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "{}://{}",
        if tls { "https" } else { "http" },
        listener.local_addr().unwrap()
    );
    let requests: Requests = Arc::new(Mutex::new(vec![]));
    let seen = requests.clone();
    let finish_stream = Arc::new(Semaphore::new(0));
    let finish = finish_stream.clone();
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let seen = seen.clone();
            let finish = finish.clone();
            tokio::spawn(async move {
                if tls {
                    if let Ok(mut stream) = acceptor.accept(socket).await {
                        respond(&mut stream, seen, finish, name).await;
                    }
                } else {
                    let mut stream = socket;
                    respond(&mut stream, seen, finish, name).await;
                }
            });
        }
    });
    Upstream {
        url,
        fingerprint,
        requests,
        finish_stream,
        task,
    }
}

async fn respond<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut T,
    requests: Requests,
    finish: Arc<Semaphore>,
    name: &str,
) {
    let Some(request) = receive(stream).await else {
        return;
    };
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    requests.lock().unwrap().push(request);
    if path == "/api/chat" {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await;
        for (index, frame) in [
            "event: run\ndata: {\"runId\":\"remote-run\"}\n\n",
            "event: done\ndata: {}\n\n",
        ]
        .iter()
        .enumerate()
        {
            if index == 1 {
                let _ = finish.acquire().await;
            }
            let data = format!("{:x}\r\n{frame}\r\n", frame.len());
            if stream.write_all(data.as_bytes()).await.is_err() {
                return;
            }
            let _ = stream.flush().await;
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    } else {
        let body = format!("{{\"name\":\"{name}\",\"ok\":true}}");
        let data = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nSet-Cookie: malicious=upstream\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(data.as_bytes()).await;
    }
    let _ = stream.shutdown().await;
}

fn profile(server: &Upstream) -> ServerProfile {
    ServerProfile {
        id: "remote".into(),
        name: "Linux".into(),
        endpoints: vec![ServerEndpoint::lan(server.url.clone())],
        fingerprint: server.fingerprint.clone(),
        device_id: "test-device".into(),
        token: "test-remote-device-token".into(),
        paired_at: "0".into(),
    }
}
fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}
async fn connect(local: &Upstream, remote: &Upstream) -> (Arc<ProxyHandle>, String) {
    let proxy = start_desktop_proxy(
        &format!(
            "{}/_pisper/desktop/bootstrap?token=test-local-sidecar-token",
            local.url
        ),
        profile(remote),
    )
    .await
    .unwrap();
    let response = local_client()
        .get(proxy.bootstrap_url().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let cookie = response.headers()[reqwest::header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    (proxy, cookie)
}
fn url(proxy: &ProxyHandle, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", proxy.port)
}

#[tokio::test]
async fn page_state_requests_stay_on_the_mobile_runtime_in_remote_mode() {
    let local = upstream(false, "mobile-runtime").await;
    let remote = upstream(true, "desktop-runtime").await;
    let (proxy, cookie) = connect(&local, &remote).await;
    let client = local_client();
    for method in [reqwest::Method::GET, reqwest::Method::PUT] {
        let response = client
            .request(method, url(&proxy, "/api/local/browser-preferences"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["name"],
            "mobile-runtime"
        );
    }
    let local_requests = local.requests.lock().unwrap();
    assert!(local_requests
        .iter()
        .any(|request| request.starts_with("GET /api/local/browser-preferences ")));
    assert!(local_requests
        .iter()
        .any(|request| request.starts_with("PUT /api/local/browser-preferences ")));
    assert!(remote.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn desktop_authenticates_locally_serves_bundled_ui_and_sends_only_device_credentials_to_remote(
) {
    let local = upstream(false, "bundled-ui").await;
    let remote = upstream(true, "linux-runtime").await;
    let (proxy, cookie) = connect(&local, &remote).await;
    let client = local_client();
    assert_eq!(
        client
            .get(url(&proxy, "/api/sessions"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(url(&proxy, "/api/sessions"))
            .header("cookie", &cookie)
            .header("origin", "https://evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert!(remote.requests.lock().unwrap().is_empty());
    assert!(local.requests.lock().unwrap().is_empty());
    let frontend = client
        .get(url(&proxy, "/"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(frontend.headers().get("set-cookie").is_none());
    assert!(frontend.text().await.unwrap().contains("bundled-ui"));
    let response = client
        .get(url(&proxy, "/api/sessions"))
        .header("cookie", format!("{cookie}; user-secret=private"))
        .header("authorization", "Bearer caller-must-not-override")
        .header("referer", "http://private.invalid/?token=private")
        .header("x-pisper-client", "spoofed")
        .send()
        .await
        .unwrap();
    assert!(response.headers().get("set-cookie").is_none());
    assert!(response.text().await.unwrap().contains("linux-runtime"));
    let local_request = local.requests.lock().unwrap()[0].to_lowercase();
    assert!(local_request.contains("cookie: __pisper_desktop=test-local-sidecar-token"));
    assert!(!local_request.contains("user-secret"));
    for request in remote.requests.lock().unwrap().iter() {
        let request = request.to_lowercase();
        assert!(request.contains("authorization: bearer test-remote-device-token"));
        assert!(!request.contains("cookie:"));
        assert!(!request.contains("local-sidecar-token"));
        assert!(!request.contains("caller-must-not-override"));
        assert!(!request.contains("private.invalid"));
        if !request.starts_with("get /api/health ") {
            assert!(request.contains("x-pisper-client: desktop-remote"));
        }
    }
    proxy.shutdown();
}

#[tokio::test]
async fn desktop_streams_before_done_and_closing_stops_listener_and_inflight_stream() {
    let local = upstream(false, "local").await;
    let remote = upstream(true, "remote").await;
    let (proxy, cookie) = connect(&local, &remote).await;
    let mut response = local_client()
        .post(url(&proxy, "/api/chat"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("remote-run"));
    assert_eq!(remote.finish_stream.available_permits(), 0);
    proxy.shutdown();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(1), response.chunk())
            .await
            .unwrap(),
        Ok(Some(_))
    ));
    let port = proxy.port;
    let weak = Arc::downgrade(&proxy);
    drop(proxy);
    tokio::time::timeout(Duration::from_secs(1), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
    assert!(local.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn desktop_windows_are_fixed_to_their_remote_and_never_fall_back_to_local_api() {
    let local = upstream(false, "LOCAL_API_MUST_NOT_RUN").await;
    let remote_one = upstream(true, "linux-one").await;
    let remote_two = upstream(true, "linux-two").await;
    let (one, cookie_one) = connect(&local, &remote_one).await;
    let (two, cookie_two) = connect(&local, &remote_two).await;
    let client = local_client();
    assert_eq!(
        client
            .get(url(&two, "/api/work"))
            .header("cookie", &cookie_one)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    for (proxy, cookie, name) in [
        (&one, &cookie_one, "linux-one"),
        (&two, &cookie_two, "linux-two"),
    ] {
        let result = client
            .get(url(proxy, "/api/work"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(result.contains(name));
    }
    // 即便档案被清除，桌面 API 也必须明确失败而不是变成本机模式。
    one.store.lock().unwrap().forget("remote").unwrap();
    let failed = client
        .post(url(&one, "/api/chat"))
        .header("cookie", &cookie_one)
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(local.requests.lock().unwrap().is_empty());
    one.shutdown();
    assert!(client
        .get(url(&two, "/api/work"))
        .header("cookie", &cookie_two)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .contains("linux-two"));
    two.shutdown();
}

#[tokio::test]
async fn desktop_rejects_remote_certificate_mismatch_without_contacting_local_api() {
    let local = upstream(false, "LOCAL_API_MUST_NOT_RUN").await;
    let remote = upstream(true, "remote").await;
    let mut invalid = profile(&remote);
    invalid.fingerprint = "00".repeat(32);
    let proxy = start_desktop_proxy(
        &format!("{}/_pisper/desktop/bootstrap?token=local", local.url),
        invalid,
    )
    .await
    .unwrap();
    let client = local_client();
    let bootstrap = client
        .get(proxy.bootstrap_url().unwrap())
        .send()
        .await
        .unwrap();
    let cookie = bootstrap.headers()[reqwest::header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let result = client
        .get(url(&proxy, "/api/sessions"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(remote.requests.lock().unwrap().is_empty());
    assert!(local.requests.lock().unwrap().is_empty());
    proxy.shutdown();
}

#[tokio::test]
async fn desktop_rejects_noncanonical_paths_before_contacting_either_upstream() {
    let local = upstream(false, "local").await;
    let remote = upstream(true, "remote").await;
    let (proxy, cookie) = connect(&local, &remote).await;
    for path in [
        "/assets/../api/config",
        "/assets/%2e%2e/api/config",
        "/%2e/_pisper/shutdown",
    ] {
        // reqwest 本身会规范化路径，必须用实际原始 HTTP 请求覆盖代理边界。
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", proxy.port))
            .await
            .unwrap();
        socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n", proxy.port).as_bytes()).await.unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    }
    assert!(local.requests.lock().unwrap().is_empty());
    assert!(remote.requests.lock().unwrap().is_empty());
    proxy.shutdown();
}
