use super::*;
use crate::native_visual::{VisualConfigPort, VisualConfigSnapshot};
use axum::{extract::State, http::Method, routing::post};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgAAAAAgABVKJPXQAAAABJRU5ErkJggg==";
struct Server {
    url: String,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Server {
    async fn new(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            url,
            cancel,
            task: Some(task),
        }
    }
    async fn close(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
struct Fixture {
    root: PathBuf,
    config: Arc<Mutex<VisualConfigSnapshot>>,
    writes: Arc<Mutex<Vec<(VisualKind, Option<String>)>>>,
    service: Arc<VisualGenerationService>,
    api: Server,
    provider: Server,
    requests: Arc<Mutex<Vec<Value>>>,
    provider_bytes: Arc<Mutex<Vec<u8>>>,
}
impl Fixture {
    async fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("pisper-visual-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider_bytes = Arc::new(Mutex::new(STANDARD.decode(PNG).unwrap()));
        let provider = Server::new(
            Router::new()
                .route("/v1/images/generations", post(provider_response))
                .with_state((requests.clone(), provider_bytes.clone())),
        )
        .await;
        let config = Arc::new(Mutex::new(VisualConfigSnapshot {
            models_json: json!({"providers":{"fixture":{"name":"Local Visual Fixture","api":"openai-completions","baseUrl":format!("{}/v1",provider.url),"models":[{"id":"gpt-image-fixture","kind":"image","visualApi":"openai-image"},{"id":"true","kind":"image","visualApi":"openai-image"},{"id":"video-fixture","kind":"video","visualApi":"openai-video"}]}}}),
            auth_json: json!({"fixture":{"type":"api_key","key":"synthetic-http-key"}}),
            app_json: json!({"providerTypes":{"fixture":"visual"},"unknown":{"retain":true}}),
            ..Default::default()
        }));
        let read = config.clone();
        let write = config.clone();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let updates = writes.clone();
        let service = VisualGenerationService::new(VisualConfigPort {
            read: Arc::new(move || {
                let value = read.lock().unwrap().clone();
                Box::pin(async move { Ok(value) })
            }),
            write_preference: Arc::new(move |kind, reference| {
                let config = write.clone();
                let updates = updates.clone();
                Box::pin(async move {
                    updates.lock().unwrap().push((kind, reference.clone()));
                    let mut config = config.lock().unwrap();
                    if !config.app_json["visualDefaultModels"].is_object() {
                        config.app_json["visualDefaultModels"] = json!({});
                    }
                    let models = config.app_json["visualDefaultModels"]
                        .as_object_mut()
                        .unwrap();
                    match reference {
                        Some(value) => {
                            models.insert(kind.as_str().into(), json!(value));
                        }
                        None => {
                            models.remove(kind.as_str());
                        }
                    }
                    Ok(())
                })
            }),
        });
        let api = Server::new(router(service.clone(), root.clone())).await;
        Self {
            root,
            config,
            writes,
            service,
            api,
            provider,
            requests,
            provider_bytes,
        }
    }
    async fn request(
        &self,
        method: Method,
        route: &str,
        body: impl Into<reqwest::Body>,
    ) -> (StatusCode, Value) {
        let response = reqwest::Client::new()
            .request(method, format!("{}{route}", self.api.url))
            .body(body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let value = response.json().await.unwrap();
        (status, value)
    }
    async fn close(&mut self) {
        self.service.dispose().await;
        self.api.close().await;
        self.provider.close().await;
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(actual) = std::fs::canonicalize(&self.root) {
            assert_eq!(
                actual.parent(),
                Some(
                    std::fs::canonicalize(std::env::temp_dir())
                        .unwrap()
                        .as_path()
                )
            );
            assert!(actual
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-visual-http-"));
            std::fs::remove_dir_all(actual).unwrap();
        }
    }
}
async fn provider_response(
    State((requests, bytes)): State<(Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<u8>>>)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    requests.lock().unwrap().push(body);
    Json(json!({"data":[{"b64_json":STANDARD.encode(bytes.lock().unwrap().as_slice())}]}))
}

#[tokio::test]
async fn models_and_preferences_keep_release_six_fields_and_js_scalar_defaults() {
    let mut fixture = Fixture::new().await;
    let (status, value) = fixture.request(Method::GET, "/api/visual/models", "").await;
    assert_eq!(status, StatusCode::OK);
    let mut keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "image",
            "imageModels",
            "imageSelection",
            "video",
            "videoModels",
            "videoSelection"
        ]
    );
    assert!(!value.to_string().contains("synthetic-http-key"));
    assert_eq!(value["imageSelection"], "");
    assert_eq!(value["videoModels"].as_array().unwrap().len(), 1);
    let (status, value) = fixture
        .request(
            Method::PUT,
            "/api/visual/models/image",
            r#"{"model":"fixture/gpt-image-fixture"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["imageSelection"], "fixture/gpt-image-fixture");
    let (status, value) = fixture
        .request(
            Method::PUT,
            "/api/visual/models/video",
            r#"{"model":"fixture/video-fixture"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["videoSelection"], "fixture/video-fixture");
    let (status, value) = fixture
        .request(Method::PUT, "/api/visual/models/image", r#"{"model":true}"#)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["imageSelection"], "fixture/true");
    for body in [
        "",
        "null",
        "[]",
        "true",
        "123",
        r#""scalar""#,
        "{}",
        r#"{"model":false}"#,
        r#"{"model":0}"#,
        r#"{"model":null}"#,
        r#"{"model":"\uFEFF\u2028"}"#,
    ] {
        let (status, value) = fixture
            .request(Method::PUT, "/api/visual/models/image", body)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}: {value}");
        assert_eq!(value["imageSelection"], "");
        assert_eq!(value["videoSelection"], "fixture/video-fixture");
    }
    assert_eq!(
        fixture.config.lock().unwrap().app_json["unknown"],
        json!({"retain":true})
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), 0);
    fixture.close().await;
}
#[tokio::test]
async fn malformed_json_and_constrained_kinds_return_json_errors_without_mutation() {
    let mut fixture = Fixture::new().await;
    for body in [" ", "{", r#"{"model":}"#] {
        let (status, value) = fixture
            .request(Method::PUT, "/api/visual/models/image", body)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(value["error"].is_string());
        assert_eq!(value.as_object().unwrap().len(), 1);
    }
    for kind in ["audio", "Image", "IMAGE"] {
        let (status, value) = fixture
            .request(
                Method::PUT,
                &format!("/api/visual/models/{kind}"),
                "not-json",
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(value, json!({"error":"接口不存在。"}));
    }
    for (method, route) in [
        (Method::POST, "/api/visual/models"),
        (Method::GET, "/api/visual/test"),
        (Method::PATCH, "/api/visual/models/image"),
    ] {
        let (status, value) = fixture.request(method, route, "not-json").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(value, json!({"error":"接口不存在。"}));
    }
    let head = reqwest::Client::new()
        .head(format!("{}/api/visual/models", fixture.api.url))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(fixture.requests.lock().unwrap().is_empty());
    let (status, value) = fixture
        .request(
            Method::PUT,
            "/api/visual/models/image",
            r#"{"model":"fixture/missing"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("未找到已启用的视觉模型"));
    assert!(!value.to_string().contains("synthetic-http-key"));
    fixture.close().await;
}
#[tokio::test]
async fn test_post_ignores_body_uses_fixed_prompt_and_saves_real_preview_inside_data_root() {
    let mut fixture = Fixture::new().await;
    let (status, value) = fixture
        .request(
            Method::POST,
            "/api/visual/test",
            "malformed-json-is-ignored",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["kind"], "image");
    assert_eq!(value["operation"], "generate");
    assert_eq!(value["provider"], "fixture");
    let path = Path::new(value["path"].as_str().unwrap());
    assert!(path.starts_with(fixture.root.join("visual-test/generated/visuals")));
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with("-config-test.png"));
    let bytes = tokio::fs::read(path).await.unwrap();
    assert_eq!(bytes, STANDARD.decode(PNG).unwrap());
    assert_eq!(value["size"], bytes.len());
    assert_eq!(
        value["previewDataUrl"],
        format!("data:image/png;base64,{PNG}")
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["prompt"],"a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background");
    drop(requests);
    fixture.close().await;
}
#[tokio::test]
async fn upstream_status_and_secret_redaction_follow_the_shared_release_error_boundary() {
    let service = VisualGenerationService::new(VisualConfigPort {
        read: Arc::new(|| {
            Box::pin(async {
                Err(VisualError::with_status(
                    "Bearer synthetic-private-token",
                    503,
                ))
            })
        }),
        write_preference: Arc::new(|_, _| Box::pin(async { Ok(()) })),
    });
    let root = std::env::temp_dir().join(format!("pisper-visual-http-{}", uuid::Uuid::new_v4()));
    let mut server = Server::new(router(service.clone(), root)).await;
    let response = reqwest::get(format!("{}/api/visual/models", server.url))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value.as_object().unwrap().len(), 1);
    assert!(!value.to_string().contains("synthetic-private-token"));
    service.dispose().await;
    server.close().await;
    let response =
        Failure(VisualError::with_status("invalid out-of-range status", 700)).into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_is_included_at_six_mib_and_omitted_above_without_losing_the_saved_file() {
    let mut fixture = Fixture::new().await;
    for size in [6 * 1024 * 1024, 6 * 1024 * 1024 + 1] {
        let mut bytes = STANDARD.decode(PNG).unwrap();
        bytes.resize(size, 0);
        *fixture.provider_bytes.lock().unwrap() = bytes.clone();
        let (status, value) = fixture
            .request(
                Method::POST,
                "/api/visual/test",
                r#"{"kind":"video","prompt":"ignored","cwd":"outside","model":"missing"}"#,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["kind"], "image");
        assert_eq!(value["size"], size);
        assert_eq!(
            tokio::fs::read(value["path"].as_str().unwrap())
                .await
                .unwrap(),
            bytes
        );
        let preview = value["previewDataUrl"].as_str().unwrap();
        if size == 6 * 1024 * 1024 {
            assert_eq!(
                STANDARD
                    .decode(preview.strip_prefix("data:image/png;base64,").unwrap())
                    .unwrap(),
                bytes
            );
        } else {
            assert_eq!(preview, "");
        }
    }
    assert!(fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request["prompt"] != "ignored"));
    fixture.close().await;
}
