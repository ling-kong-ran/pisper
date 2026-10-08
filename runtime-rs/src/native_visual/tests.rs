use super::*;
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgAAAAAgABVKJPXQAAAABJRU5ErkJggg==";
const MP4: &[u8] = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom";
#[derive(Clone)]
struct Recorded {
    method: Method,
    path: String,
    query: String,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}
#[derive(Clone)]
struct ServerState {
    requests: Arc<Mutex<Vec<Recorded>>>,
    mode: Arc<Mutex<String>>,
    origin: String,
    arrived: Arc<tokio::sync::Notify>,
    cancelled: CancellationToken,
}
struct Fixture {
    root: PathBuf,
    state: ServerState,
    task: Option<tokio::task::JoinHandle<()>>,
    config: Arc<Mutex<VisualConfigSnapshot>>,
    service: Arc<VisualGenerationService>,
}
impl Fixture {
    async fn new(driver: &str, kind: VisualKind) -> Self {
        let root =
            std::env::temp_dir().join(format!("pisper-native-visual-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let cancelled = CancellationToken::new();
        let state = ServerState {
            requests: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(Mutex::new("normal".into())),
            origin: origin.clone(),
            arrived: Arc::new(tokio::sync::Notify::new()),
            cancelled: cancelled.clone(),
        };
        let router = Router::new()
            .route("/", any(response))
            .route("/{*path}", any(response))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancelled.cancelled_owned())
                .await
                .unwrap();
        });
        let model = json!({"id":"gpt-image-fixture","name":"Fixture model","kind":kind.as_str(),"visualApi":driver});
        let config = Arc::new(Mutex::new(VisualConfigSnapshot {
            models_json: json!({"providers":{"fixture":{"name":"Fixture Provider","api":"openai-responses","baseUrl":format!("{origin}/v1"),"headers":{"X-Test-Header":"exact-header","X-Stainless-Test":"must-be-filtered-for-sdk"},"models":[model]}}}),
            auth_json: json!({"fixture":{"type":"api_key","key":"synthetic-visual-key"}}),
            app_json: json!({"providerTypes":{"fixture":"visual"},"unrelated":{"keep":true}}),
            ..Default::default()
        }));
        let read = config.clone();
        let write = config.clone();
        let port = VisualConfigPort {
            read: Arc::new(move || {
                let value = read.lock().unwrap().clone();
                Box::pin(async move { Ok(value) })
            }),
            write_preference: Arc::new(move |kind, reference| {
                let config = write.clone();
                Box::pin(async move {
                    let mut config = config.lock().unwrap();
                    if !config.app_json["visualDefaultModels"].is_object() {
                        config.app_json["visualDefaultModels"] = json!({});
                    }
                    match reference {
                        Some(value) => {
                            config.app_json["visualDefaultModels"][kind.as_str()] = json!(value)
                        }
                        None => {
                            config.app_json["visualDefaultModels"]
                                .as_object_mut()
                                .unwrap()
                                .remove(kind.as_str());
                        }
                    }
                    Ok(())
                })
            }),
        };
        let service = VisualGenerationService::new(port);
        Self {
            root,
            state,
            task: Some(task),
            config,
            service,
        }
    }
    fn input(&self, kind: VisualKind) -> VisualRequest {
        VisualRequest {
            cwd: self.root.join("workspace"),
            input: json!({"kind":kind.as_str(),"prompt":"controlled fixture result","outputName":"fixture","aspectRatio":"16:9"}),
        }
    }
    fn mode(&self, value: &str) {
        *self.state.mode.lock().unwrap() = value.into();
    }
    fn records(&self) -> Vec<Recorded> {
        self.state.requests.lock().unwrap().clone()
    }
    async fn close(mut self) {
        self.service.dispose().await;
        self.state.cancelled.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.cancelled.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if self.root.parent() == Some(std::env::temp_dir().as_path())
            && self
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-native-visual-")
        {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
async fn response(State(state): State<ServerState>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    state.requests.lock().unwrap().push(Recorded {
        method: method.clone(),
        path: path.clone(),
        query,
        headers: headers.clone(),
        body: body.clone(),
    });
    state.arrived.notify_one();
    let mode = state.mode.lock().unwrap().clone();
    if path == "/" {
        return if mode == "new-api" || mode == "legacy-video" || mode == "duplicate-duration" {
            "<title>New API</title>".into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        };
    }
    if mode == "stall" && method == Method::POST {
        state.cancelled.cancelled().await;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if path.ends_with("/images/generations") {
        if mode == "secret-error" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":headers["x-secret"].to_str().unwrap()}})),
            )
                .into_response();
        }
        let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let first = input["model"] == "gpt-image-fixture";
        if first && matches!(mode.as_str(), "model-fallback" | "auth-fallback" | "safety") {
            let (status, message) = match mode.as_str() {
                "model-fallback" => (StatusCode::SERVICE_UNAVAILABLE, "no available channel"),
                "auth-fallback" => (StatusCode::UNAUTHORIZED, "invalid API key"),
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Content policy safety rejection",
                ),
            };
            return (
                status,
                [("x-should-retry", "false")],
                Json(json!({"error":{"message":message}})),
            )
                .into_response();
        }
        if mode == "responses" {
            return (StatusCode::NOT_FOUND, "Images absent").into_response();
        }
        if mode == "safety" {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":{"message":"Content policy safety rejection"}})),
            )
                .into_response();
        }
        if mode == "unavailable" {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [("x-should-retry", "false")],
                Json(json!({"error":{"message":"no available channel"}})),
            )
                .into_response();
        }
        if mode == "sdk-retry" {
            let count = state
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|value| value.path.ends_with("/images/generations"))
                .count();
            if count == 1 {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after-ms", "1")],
                    Json(json!({"error":{"message":"try again"}})),
                )
                    .into_response();
            }
        }
        return Json(json!({"data":[{"b64_json":PNG,"mime_type":"image/png"}]})).into_response();
    }
    if path.ends_with("/images/edits") {
        return Json(json!({"data":[{"b64_json":PNG}]})).into_response();
    }
    if path.ends_with("/responses") {
        return Json(json!({"id":"response-image-1","output":[{"type":"image_generation_call","result":PNG}]})).into_response();
    }
    if path.ends_with("/chat/completions") {
        return Json(json!({"choices":[{"message":{"images":[{"image_url":{"url":format!("data:image/png;base64,{PNG}")}}]}}]})).into_response();
    }
    if path.ends_with(":generateContent") {
        return Json(json!({"candidates":[{"content":{"parts":[{"inline_data":{"mime_type":"image/png","data":PNG}}]}}]})).into_response();
    }
    if path.ends_with(":predictLongRunning") {
        if mode == "polling" {
            return Json(json!({"name":"operations/google-video","done":false})).into_response();
        }
        return Json(json!({"name":"operations/google-video","done":true,"response":{"generatedVideos":[{"video":{"uri":format!("{}/video.mp4",state.origin)}}]}})).into_response();
    }
    if path == "/v1/operations/google-video" && method == Method::GET {
        return Json(json!({"name":"operations/google-video","done":true,"response":{"generatedVideos":[{"video":{"uri":format!("{}/video.mp4",state.origin)}}]}})).into_response();
    }
    if path.ends_with("/videos") && method == Method::POST {
        if mode == "legacy-video" {
            return StatusCode::NOT_FOUND.into_response();
        }
        if mode == "duplicate-duration" {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error":{"message":"duplicate field duration"}})),
            )
                .into_response();
        }
        return Json(json!({"id":"video-1","status":if mode == "polling" {"queued"} else {"completed"},"progress":if mode == "polling" {7} else {100}})).into_response();
    }
    if path == "/v1/videos/generations" || path == "/v1/video/generations" {
        if mode == "polling" {
            return Json(json!({"task_id":"video-relay","status":"pending"})).into_response();
        }
        return Json(json!({"task_id":"video-relay","status":"succeeded","url":format!("{}/video.mp4",state.origin)})).into_response();
    }
    if method == Method::GET
        && (path.ends_with("/videos/video-1") || path.ends_with("/videos/generations/video-relay"))
    {
        return Json(json!({"id":"video-1","status":"completed","progress":100,"url":format!("{}/video.mp4",state.origin)})).into_response();
    }
    if path == "/video.mp4" || path.ends_with("/content") {
        if mode == "content-fallback" && path.starts_with("/v1/") {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("x-should-retry", "false")],
                Json(json!({"error":{"message":"download rate limited"}})),
            )
                .into_response();
        }
        return ([(axum::http::header::CONTENT_TYPE, "video/mp4")], MP4).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

#[tokio::test]
async fn all_nine_driver_registrations_make_real_http_and_save_actual_bytes() {
    for (driver, kind) in [
        ("openai-image", VisualKind::Image),
        ("openrouter-image", VisualKind::Image),
        ("google-image", VisualKind::Image),
        ("xai-image", VisualKind::Image),
        ("new-api-image", VisualKind::Image),
        ("openai-video", VisualKind::Video),
        ("google-video", VisualKind::Video),
        ("xai-video", VisualKind::Video),
        ("new-api-video", VisualKind::Video),
    ] {
        let fixture = Fixture::new(driver, kind).await;
        let updates = Arc::new(Mutex::new(Vec::new()));
        let observed = updates.clone();
        let result = fixture
            .service
            .generate(
                fixture.input(kind),
                VisualOptions {
                    on_progress: Some(Arc::new(move |value| observed.lock().unwrap().push(value))),
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{driver}: {error}"));
        let expected = if kind == VisualKind::Image {
            STANDARD.decode(PNG).unwrap()
        } else {
            MP4.to_vec()
        };
        assert_eq!(
            tokio::fs::read(&result.path).await.unwrap(),
            expected,
            "{driver}"
        );
        assert_eq!(result.size, expected.len() as u64);
        assert_eq!(result.kind, kind);
        assert_eq!(result.operation, VisualOperation::Generate);
        assert_eq!(result.attempted_models, ["fixture/gpt-image-fixture"]);
        assert!(!result.fallback_used);
        assert!(result
            .path
            .starts_with(fixture.root.join("workspace/generated/visuals")));
        assert!(updates.lock().unwrap()[0].contains("使用 Fixture Provider / Fixture model"));
        let records = fixture.records();
        assert!(
            records.iter().any(|value| value.method == Method::POST),
            "{driver}"
        );
        if driver.starts_with("google-") {
            let post = records
                .iter()
                .find(|value| value.method == Method::POST)
                .unwrap();
            assert_eq!(post.headers["x-goog-api-key"], "synthetic-visual-key");
        } else {
            let post = records
                .iter()
                .find(|value| value.method == Method::POST)
                .unwrap();
            assert_eq!(post.headers["authorization"], "Bearer synthetic-visual-key");
        }
        if driver == "openai-video" {
            let post = records
                .iter()
                .find(|value| value.method == Method::POST)
                .unwrap();
            assert!(post.headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data;"));
            assert!(String::from_utf8_lossy(&post.body).contains("name=\"model\""));
            assert!(records
                .iter()
                .any(|value| value.path.ends_with("/content") && value.query == "variant=video"));
        }
        if matches!(
            driver,
            "openai-image" | "new-api-image" | "openrouter-image" | "openai-video"
        ) {
            assert!(!records.iter().any(|record| record
                .headers
                .keys()
                .any(|name| name.as_str().starts_with("x-stainless-"))));
        }
        assert_eq!(fixture.service.live_count(), 0);
        fixture.close().await;
    }
}

#[tokio::test]
async fn image_edit_sources_mask_quality_and_format_reach_actual_multipart() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    let source = fixture.root.join("outside-source.png");
    let mask = fixture.root.join("mask.png");
    tokio::fs::write(&source, STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    tokio::fs::write(&mask, STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    let mut request = fixture.input(VisualKind::Image);
    request.input["sourceImages"] = json!([source, source]);
    request.input["maskPath"] = json!(mask);
    request.input["quality"] = json!("hd");
    request.input["outputFormat"] = json!("jpeg");
    let result = fixture
        .service
        .generate(request, Default::default())
        .await
        .unwrap();
    assert_eq!(result.operation, VisualOperation::Edit);
    assert_eq!(result.mime_type, "image/jpeg");
    assert_eq!(result.path.extension().unwrap(), "jpg");
    let record = fixture
        .records()
        .into_iter()
        .find(|value| value.path.ends_with("/images/edits"))
        .unwrap();
    let body = String::from_utf8_lossy(&record.body);
    assert_eq!(body.matches("name=\"image[]\"").count(), 2);
    assert!(body.contains("filename=\"outside-source.png\""));
    assert!(body.contains("name=\"mask\""));
    assert!(body.contains("name=\"quality\"") && body.contains("hd"));
    assert!(body.contains("name=\"output_format\"") && body.contains("jpeg"));
    fixture.close().await;
}

#[tokio::test]
async fn responses_fallback_and_paid_no_fallback_have_exact_post_boundaries() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    fixture.mode("responses");
    let result = fixture
        .service
        .generate(fixture.input(VisualKind::Image), Default::default())
        .await
        .unwrap();
    assert_eq!(result.remote_id.as_deref(), Some("response-image-1"));
    assert!(!result.fallback_used);
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|value| value.method == Method::POST)
            .map(|value| value.path.as_str())
            .collect::<Vec<_>>(),
        ["/v1/images/generations", "/v1/responses"]
    );
    fixture.state.requests.lock().unwrap().clear();
    assert!(fixture
        .service
        .generate(
            fixture.input(VisualKind::Image),
            VisualOptions {
                allow_fallback: false,
                ..Default::default()
            }
        )
        .await
        .is_err());
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|value| value.method == Method::POST)
            .count(),
        1
    );
    fixture.close().await;
}

#[tokio::test]
async fn sdk_retries_are_real_but_disabled_for_paid_work() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    fixture.mode("sdk-retry");
    fixture
        .service
        .generate(fixture.input(VisualKind::Image), Default::default())
        .await
        .unwrap();
    assert_eq!(fixture.records().len(), 2);
    fixture.state.requests.lock().unwrap().clear();
    let error = fixture
        .service
        .generate(
            fixture.input(VisualKind::Image),
            VisualOptions {
                allow_fallback: false,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(503));
    assert_eq!(fixture.records().len(), 1);
    fixture.close().await;
}

#[tokio::test]
async fn new_api_detection_and_legacy_video_json_do_not_use_xai_route() {
    let fixture = Fixture::new("xai-video", VisualKind::Video).await;
    fixture.mode("legacy-video");
    let mut request = fixture.input(VisualKind::Video);
    request.input["durationSeconds"] = json!(4);
    let result = fixture
        .service
        .generate(request, Default::default())
        .await
        .unwrap();
    assert_eq!(result.remote_id.as_deref(), Some("video-relay"));
    let records = fixture.records();
    assert_eq!(records[0].path, "/");
    assert!(!records[0].headers.contains_key("authorization"));
    assert!(!records
        .iter()
        .any(|value| value.path == "/v1/videos/generations"));
    let legacy = records
        .iter()
        .find(|value| value.path == "/v1/video/generations")
        .unwrap();
    let body: Value = serde_json::from_slice(&legacy.body).unwrap();
    assert_eq!(body["duration"], 4);
    assert_eq!(body["width"], 1280);
    assert_eq!(body["height"], 720);
    assert!(body.get("seconds").is_none());
    fixture.close().await;
}

#[tokio::test]
async fn new_api_duplicate_duration_error_never_tries_another_route() {
    let fixture = Fixture::new("xai-video", VisualKind::Video).await;
    fixture.mode("duplicate-duration");
    let error = fixture
        .service
        .generate(fixture.input(VisualKind::Video), Default::default())
        .await
        .unwrap_err();
    assert!(error.message.contains("New API 视频渠道转发失败"));
    assert_eq!(error.status, Some(422));
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|value| value.method == Method::POST)
            .map(|value| value.path.as_str())
            .collect::<Vec<_>>(),
        ["/v1/videos"]
    );
    fixture.close().await;
}

#[tokio::test]
async fn preferences_and_test_preview_match_six_field_api_and_fixed_prompt() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    let initial = fixture.service.get_all_status().await.unwrap();
    assert_eq!(initial.as_object().unwrap().len(), 6);
    assert_eq!(initial["video"], Value::Null);
    let selected = fixture
        .service
        .set_preferred_model(VisualKind::Image, &json!("fixture/gpt-image-fixture"))
        .await
        .unwrap();
    assert_eq!(selected["imageSelection"], "fixture/gpt-image-fixture");
    let before = fixture.config.lock().unwrap().app_json.clone();
    assert!(fixture
        .service
        .set_preferred_model(VisualKind::Image, &json!("missing"))
        .await
        .is_err());
    assert_eq!(fixture.config.lock().unwrap().app_json, before);
    fixture
        .service
        .set_preferred_model(VisualKind::Image, &json!(""))
        .await
        .unwrap();
    assert_eq!(
        fixture.config.lock().unwrap().app_json["unrelated"]["keep"],
        true
    );
    let result = fixture
        .service
        .test_visual(&fixture.root, Default::default())
        .await
        .unwrap();
    assert!(result["path"].as_str().unwrap().contains("visual-test"));
    assert!(result["previewDataUrl"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    let request: Value = serde_json::from_slice(&fixture.records().last().unwrap().body).unwrap();
    assert_eq!(request["prompt"],"a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background");
    fixture.close().await;
}

#[tokio::test]
async fn cancellation_and_dispose_join_live_http_without_reexecuting() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    fixture.mode("stall");
    let token = CancellationToken::new();
    let service = fixture.service.clone();
    let request = fixture.input(VisualKind::Image);
    let cancel = token.clone();
    let task = tokio::spawn(async move {
        service
            .generate(
                request,
                VisualOptions {
                    cancellation: cancel,
                    ..Default::default()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), fixture.state.arrived.notified())
        .await
        .unwrap();
    token.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .cancelled
    );
    tokio::time::timeout(Duration::from_secs(2), fixture.service.dispose())
        .await
        .unwrap();
    assert_eq!(fixture.service.live_count(), 0);
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|value| value.method == Method::POST)
            .count(),
        1
    );
    fixture.close().await;
}

#[test]
fn source_pure_oracle_request_normalization_and_fallback_matrix_match() {
    let oracle: Value = serde_json::from_str(include_str!("oracles/contract.json")).unwrap();
    let cases = &oracle["executableVerificationPlan"]["actualPureOracle"];
    for row in cases["normalize"].as_array().unwrap() {
        let model = types::VisualModel {
            public: json!({"driver":row["driver"]}),
            key: Value::Null,
            headers: Default::default(),
            visual: false,
            configured: false,
            score: 0,
        };
        let kind = if row["input"]["kind"] == "video" {
            VisualKind::Video
        } else {
            VisualKind::Image
        };
        assert_eq!(
            service::normalize_request(&model, &row["input"], kind),
            row["expected"]
        );
    }
    for row in cases["fallback"].as_array().unwrap() {
        let current = types::VisualModel {
            public: json!({"providerId":"first"}),
            key: Value::Null,
            headers: Default::default(),
            visual: false,
            configured: false,
            score: 0,
        };
        let mut next = current.clone();
        if row["same"] == false {
            next.public["providerId"] = json!("second");
        }
        let error = VisualError::with_status(
            row["message"].as_str().unwrap(),
            row["status"].as_u64().unwrap() as u16,
        );
        assert_eq!(
            service::can_fallback(&error, &Default::default(), &next, &current),
            row["expected"].as_bool().unwrap()
        );
    }
}

#[tokio::test]
async fn real_native_tool_context_progress_result_and_archive_failure_retain_file() {
    use pi_rust::coding_agent::{
        extensions::{
            loader::ExtensionRuntime,
            runner::ExtensionRunner,
            types::{AgentToolUpdateCallbackValue, NoopProviderRegistry},
        },
        session_manager::SessionManager,
    };
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    let cwd = fixture.root.to_string_lossy().into_owned();
    let manager = SessionManager::in_memory(&cwd, None, None).unwrap();
    let native_id = manager.get_session_id().to_owned();
    let expected_id = native_id.clone();
    let expected_cwd = fixture.root.clone();
    let context: VisualContextPort = Arc::new(move |id, cwd| {
        assert_eq!(id, expected_id);
        assert_eq!(cwd, expected_cwd);
        Box::pin(async move {
            Ok(VisualToolContext {
                cwd,
                session_id: "actual-public-owner".into(),
            })
        })
    });
    let archived = Arc::new(Mutex::new(Vec::new()));
    let observed = archived.clone();
    let generated: VisualGeneratedFilePort = Arc::new(move |context, result| {
        observed.lock().unwrap().push((context, result));
        Box::pin(async { Err(VisualError::new("actual archive failure")) })
    });
    let tool = create_tool(fixture.service.clone(), context, generated);
    let ctx = ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        &cwd,
        Arc::new(Mutex::new(manager)),
        Arc::new(NoopProviderRegistry),
    )
    .create_context();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let observed = updates.clone();
    let update: AgentToolUpdateCallbackValue =
        Arc::new(move |value| observed.lock().unwrap().push(value.clone()));
    let result = tool.execute_async.as_ref().unwrap()(
        "actual-visual-call".into(),
        json!({"kind":"image","prompt":"real tool file","cwd":"must be ignored"}),
        None,
        Some(update),
        ctx.clone(),
    )
    .await
    .unwrap();
    let path = PathBuf::from(result["details"]["path"].as_str().unwrap());
    assert!(path.starts_with(&fixture.root));
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        STANDARD.decode(PNG).unwrap()
    );
    assert!(!updates.lock().unwrap().is_empty());
    assert_eq!(
        archived.lock().unwrap()[0].0.session_id,
        "actual-public-owner"
    );
    assert_eq!(fixture.records().len(), 1);
    let already_aborted = Arc::new(pi_rust::coding_agent::extensions::types::AbortSignal::new());
    already_aborted.abort();
    assert!(tool.execute_async.as_ref().unwrap()(
        "already-aborted-call".into(),
        json!({"kind":"image","prompt":"cancelled request"}),
        Some(already_aborted),
        None,
        ctx
    )
    .await
    .unwrap_err()
    .contains("aborted"));
    assert_eq!(fixture.records().len(), 1);
    assert_eq!(archived.lock().unwrap().len(), 1);
    fixture.close().await;
}

fn add_backup(fixture: &Fixture, kind: VisualKind, other_provider: bool) {
    let mut config = fixture.config.lock().unwrap();
    let definition = json!({"id":"backup-model","name":"Backup model","kind":kind.as_str(),"visualApi":format!("openai-{}",kind.as_str())});
    if other_provider {
        config.models_json["providers"]["backup"] = json!({"name":"Backup Provider","api":"openai-responses","baseUrl":format!("{}/backup-v1",fixture.state.origin),"models":[definition]});
        config.auth_json["backup"] = json!({"type":"api_key","key":"synthetic-backup-key"});
        config.app_json["providerTypes"]["backup"] = json!("visual");
    } else {
        config.models_json["providers"]["fixture"]["models"]
            .as_array_mut()
            .unwrap()
            .push(definition);
    }
    config.app_json["visualDefaultModels"][kind.as_str()] = json!("fixture/gpt-image-fixture");
}

#[tokio::test]
async fn automatic_model_fallback_explicit_and_paid_boundaries_are_real_posts() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    add_backup(&fixture, VisualKind::Image, false);
    fixture.mode("model-fallback");
    let updates = Arc::new(Mutex::new(Vec::new()));
    let observed = updates.clone();
    let result = fixture
        .service
        .generate(
            fixture.input(VisualKind::Image),
            VisualOptions {
                on_progress: Some(Arc::new(move |value| observed.lock().unwrap().push(value))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.fallback_used);
    assert_eq!(
        result.attempted_models,
        ["fixture/gpt-image-fixture", "fixture/backup-model"]
    );
    assert_eq!(result.model, "backup-model");
    assert_eq!(
        tokio::fs::read(&result.path).await.unwrap(),
        STANDARD.decode(PNG).unwrap()
    );
    assert!(updates.lock().unwrap().iter().any(|value| value
        == "Fixture Provider / Fixture model 当前不可用，尝试 Fixture Provider / Backup model…"));
    assert_eq!(
        fixture
            .records()
            .iter()
            .map(|value| serde_json::from_slice::<Value>(&value.body).unwrap()["model"].clone())
            .collect::<Vec<_>>(),
        [json!("gpt-image-fixture"), json!("backup-model")]
    );
    for (explicit, paid) in [(true, false), (false, true)] {
        fixture.state.requests.lock().unwrap().clear();
        let mut request = fixture.input(VisualKind::Image);
        if explicit {
            request.input["model"] = json!("\u{feff}fixture/gpt-image-fixture\u{2028}");
        }
        let error = fixture
            .service
            .generate(
                request,
                VisualOptions {
                    allow_fallback: !paid,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.status, Some(503));
        assert_eq!(fixture.records().len(), 1);
    }
    fixture.close().await;
}

#[tokio::test]
async fn authorization_cross_provider_fallback_and_safety_stop_use_real_responses() {
    for other_provider in [false, true] {
        let fixture = Fixture::new("openai-image", VisualKind::Image).await;
        add_backup(&fixture, VisualKind::Image, other_provider);
        fixture.mode("auth-fallback");
        let result = fixture
            .service
            .generate(fixture.input(VisualKind::Image), Default::default())
            .await;
        if other_provider {
            let result = result.unwrap();
            assert_eq!(result.provider, "backup");
            assert!(result.fallback_used);
            let records = fixture.records();
            assert_eq!(records.len(), 2);
            assert_eq!(
                records[1].headers["authorization"],
                "Bearer synthetic-backup-key"
            );
        } else {
            assert_eq!(result.unwrap_err().status, Some(401));
            assert_eq!(fixture.records().len(), 1);
        }
        fixture.state.requests.lock().unwrap().clear();
        fixture.mode("safety");
        assert!(fixture
            .service
            .generate(fixture.input(VisualKind::Image), Default::default())
            .await
            .unwrap_err()
            .message
            .contains("Content policy safety rejection"));
        assert_eq!(fixture.records().len(), 1);
        fixture.close().await;
    }
}

#[tokio::test]
async fn all_video_families_poll_real_http_and_download_after_five_second_wait() {
    for (driver, poll_path, remote) in [
        ("openai-video", "/v1/videos/video-1", "video-1"),
        (
            "google-video",
            "/v1/operations/google-video",
            "operations/google-video",
        ),
        (
            "xai-video",
            "/v1/videos/generations/video-relay",
            "video-relay",
        ),
        ("new-api-video", "/v1/videos/video-1", "video-1"),
    ] {
        let fixture = Fixture::new(driver, VisualKind::Video).await;
        fixture.mode("polling");
        let waited = Arc::new(tokio::sync::Notify::new());
        let notify = waited.clone();
        let updates = Arc::new(Mutex::new(Vec::new()));
        let observed = updates.clone();
        let service = fixture.service.clone();
        let mut request = fixture.input(VisualKind::Video);
        request.input["durationSeconds"] = json!(8);
        let task = tokio::spawn(async move {
            service
                .generate(
                    request,
                    VisualOptions {
                        on_progress: Some(Arc::new(move |message| {
                            if message.starts_with("视频生成中") {
                                notify.notify_one();
                            }
                            observed.lock().unwrap().push(message);
                        })),
                        ..Default::default()
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), waited.notified())
            .await
            .unwrap();
        assert!(
            !fixture
                .records()
                .iter()
                .any(|value| value.path == poll_path),
            "{driver}"
        );
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_or_else(|error| panic!("{driver}: {error}"));
        assert_eq!(result.remote_id.as_deref(), Some(remote));
        assert_eq!(tokio::fs::read(&result.path).await.unwrap(), MP4);
        let records = fixture.records();
        let poll = records
            .iter()
            .find(|value| value.path == poll_path)
            .unwrap();
        assert_eq!(poll.method, Method::GET);
        assert_eq!(
            records
                .iter()
                .filter(|value| value.method == Method::POST)
                .count(),
            1
        );
        if driver == "google-video" {
            assert_eq!(poll.headers["content-type"], "application/json");
            let download = records
                .iter()
                .find(|value| value.path == "/video.mp4")
                .unwrap();
            assert_eq!(download.headers["x-goog-api-key"], "synthetic-visual-key");
            assert!(!download.headers.contains_key("x-test-header"));
            assert!(!download.headers.contains_key("authorization"));
        }
        assert!(updates
            .lock()
            .unwrap()
            .iter()
            .any(|value| value.starts_with("视频生成中")));
        fixture.close().await;
    }
}

#[tokio::test]
async fn cancellation_during_polling_wait_never_polls_or_recreates_paid_job() {
    let fixture = Fixture::new("openai-video", VisualKind::Video).await;
    fixture.mode("polling");
    let waited = Arc::new(tokio::sync::Notify::new());
    let notify = waited.clone();
    let token = CancellationToken::new();
    let cancellation = token.clone();
    let service = fixture.service.clone();
    let request = fixture.input(VisualKind::Video);
    let task = tokio::spawn(async move {
        service
            .generate(
                request,
                VisualOptions {
                    cancellation,
                    on_progress: Some(Arc::new(move |message| {
                        if message.starts_with("视频生成中") {
                            notify.notify_one();
                        }
                    })),
                    ..Default::default()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), waited.notified())
        .await
        .unwrap();
    token.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .cancelled
    );
    assert_eq!(fixture.records().len(), 1);
    assert_eq!(fixture.service.live_count(), 0);
    fixture.close().await;
}

#[tokio::test]
async fn sdk_video_content_error_keeps_status_and_can_fallback_to_next_model() {
    let fixture = Fixture::new("openai-video", VisualKind::Video).await;
    add_backup(&fixture, VisualKind::Video, true);
    fixture.mode("content-fallback");
    // Both POST paths return a completed job; SDK downloads must keep 429.
    let result = fixture
        .service
        .generate(fixture.input(VisualKind::Video), Default::default())
        .await
        .unwrap();
    assert_eq!(result.provider, "backup");
    assert!(result.fallback_used);
    assert_eq!(
        result.attempted_models,
        ["fixture/gpt-image-fixture", "backup/backup-model"]
    );
    let records = fixture.records();
    assert_eq!(
        records
            .iter()
            .filter(|value| value.method == Method::POST)
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|value| value.path.ends_with("/content"))
            .count(),
        2
    );
    fixture.close().await;
}

#[tokio::test]
async fn sdk_nullable_array_headers_and_fetch_coercion_have_distinct_http_records() {
    for driver in ["openai-image", "google-image", "xai-image"] {
        let fixture = Fixture::new(driver, VisualKind::Image).await;
        fixture.config.lock().unwrap().models_json["providers"]["fixture"]["headers"] =
            json!({"Authorization":null,"X-Test":["a","b"],"Content-Type":null});
        fixture
            .service
            .generate(fixture.input(VisualKind::Image), Default::default())
            .await
            .unwrap();
        let record = fixture
            .records()
            .into_iter()
            .find(|value| value.method == Method::POST)
            .unwrap();
        if driver == "openai-image" {
            assert!(!record.headers.contains_key("authorization"));
            assert_eq!(record.headers["x-test"], "a, b");
            assert_eq!(record.headers["content-type"], "application/json");
        } else {
            assert_eq!(record.headers["authorization"], "null");
            assert_eq!(record.headers["x-test"], "a,b");
            assert_eq!(
                record.headers["content-type"],
                if driver == "google-image" {
                    "null"
                } else {
                    "application/json"
                }
            );
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn js_whitespace_in_preference_and_source_path_is_removed_at_real_boundary() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    let selected = fixture
        .service
        .set_preferred_model(
            VisualKind::Image,
            &json!("\u{feff}fixture/gpt-image-fixture\u{2028}"),
        )
        .await
        .unwrap();
    assert_eq!(selected["imageSelection"], "fixture/gpt-image-fixture");
    let source = fixture.root.join("outside-source.png");
    tokio::fs::write(&source, STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    let mut request = fixture.input(VisualKind::Image);
    request.input["sourceImages"] = json!([format!("\u{feff}{}\u{2028}", source.display())]);
    let result = fixture
        .service
        .generate(request, Default::default())
        .await
        .unwrap();
    assert_eq!(result.operation, VisualOperation::Edit);
    assert_eq!(fixture.records().len(), 1);
    fixture.close().await;
}

#[tokio::test]
async fn dropped_caller_and_shutdown_each_settle_live_http_and_prevent_new_requests() {
    for abort_caller in [false, true] {
        let fixture = Fixture::new("openai-image", VisualKind::Image).await;
        fixture.mode("stall");
        let service = fixture.service.clone();
        let request = fixture.input(VisualKind::Image);
        let task = tokio::spawn(async move { service.generate(request, Default::default()).await });
        tokio::time::timeout(Duration::from_secs(2), fixture.state.arrived.notified())
            .await
            .unwrap();
        if abort_caller {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            tokio::time::timeout(Duration::from_secs(2), fixture.service.dispose())
                .await
                .unwrap();
            assert!(task.await.unwrap().unwrap_err().cancelled);
        }
        tokio::time::timeout(Duration::from_secs(2), fixture.service.dispose())
            .await
            .unwrap();
        assert_eq!(fixture.service.live_count(), 0);
        assert!(
            fixture
                .service
                .generate(fixture.input(VisualKind::Image), Default::default())
                .await
                .unwrap_err()
                .cancelled
        );
        assert_eq!(fixture.records().len(), 1);
        fixture.close().await;
    }
}

#[tokio::test]
async fn configured_header_arrays_are_redacted_after_real_upstream_error() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    fixture.config.lock().unwrap().models_json["providers"]["fixture"]["headers"] =
        json!({"X-Secret":["synthetic-secret-1","synthetic-secret-2"]});
    fixture.mode("secret-error");
    let error = fixture
        .service
        .generate(fixture.input(VisualKind::Image), Default::default())
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(400));
    assert_eq!(error.message, "400 [REDACTED], [REDACTED]");
    assert_eq!(
        fixture.records()[0].headers["x-secret"],
        "synthetic-secret-1, synthetic-secret-2"
    );
    fixture.close().await;
}

#[tokio::test]
async fn sdk_multipart_custom_content_type_and_fetch_case_append_match_release() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    fixture.config.lock().unwrap().models_json["providers"]["fixture"]["headers"] =
        json!({"Content-Type":"ignored-custom"});
    let source = fixture.root.join("source.png");
    tokio::fs::write(&source, STANDARD.decode(PNG).unwrap())
        .await
        .unwrap();
    let mut request = fixture.input(VisualKind::Image);
    request.input["sourceImages"] = json!([source]);
    fixture
        .service
        .generate(request, Default::default())
        .await
        .unwrap();
    assert_eq!(
        fixture.records()[0].headers["content-type"],
        "ignored-custom"
    );
    fixture.close().await;
    let fixture = Fixture::new("xai-image", VisualKind::Image).await;
    fixture.config.lock().unwrap().models_json["providers"]["fixture"]["headers"] =
        json!({"authorization":"override"});
    fixture
        .service
        .generate(fixture.input(VisualKind::Image), Default::default())
        .await
        .unwrap();
    let post = fixture
        .records()
        .into_iter()
        .find(|value| value.method == Method::POST)
        .unwrap();
    assert_eq!(
        post.headers["authorization"],
        "Bearer synthetic-visual-key, override"
    );
    fixture.close().await;
}

#[tokio::test]
async fn google_four_k_source_parts_and_xai_edit_parameters_reach_real_providers() {
    for driver in ["google-image", "xai-image"] {
        let fixture = Fixture::new(driver, VisualKind::Image).await;
        let source = fixture.root.join("source.png");
        let mask = fixture.root.join("mask.png");
        tokio::fs::write(&source, STANDARD.decode(PNG).unwrap())
            .await
            .unwrap();
        tokio::fs::write(&mask, STANDARD.decode(PNG).unwrap())
            .await
            .unwrap();
        let mut request = fixture.input(VisualKind::Image);
        request.input["sourceImages"] = json!([source]);
        request.input["maskPath"] = json!(mask);
        request.input["imageSize"] = json!("4K");
        request.input["quality"] = json!("hd");
        request.input["outputFormat"] = json!("webp");
        fixture
            .service
            .generate(request, Default::default())
            .await
            .unwrap();
        let post = fixture
            .records()
            .into_iter()
            .find(|value| value.method == Method::POST)
            .unwrap();
        if driver == "google-image" {
            let body: Value = serde_json::from_slice(&post.body).unwrap();
            assert_eq!(
                body["generationConfig"]["imageConfig"],
                json!({"imageSize":"4K","aspectRatio":"16:9"})
            );
            assert_eq!(body["contents"][0]["parts"][0]["inlineData"]["data"], PNG);
            assert_eq!(
                body["contents"][0]["parts"][1]["text"],
                "controlled fixture result"
            );
            assert!(body.get("quality").is_none());
        } else {
            let body = String::from_utf8_lossy(&post.body);
            assert!(body.contains("filename=\"image-1\""));
            assert!(body.contains("filename=\"mask.png\""));
            assert!(body.contains("name=\"quality\"") && body.contains("hd"));
            assert!(!body.contains("name=\"output_format\"") && !body.contains("name=\"n\""));
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn raw_null_config_documents_error_without_normalizing_or_provider_http() {
    let fixture = Fixture::new("openai-image", VisualKind::Image).await;
    let initial = fixture.config.lock().unwrap().clone();
    for (field, message) in [
        (
            "app",
            "Cannot read properties of null (reading 'disabledProviders')",
        ),
        (
            "models",
            "Cannot read properties of null (reading 'providers')",
        ),
        ("auth", "Cannot read properties of null (reading 'fixture')"),
    ] {
        let mut value = initial.clone();
        match field {
            "app" => value.app_json = Value::Null,
            "models" => value.models_json = Value::Null,
            _ => value.auth_json = Value::Null,
        }
        *fixture.config.lock().unwrap() = value;
        assert_eq!(
            fixture.service.get_all_status().await.unwrap_err().message,
            message
        );
    }
    for primitive in [json!(false), json!(0), json!("raw document"), json!([])] {
        let mut value = initial.clone();
        value.app_json = primitive;
        *fixture.config.lock().unwrap() = value;
        assert!(fixture.service.get_all_status().await.is_ok());
    }
    assert!(fixture.records().is_empty());
    fixture.close().await;
}
