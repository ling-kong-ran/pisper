use super::*;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures::StreamExt;
use pi_rust::coding_agent::extensions::{
    loader::ExtensionRuntime,
    runner::ExtensionRunner,
    types::{AbortSignal, AgentToolUpdateCallbackValue, NoopProviderRegistry},
};
use reqwest::Url;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[path = "proof_tests.rs"]
mod proof;

const RSS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"><channel>
<item><title>Pisper &amp; Agent</title><link>https://example.com/pisper</link><description>An open agent.</description><pubDate>Tue, 21 Jul 2026 03:14:00 GMT</pubDate></item>
<item><title>Documentation</title><link>https://example.com/docs</link><description><![CDATA[Official <b>docs</b>]]></description></item>
<item><title>Unsafe</title><link>javascript:alert(1)</link><description>discard me</description></item>
</channel></rss>"#;

#[test]
fn generated_release_oracle_matches_config_and_rss_cases() {
    let oracle: Value = serde_json::from_str(include_str!("release-oracle.json")).unwrap();
    assert_eq!(
        oracle["sourceCommit"],
        "582160235671903d9f1c7034b457557b1df74b68"
    );
    for row in oracle["configs"].as_array().unwrap() {
        assert_eq!(
            normalize_config(&row["input"]),
            row["expected"],
            "{}",
            row["input"]
        );
    }
    let parsed = parse_bing_rss_results(oracle["rss"]["xml"].as_str().unwrap(), 8).unwrap();
    assert_eq!(
        serde_json::to_value(parsed).unwrap(),
        oracle["rss"]["expected"]
    );
}

#[derive(Clone)]
struct Recorded {
    path: String,
    params: HashMap<String, String>,
    headers: HeaderMap,
}
#[derive(Clone)]
struct ServerState {
    requests: Arc<Mutex<Vec<Recorded>>>,
    shutdown: CancellationToken,
    arrived: Arc<tokio::sync::Notify>,
}
struct Fixture {
    directory: PathBuf,
    state: ServerState,
    endpoint: Url,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("pisper-web-search-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{}/search", listener.local_addr().unwrap())).unwrap();
        let state = ServerState {
            requests: Arc::new(Mutex::new(Vec::new())),
            shutdown: CancellationToken::new(),
            arrived: Arc::new(tokio::sync::Notify::new()),
        };
        let router = Router::new()
            .route("/search", get(fixture_response))
            .route("/redirect", get(fixture_response))
            .with_state(state.clone());
        let shutdown = state.shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            directory,
            state,
            endpoint,
            task: Some(task),
        }
    }
    fn service(&self) -> Arc<WebSearchService> {
        WebSearchService::fixture(
            self.directory.join("pisper.json"),
            self.endpoint.clone(),
            Duration::from_secs(15),
        )
    }
    fn last(&self) -> Recorded {
        self.state.requests.lock().unwrap().last().unwrap().clone()
    }
    async fn close(mut self) {
        self.state.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.task.take().unwrap())
            .await
            .expect("fixture shutdown")
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if self.directory.parent() == Some(std::env::temp_dir().as_path())
            && self
                .directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-web-search-")
        {
            std::fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}
async fn fixture_response(State(state): State<ServerState>, request: Request) -> Response {
    let url = Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
    let params = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<HashMap<_, _>>();
    state.requests.lock().unwrap().push(Recorded {
        path: url.path().into(),
        params: params.clone(),
        headers: request.headers().clone(),
    });
    state.arrived.notify_one();
    if url.path() == "/redirect" {
        return (StatusCode::FOUND, [("location", "/search?q=redirected")]).into_response();
    }
    match params.get("q").map(String::as_str) {
        Some("stall") => {
            state.shutdown.cancelled().await;
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Some("body-stall") => {
            let shutdown = state.shutdown.clone();
            Body::from_stream(
                futures::stream::iter([Ok::<_, std::convert::Infallible>("<rss>")]).chain(
                    futures::stream::once(async move {
                        shutdown.cancelled().await;
                        Ok::<_, std::convert::Infallible>("<rss></rss>")
                    }),
                ),
            )
            .into_response()
        }
        Some("error") => (
            StatusCode::SERVICE_UNAVAILABLE,
            "<b>Maintenance</b> &amp; retry",
        )
            .into_response(),
        Some("empty-error") => StatusCode::FORBIDDEN.into_response(),
        Some("non-rss") => "<html>Temporarily unavailable</html>".into_response(),
        Some("empty") => "<rss><channel/></rss>".into_response(),
        _ => ([("content-type", "text/xml")], RSS).into_response(),
    }
}

#[test]
fn config_coercion_matches_js_number_round_truthiness_and_utf16() {
    assert_eq!(
        normalize_config(&json!({})),
        json!({"provider":"bing","language":"auto","safeSearch":1,"maxResults":8})
    );
    for (value, safe, max) in [
        (Value::Null, 0, 1),
        (json!(true), 1, 1),
        (json!(false), 0, 1),
        (json!(""), 0, 1),
        (json!(" 1.5 "), 2, 2),
        (json!("0x9"), 2, 9),
        (json!("0b10"), 2, 2),
        (json!("0o12"), 2, 10),
        (json!([]), 0, 1),
        (json!(["6.4"]), 2, 6),
        (json!([null]), 0, 1),
        (json!({}), 1, 8),
        (json!("1,2"), 1, 8),
        (json!("Infinity"), 1, 8),
        (json!("invalid"), 1, 8),
        (json!(-0.5), 0, 1),
        (json!(20), 2, 12),
    ] {
        let result = normalize_config(&json!({"safeSearch":value,"maxResults":value}));
        assert_eq!(result["safeSearch"], safe, "{value}");
        assert_eq!(result["maxResults"], max, "{value}");
    }
    for (input, expected) in [
        (json!(null), "auto"),
        (json!(false), "auto"),
        (json!([]), "auto"),
        (json!(["zh-CN", "en-US"]), "zh-CN,en-US"),
        (json!({}), "[object Object]"),
        (json!("\u{feff}ko-KR\u{a0}"), "ko-KR"),
        (json!("\u{85}ko-KR\u{85}"), "\u{85}ko-KR\u{85}"),
        (json!(1e21), "1e+21"),
        (json!(0.0000001), "1e-7"),
    ] {
        assert_eq!(
            normalize_config(&json!({"language":input}))["language"],
            expected
        );
    }
    assert_eq!(
        normalize_config(&json!({"language":format!("{}😀","a".repeat(39))}))["language"],
        format!("{}�", "a".repeat(39))
    );
    assert_eq!(
        normalize_config(&json!({"language":"😀".repeat(21)}))["language"],
        "😀".repeat(20)
    );
}

#[test]
fn rss_parser_and_output_follow_release_order_entities_and_url_rules() {
    let results = parse_bing_rss_results(RSS, 10).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0],
        SearchItem {
            title: "Pisper & Agent".into(),
            url: "https://example.com/pisper".into(),
            snippet: "An open agent.".into(),
            published_at: "Tue, 21 Jul 2026 03:14:00 GMT".into()
        }
    );
    assert_eq!(results[1].snippet, "Official docs");
    let unusual = r#"<rss><ITEM><title></title><link>HTTPS://EXAMPLE.COM:443/a b</link><description>&amp;lt;b&amp;gt;Hi&amp;lt;/b&amp;gt; &nbsp; &#x1F600; &#39;</description></ITEM><item><link>file:///unsafe</link></item></rss>"#;
    let unusual = parse_bing_rss_results(unusual, 8).unwrap();
    assert_eq!(unusual.len(), 1);
    assert_eq!(unusual[0].title, "example.com");
    assert_eq!(unusual[0].url, "https://example.com/a%20b");
    assert_eq!(unusual[0].snippet, "Hi 😀 '");
    assert!(parse_bing_rss_results(
        &format!("{RSS}<item><link>https://example.com</link><title>&#1114112;</title></item>"),
        1
    )
    .unwrap_err()
    .message
    .contains("Invalid code point"));
    assert_eq!(
        super::rss::result_text("none", &[]),
        "Bing 没有找到“none”的结果。"
    );
    assert_eq!(super::rss::result_text("query",&results),"Bing 搜索结果：query\n\n1. Pisper & Agent\nhttps://example.com/pisper\nAn open agent.\n发布时间：Tue, 21 Jul 2026 03:14:00 GMT\n\n2. Documentation\nhttps://example.com/docs\nOfficial docs");
}

#[tokio::test]
async fn real_http_builds_bing_query_headers_and_pagination() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let result = service
        .search(
            &json!({"query":"  Pisper agent  ","language":"en-US","page":2,"limit":2}),
            Some(&json!({"safeSearch":2})),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let request = fixture.last();
    assert_eq!(request.path, "/search");
    assert_eq!(
        request.params,
        HashMap::from([
            ("q".into(), "Pisper agent".into()),
            ("format".into(), "rss".into()),
            ("count".into(), "2".into()),
            ("adlt".into(), "strict".into()),
            ("first".into(), "3".into()),
            ("mkt".into(), "en-US".into()),
            ("setlang".into(), "en".into())
        ])
    );
    assert_eq!(
        request.headers["accept"],
        "application/rss+xml,application/xml,text/xml"
    );
    assert_eq!(request.headers["user-agent"], "Pisper Web Search/1.0");
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.query, "Pisper agent");
    for (language, mkt, setlang) in [
        ("zh-CN", "zh-CN", "zh"),
        ("zh-TW", "zh-TW", "zh-Hant"),
        ("en-GB", "en-GB", "en"),
        ("ja-JP", "ja-JP", "ja"),
        ("ko-KR", "ko-KR", "ko"),
    ] {
        service
            .search(
                &json!({"query":"market","language":language}),
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let request = fixture.last();
        assert_eq!(request.params["mkt"], mkt);
        assert_eq!(request.params["setlang"], setlang);
        assert!(!request.params.contains_key("first"));
    }
    service
        .search(
            &json!({"query":"auto","language":"auto","limit":12,"page":20}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let request = fixture.last();
    assert!(!request.params.contains_key("mkt"));
    assert_eq!(request.params["first"], "229");
    fixture.close().await;
}

#[tokio::test]
async fn canonical_config_is_read_fresh_without_changing_unknown_fields() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let path = fixture.directory.join("pisper.json");
    let original = json!({"unknown":{"preserved":true},"webSearch":{"language":"zh-CN","safeSearch":0,"maxResults":6,"future":"preserved"}});
    tokio::fs::write(&path, serde_json::to_vec(&original).unwrap())
        .await
        .unwrap();
    service
        .search(&json!({"query":"config"}), None, CancellationToken::new())
        .await
        .unwrap();
    let request = fixture.last();
    assert_eq!(request.params["count"], "6");
    assert_eq!(request.params["adlt"], "off");
    assert_eq!(request.params["mkt"], "zh-CN");
    assert_eq!(
        serde_json::from_slice::<Value>(&tokio::fs::read(&path).await.unwrap()).unwrap(),
        original
    );
    tokio::fs::write(
        &path,
        br#"{"webSearch":{"language":"ko-KR","maxResults":11}}"#,
    )
    .await
    .unwrap();
    service
        .search(&json!({"query":"config"}), None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(fixture.last().params["count"], "11");
    assert_eq!(fixture.last().params["mkt"], "ko-KR");
    tokio::fs::write(&path, b"broken").await.unwrap();
    assert!(service
        .get_config()
        .await
        .unwrap_err()
        .message
        .contains("配置无法解析"));
    let before = fixture.state.requests.lock().unwrap().len();
    assert!(service
        .search(
            &json!({"query":"must not connect"}),
            None,
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .message
        .contains("配置无法解析"));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), before);
    fixture.close().await;
}

#[tokio::test]
async fn real_http_redirect_errors_non_rss_and_empty_results() {
    let fixture = Fixture::new().await;
    let mut endpoint = fixture.endpoint.clone();
    endpoint.set_path("/redirect");
    let service = WebSearchService::fixture(
        fixture.directory.join("pisper.json"),
        endpoint,
        Duration::from_secs(15),
    );
    assert_eq!(
        service
            .search(&json!({"query":"redirect"}), None, CancellationToken::new())
            .await
            .unwrap()
            .results
            .len(),
        2
    );
    assert_eq!(fixture.last().params["q"], "redirected");
    let service = fixture.service();
    for (query, expected) in [
        ("error", "Bing 搜索失败（HTTP 503）：Maintenance & retry"),
        ("empty-error", "Bing 搜索失败（HTTP 403）：无响应内容"),
        (
            "non-rss",
            "Bing 没有返回可解析的 RSS 搜索结果，请稍后重试。",
        ),
    ] {
        assert_eq!(
            service
                .search(&json!({"query":query}), None, CancellationToken::new())
                .await
                .unwrap_err()
                .message,
            expected
        );
    }
    let empty = service
        .search(&json!({"query":"empty"}), None, CancellationToken::new())
        .await
        .unwrap();
    assert!(empty.results.is_empty());
    assert_eq!(empty.text, "Bing 没有找到“empty”的结果。");
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_and_shutdown_interrupt_live_requests_and_body_reads() {
    for (query, shutdown) in [("stall", false), ("body-stall", false), ("stall", true)] {
        let fixture = Fixture::new().await;
        let service = fixture.service();
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        let captured = service.clone();
        let task =
            tokio::spawn(
                async move { captured.search(&json!({"query":query}), None, signal).await },
            );
        tokio::time::timeout(Duration::from_secs(2), fixture.state.arrived.notified())
            .await
            .unwrap();
        if shutdown {
            service.dispose();
        } else {
            cancellation.cancel();
        }
        let error = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.cancelled);
        assert!(!error.message.contains("超时"));
        fixture.close().await;
    }
}

#[tokio::test]
async fn request_and_body_timeouts_are_distinct_from_cancellation() {
    for query in ["stall", "body-stall"] {
        let fixture = Fixture::new().await;
        let service = WebSearchService::fixture(
            fixture.directory.join("pisper.json"),
            fixture.endpoint.clone(),
            Duration::from_millis(200),
        );
        let error = service
            .search(&json!({"query":query}), None, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!error.cancelled);
        assert_eq!(
            error.message,
            if query == "body-stall" {
                "The operation was aborted due to timeout"
            } else {
                "Bing 搜索请求超时，请检查网络后重试。"
            }
        );
        fixture.close().await;
    }
}

#[tokio::test]
async fn pre_cancelled_request_and_empty_query_do_not_connect() {
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(
        service
            .search(&json!({"query":"hello"}), None, cancellation)
            .await
            .unwrap_err()
            .cancelled
    );
    for query in [json!(" "), Value::Null, json!(false), json!([])] {
        assert_eq!(
            service
                .search(&json!({"query":query}), None, CancellationToken::new())
                .await
                .unwrap_err()
                .message,
            "搜索关键词不能为空。"
        );
    }
    assert!(fixture.state.requests.lock().unwrap().is_empty());
    fixture.close().await;
}

#[tokio::test]
async fn network_errors_remove_query_and_endpoint_details() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/private-endpoint",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    // Keep ownership of the bound endpoint. A just-closed Windows port can
    // spend the entire request deadline refusing a connection or be reused by
    // a parallel fixture; neither deterministically exercises transport error.
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "search connection closed before HTTP headers");
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        // The HTTP parser rejects this immediately, before response headers.
        socket
            .write_all(b"not-an-http-response\r\n\r\n")
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let service = WebSearchService::fixture(
        PathBuf::from("missing-test-config.json"),
        endpoint,
        Duration::from_secs(1),
    );
    let error = service
        .search(
            &json!({"query":"private-query"}),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let request = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
    assert!(request.starts_with("GET /private-endpoint?"));
    assert!(request.contains("q=private-query"));
    assert_eq!(error.message, "无法连接 Bing：fetch failed");
    assert!(!error.cancelled);
    assert!(!error.message.contains("private-query"));
    assert!(!error.message.contains("private-endpoint"));
}

fn context() -> pi_rust::coding_agent::extensions::types::ExtensionContext {
    ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        ".",
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    )
    .create_context()
}

#[tokio::test]
async fn native_tool_performs_http_and_emits_release_update_and_structured_details() {
    let fixture = Fixture::new().await;
    let tool = create_tool(fixture.service());
    assert_eq!(tool.name, "web_search");
    assert_eq!(tool.prompt_guidelines.as_ref().unwrap().len(), 5);
    assert_eq!(manifest()["risk"], "medium");
    let updates = Arc::new(Mutex::new(Vec::new()));
    let observed = updates.clone();
    let update: AgentToolUpdateCallbackValue =
        Arc::new(move |value| observed.lock().unwrap().push(value.clone()));
    let result = tool.execute_async.as_ref().unwrap()(
        "search-1".into(),
        json!({"query":"Pisper agent","limit":2}),
        None,
        Some(update),
        context(),
    )
    .await
    .unwrap();
    assert_eq!(
        *updates.lock().unwrap(),
        vec![json!({"content":[{"type":"text","text":"Searching Bing for: Pisper agent"}]})]
    );
    assert_eq!(result["details"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(result["content"][0]["text"], result["details"]["text"]);
    assert_eq!(fixture.last().params["q"], "Pisper agent");
    fixture.close().await;
}

#[tokio::test]
async fn native_tool_abort_signal_interrupts_http_and_retains_update() {
    let fixture = Fixture::new().await;
    let tool = create_tool(fixture.service());
    let signal = Arc::new(AbortSignal::new());
    let cancellation = signal.clone();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let observed = updates.clone();
    let update: AgentToolUpdateCallbackValue =
        Arc::new(move |value| observed.lock().unwrap().push(value.clone()));
    let execution = tool.execute_async.as_ref().unwrap()(
        "search-cancel".into(),
        json!({"query":"stall"}),
        Some(signal),
        Some(update),
        context(),
    );
    let task = tokio::spawn(execution);
    tokio::time::timeout(Duration::from_secs(2), fixture.state.arrived.notified())
        .await
        .unwrap();
    cancellation.abort();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        "This operation was aborted"
    );
    assert_eq!(updates.lock().unwrap().len(), 1);
    fixture.close().await;
}

#[tokio::test]
async fn http_test_route_returns_only_count_provider_and_uses_submitted_config() {
    let fixture = Fixture::new().await;
    let router = crate::web_search_api::router::<()>(fixture.service());
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/plugins/web-search/test")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"language":"zh-TW","safeSearch":2,"maxResults":12}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"count":2,"provider":"bing"})
    );
    let request = fixture.last();
    assert_eq!(request.params["q"], "Pisper AI agent");
    assert_eq!(request.params["count"], "3");
    assert_eq!(request.params["mkt"], "zh-TW");
    assert_eq!(request.params["adlt"], "strict");
    fixture.close().await;
}
