//! Real loopback HTTP and production Pi factory proofs. No model provider,
//! Bing endpoint or profile outside these isolated fixtures is contacted.
use super::*;
use axum::Json;
use base64::{engine::general_purpose::STANDARD, Engine};
use pi_rust::{
    ai::auth::credential_store::InMemoryCredentialStore,
    coding_agent::{
        agent_session::{AgentSession, ExtensionBindings},
        cli::{args::Args, project_trust::AppMode},
        core::{
            agent_session_runtime::{
                create_agent_session_runtime, CreateAgentSessionRuntimeOptions,
            },
            model_runtime::{CreateModelRuntimeOptions, ModelRuntime},
            models_store::InMemoryCodingAgentModelsStore,
            settings_manager::{SettingsManager, SettingsValue},
        },
        main::runtime::{create_cli_runtime_factory, CliRuntimeFactoryOptions},
        session_manager::SessionManager,
    },
};
use std::sync::Weak;

fn oracle() -> Value {
    serde_json::from_str(include_str!("compression-oracle.json")).unwrap()
}

#[derive(Clone)]
struct PayloadState {
    bytes: Vec<u8>,
    encoding: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}
struct PayloadServer {
    endpoint: Url,
    requests: Arc<Mutex<Vec<Recorded>>>,
    shutdown: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl PayloadServer {
    async fn start(row: &Value) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = PayloadState {
            bytes: STANDARD.decode(row["base64"].as_str().unwrap()).unwrap(),
            encoding: row["encoding"].as_str().unwrap().into(),
            requests: requests.clone(),
        };
        async fn response(State(state): State<PayloadState>, request: Request) -> Response {
            let url = Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
            state.requests.lock().unwrap().push(Recorded {
                path: url.path().into(),
                params: url
                    .query_pairs()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect(),
                headers: request.headers().clone(),
            });
            let mut response = Body::from(state.bytes).into_response();
            response.headers_mut().insert(
                "Content-Type",
                "application/rss+xml; charset=iso-8859-1".parse().unwrap(),
            );
            if state.encoding != "identity" {
                response
                    .headers_mut()
                    .insert("Content-Encoding", state.encoding.parse().unwrap());
            }
            response
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{}/search", listener.local_addr().unwrap())).unwrap();
        let router = Router::new()
            .route("/search", get(response))
            .with_state(state);
        let shutdown = CancellationToken::new();
        let cancel = shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            endpoint,
            requests,
            shutdown,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for PayloadServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn actual_compressed_http_matches_node_fetch_utf8_and_nonempty_results() {
    let reference = oracle();
    assert_eq!(
        reference["sourceCommit"],
        "582160235671903d9f1c7034b457557b1df74b68"
    );
    for row in reference["rows"].as_array().unwrap() {
        let server = PayloadServer::start(row).await;
        let service = WebSearchService::fixture(
            PathBuf::from("missing-web-search-proof-config.json"),
            server.endpoint.clone(),
            Duration::from_secs(15),
        );
        let result = service
            .search(
                &reference["input"],
                Some(&json!({"safeSearch":2,"maxResults":11})),
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", row["format"]));
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            row["expected"],
            "{}",
            row["format"]
        );
        assert_eq!(result.results.len(), 2);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].params,
            HashMap::from([
                ("q".into(), "release public evidence".into()),
                ("format".into(), "rss".into()),
                ("count".into(), "2".into()),
                ("adlt".into(), "strict".into()),
                ("first".into(), "3".into()),
                ("mkt".into(), "ko-KR".into()),
                ("setlang".into(), "ko".into()),
            ])
        );
        for (name, field) in [
            ("accept", "accept"),
            ("user-agent", "userAgent"),
            ("accept-encoding", "acceptEncoding"),
            ("accept-language", "acceptLanguage"),
            ("sec-fetch-mode", "fetchMode"),
        ] {
            assert_eq!(
                requests[0].headers[name],
                reference["request"]["headers"][field].as_str().unwrap()
            );
        }
        drop(requests);
        server.close().await;
    }
}

#[test]
fn rss_utf16_entities_replacement_order_and_wire_cleaning_follow_release() {
    let reference = oracle();
    assert_eq!(
        serde_json::to_value(
            parse_bing_rss_results(reference["xml"].as_str().unwrap(), 2).unwrap()
        )
        .unwrap(),
        reference["rows"][0]["expected"]["results"],
    );
    for (input, expected) in [
        ("&#xD83D;&#xDE00;", "😀"),
        ("&#55357;&#56832;", "😀"),
        ("&#xD83D;&#56832;", "😀"),
        ("&#55357;&#xDE00;", "😀"),
        ("&#xD83D;x&#xDE00;", "�x�"),
        ("&#xD83D;<b></b>&#xDE00;", "� �"),
        ("&amp;#39;&AMP;apos;", "''"),
        ("&amp;nbsp;", "&nbsp;"),
        ("&#x26;#65;", "A"),
        ("a\r\n\tb\u{feff}c", "a b c"),
    ] {
        assert_eq!(
            super::super::rss::plain_text(input, 1200).unwrap(),
            expected,
            "{input}"
        );
    }
    assert_eq!(
        super::super::rss::plain_text("&#xD83D;&#xDE00;", 1).unwrap(),
        "�"
    );
}

#[tokio::test]
async fn actual_http_test_route_accepts_empty_and_untyped_json_and_release_headers() {
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web_search_api::router::<()>(fixture.service());
    let shutdown = CancellationToken::new();
    let cancel = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for body in ["", r#"{"language":"zh-TW","safeSearch":2,"maxResults":12}"#] {
        let response = client
            .post(format!("{base}/api/plugins/web-search/test"))
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.headers()["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"count":2,"provider":"bing"})
        );
    }
    let request = fixture.last();
    assert_eq!(request.params["q"], "Pisper AI agent");
    assert_eq!(request.params["count"], "3");
    assert_eq!(request.params["mkt"], "zh-TW");
    assert_eq!(request.params["adlt"], "strict");
    let before = fixture.state.requests.lock().unwrap().len();
    let invalid = client
        .post(format!("{base}/api/plugins/web-search/test"))
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert!(invalid.json::<Value>().await.unwrap()["error"]
        .as_str()
        .unwrap()
        .contains("JSON"));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), before);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn absent_config_defaults_but_corrupt_or_directory_config_prevents_http() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.service().get_config().await.unwrap(),
        WebSearchConfig::normalize(&Value::Null)
    );
    let service = WebSearchService::fixture(
        fixture.directory.clone(),
        fixture.endpoint.clone(),
        Duration::from_secs(15),
    );
    assert!(service
        .get_config()
        .await
        .unwrap_err()
        .message
        .contains("配置无法读取"));
    assert!(fixture.state.requests.lock().unwrap().is_empty());
    let path = fixture.directory.join("pisper.json");
    tokio::fs::write(&path, b"broken").await.unwrap();
    let service = fixture.service();
    assert!(service
        .search(
            &json!({"query":"no configuration fallback"}),
            None,
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .message
        .contains("配置无法解析"));
    // An explicit truthy test config bypasses reading the canonical file,
    // exactly as release search({config}) does.
    assert_eq!(
        service
            .test(&json!({}), CancellationToken::new())
            .await
            .unwrap()
            .results
            .len(),
        2
    );
    fixture.close().await;
}

#[derive(Clone)]
struct ModelState {
    requests: Arc<Mutex<Vec<Value>>>,
}

#[tokio::test]
async fn json_input_overflow_and_surrogates_use_node_coercions_without_weakening_grammar() {
    let reference: Value = serde_json::from_str(include_str!("json-input-oracle.json")).unwrap();
    let fixture = Fixture::new().await;
    let service = fixture.service();
    let config_path = fixture.directory.join("pisper.json");
    for row in reference["rows"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        let parsed = super::super::parse_config_json(text).unwrap();
        assert_eq!(
            normalize_config_checked(&parsed).unwrap(),
            row["expected"],
            "{text}"
        );
        let canonical = format!(r#"{{"unrelated":true,"webSearch":{text}}}"#);
        tokio::fs::write(&config_path, canonical.as_bytes())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(service.get_config().await.unwrap()).unwrap(),
            row["expected"]
        );
        let router = crate::web_search_api::router::<()>(service.clone());
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/plugins/web-search/test")
                    .body(Body::from(text.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{text}");
        let result: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result, json!({"count":2,"provider":"bing"}));
        assert_eq!(
            tokio::fs::read(&config_path).await.unwrap(),
            canonical.as_bytes()
        );
    }
    for invalid in [
        "{1e400:1}",
        r#"{"field":01e400}"#,
        "1e400 1",
        "[1e400,,2]",
        r#"{"x":"\uZZZZ"}"#,
        r#"{"x":"\UFFFF"}"#,
        "+1e400",
        "1e400false",
    ] {
        assert!(
            super::super::parse_config_json(invalid).is_err(),
            "{invalid}"
        );
    }
    fixture.close().await;
}

#[tokio::test]
async fn checked_js_conversion_and_null_canonical_config_match_node_errors() {
    for input in [
        json!({"language":{"toString":null}}),
        json!({"maxResults":{"toString":4}}),
        json!({"safeSearch":[{"toString":false}]}),
    ] {
        assert_eq!(
            normalize_config_checked(&input).unwrap_err().message,
            "Cannot convert object to primitive value",
            "{input}",
        );
    }
    assert_eq!(
        normalize_config_checked(&Value::Null).unwrap_err().message,
        "Cannot read properties of null (reading 'language')",
    );
    let fixture = Fixture::new().await;
    let service = fixture.service();
    for input in [
        json!({"query":{"toString":false}}),
        json!({"query":"valid","limit":{"toString":null}}),
        json!({"query":"valid","page":[{"toString":1}]}),
        json!({"query":"valid","language":{"toString":true}}),
    ] {
        assert_eq!(
            service
                .search(&input, Some(&json!({})), CancellationToken::new())
                .await
                .unwrap_err()
                .message,
            "Cannot convert object to primitive value",
        );
    }
    let router = crate::web_search_api::router::<()>(service.clone());
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/plugins/web-search/test")
                .body(Body::from(r#"{"language":{"toString":null}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(error["error"], "Cannot convert object to primitive value");
    tokio::fs::write(fixture.directory.join("pisper.json"), b"null")
        .await
        .unwrap();
    assert_eq!(
        service.get_config().await.unwrap_err().message,
        "Cannot read properties of null (reading 'webSearch')",
    );
    assert!(fixture.state.requests.lock().unwrap().is_empty());
    fixture.close().await;
}

async fn model_completion(State(state): State<ModelState>, Json(body): Json<Value>) -> Response {
    state.requests.lock().unwrap().push(body.clone());
    let tools = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    let delta = match tools.len() {
        0 => {
            json!({"role":"assistant","tool_calls":[{"index":0,"id":"proof-discovery-1","type":"function","function":{"name":"discover_tools","arguments":json!({"query":"网页搜索","limit":1}).to_string()}}]})
        }
        1 => {
            json!({"role":"assistant","tool_calls":[{"index":0,"id":"proof-search-1","type":"function","function":{"name":"call_tool","arguments":json!({"name":"web_search","arguments":{"query":"  release public evidence  ","page":2,"limit":2}}).to_string()}}]})
        }
        _ => {
            json!({"role":"assistant","content":format!("Verified actual RSS sources:\n{}",tools.last().unwrap()["content"].as_str().unwrap())})
        }
    };
    let finish = if tools.len() < 2 {
        "tool_calls"
    } else {
        "stop"
    };
    let chunks = [
        json!({"id":"web-search-proof","object":"chat.completion.chunk","model":"web-search-proof-model","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
        json!({"id":"web-search-proof","object":"chat.completion.chunk","model":"web-search-proof-model","choices":[{"index":0,"delta":{},"finish_reason":finish}],"usage":{"prompt_tokens":12,"completion_tokens":8,"total_tokens":20}}),
    ];
    let text = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        text,
    )
        .into_response()
}

#[tokio::test]
async fn actual_pi_factory_discovery_gateway_http_search_returns_results_to_model() {
    use crate::native_tool_gateway::{GatewayPort, GatewaySession};
    use pi_rust::coding_agent::modes::json_event::to_json_event_string;
    let fixture = Fixture::new().await;
    let reference = oracle();
    let row = reference["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["format"] == "gzip")
        .unwrap();
    let rss = PayloadServer::start(row).await;
    let cwd = fixture.directory.join("workspace");
    let agent = fixture.directory.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent).unwrap();
    let app_path = agent.join("pisper.json");
    std::fs::write(&app_path,serde_json::to_vec(&json!({"enabledTools":["web_search"],"webSearch":{"language":"ko-KR","safeSearch":2,"maxResults":11},"unrelated":{"keep":true}})).unwrap()).unwrap();
    let app_before = std::fs::read(&app_path).unwrap();
    let web = WebSearchService::fixture(
        app_path.clone(),
        rss.endpoint.clone(),
        Duration::from_secs(15),
    );
    let model_requests = Arc::new(Mutex::new(Vec::new()));
    let model_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model_base = format!("http://{}/v1", model_listener.local_addr().unwrap());
    let models = json!({"providers":{"web-search-proof":{"baseUrl":model_base,"api":"openai-completions","apiKey":"synthetic-web-search-proof-key","models":[{"id":"web-search-proof-model","name":"Local web search proof","reasoning":false,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":8192,"maxTokens":2048}]}}});
    std::fs::write(
        agent.join("models.json"),
        serde_json::to_vec(&models).unwrap(),
    )
    .unwrap();
    let model_router = Router::new()
        .route(
            "/v1/chat/completions",
            axum::routing::post(model_completion),
        )
        .with_state(ModelState {
            requests: model_requests.clone(),
        });
    let model_shutdown = CancellationToken::new();
    let cancel = model_shutdown.clone();
    let model_server = tokio::spawn(async move {
        axum::serve(model_listener, model_router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .unwrap();
    });
    let session_holder = Arc::new(Mutex::new(None::<Weak<AgentSession>>));
    let holder = session_holder.clone();
    let authorized = Arc::new(Mutex::new(Vec::<Value>::new()));
    let authorization = authorized.clone();
    let port: GatewayPort = Arc::new(move |native_id| {
        let (holder, authorization) = (holder.clone(), authorization.clone());
        Box::pin(async move {
            let session = holder.lock().unwrap().as_ref().unwrap().upgrade().unwrap();
            assert_eq!(native_id, session.session_id());
            let target = session.get_tool_definition("web_search").unwrap();
            Ok(GatewaySession {
                callable: vec![target],
                active_names: session.get_active_tool_names().into_iter().collect(),
                authorize: Arc::new(move |name, call, args, _| {
                    authorization.lock().unwrap().push(
                        json!({"name":name,"call":call,"args":args,"nativeSessionId":native_id}),
                    );
                    Box::pin(async { Ok(None) })
                }),
            })
        })
    });
    let search = web.clone();
    let factory = create_cli_runtime_factory(CliRuntimeFactoryOptions {
        parsed: Args {
            provider: Some("web-search-proof".into()),
            model: Some("web-search-proof-model".into()),
            no_skills: Some(true),
            no_prompt_templates: Some(true),
            no_themes: Some(true),
            no_context_files: Some(true),
            project_trust_override: Some(true),
            ..Default::default()
        },
        startup_cwd: cwd.to_string_lossy().into_owned(),
        initial_session_cwd: cwd.to_string_lossy().into_owned(),
        agent_dir: agent.to_string_lossy().into_owned(),
        startup_settings_manager: SettingsManager::in_memory(SettingsValue::obj(vec![])),
        app_mode: AppMode::Print,
        extension_factories: vec![crate::native_tool_gateway::create_extension(port)],
        extension_module_loader: None,
        model_runtime_factory: Some(Arc::new(|_, agent, signal| {
            Box::pin(async move {
                ModelRuntime::create(CreateModelRuntimeOptions {
                    credentials: Some(Arc::new(InMemoryCredentialStore::default())),
                    models_path: Some(Some(
                        PathBuf::from(agent)
                            .join("models.json")
                            .to_string_lossy()
                            .into_owned(),
                    )),
                    models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
                    allow_model_network: false,
                    signal: Some(signal),
                    ..Default::default()
                })
                .await
                .map_err(anyhow::Error::msg)
            })
        })),
        custom_tool_factory: Some(Arc::new(move |_, _| {
            let search = search.clone();
            Box::pin(async move { Ok(vec![create_tool(search)]) })
        })),
        model_scope_warning: None,
    })
    .unwrap();
    let manager = SessionManager::create(
        &cwd.to_string_lossy(),
        Some(&agent.join("sessions").to_string_lossy()),
        None,
    )
    .unwrap();
    let runtime = create_agent_session_runtime(
        factory.create_runtime,
        CreateAgentSessionRuntimeOptions {
            cwd: cwd.to_string_lossy().into_owned(),
            agent_dir: agent.to_string_lossy().into_owned(),
            session_manager: Arc::new(Mutex::new(manager)),
            session_start_event: None,
            project_trust_context: None,
        },
    )
    .await
    .unwrap();
    let session = runtime.session();
    *session_holder.lock().unwrap() = Some(Arc::downgrade(&session));
    session
        .bind_extensions(ExtensionBindings::default())
        .await
        .unwrap();
    session.set_active_tools_by_name(vec!["discover_tools".into(), "call_tool".into()]);
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = events.clone();
    let subscription = session.subscribe(Arc::new(move |event| {
        observed
            .lock()
            .unwrap()
            .push(serde_json::from_str(&to_json_event_string(event).unwrap()).unwrap());
    }));
    tokio::time::timeout(
        Duration::from_secs(30),
        session.prompt(
            "Discover web search and verify public sources through its gateway.",
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    subscription.unsubscribe();
    let requests = model_requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "actual model HTTP roundtrips");
    for request in requests.iter() {
        let tools = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(tools.contains(&"discover_tools") && tools.contains(&"call_tool"));
        assert!(
            !tools.contains(&"web_search"),
            "optional tool must remain inactive"
        );
    }
    let discovery = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(discovery.contains("Web Search [web_search]"));
    assert!(discovery.contains("query: string"));
    assert!(discovery.contains("inactive: call through call_tool"));
    let received = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .last()
        .unwrap()["content"]
        .as_str()
        .unwrap();
    assert_eq!(
        received,
        reference["rows"][0]["expected"]["text"].as_str().unwrap()
    );
    assert!(received.contains("https://example.com/evidence?q=a&b=2"));
    assert!(received.contains("https://example.com/docs"));
    drop(requests);
    let events = events.lock().unwrap();
    let finished = events
        .iter()
        .find(|event| event["type"] == "tool_execution_end" && event["toolName"] == "call_tool")
        .unwrap();
    assert_eq!(finished["isError"], false);
    let details = &finished["result"]["details"];
    assert_eq!(details["gatewayToolName"], "web_search");
    for field in ["query", "provider", "results", "text"] {
        assert_eq!(details[field], reference["rows"][0]["expected"][field]);
    }
    assert_eq!(details["results"].as_array().unwrap().len(), 2);
    assert!(events
        .iter()
        .any(|event| event["type"] == "tool_execution_update"
            && event["toolName"] == "call_tool"
            && event.to_string().contains("Searching Bing for:")));
    assert!(events.iter().any(|event| event["type"] == "message_end"
        && event.to_string().contains("Verified actual RSS sources:")));
    drop(events);
    assert_eq!(
        authorized.lock().unwrap().as_slice(),
        &[
            json!({"name":"web_search","call":"proof-search-1:web_search","args":{"query":"  release public evidence  ","page":2,"limit":2},"nativeSessionId":session.session_id()})
        ]
    );
    assert!(!session
        .get_active_tool_names()
        .contains(&"web_search".to_owned()));
    let requests = rss.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].params["mkt"], "ko-KR");
    assert_eq!(requests[0].params["first"], "3");
    assert_eq!(requests[0].params["adlt"], "strict");
    assert_eq!(requests[0].headers["user-agent"], "Pisper Web Search/1.0");
    drop(requests);
    assert_eq!(std::fs::read(&app_path).unwrap(), app_before);
    let journal = session
        .session_manager
        .lock()
        .unwrap()
        .get_session_file()
        .unwrap()
        .to_owned();
    let persisted = std::fs::read_to_string(journal).unwrap();
    assert!(persisted.contains("gatewayToolName"));
    assert!(persisted.contains("https://example.com/docs"));
    runtime.dispose().await.unwrap();
    web.dispose();
    model_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), model_server)
        .await
        .unwrap()
        .unwrap();
    rss.close().await;
    fixture.close().await;
}
