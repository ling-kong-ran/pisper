//! 独立工作台的 HTTP 契约，所有素材均属于 game-assets 的媒体实例。
use crate::{
    native_game_assets::{protocol, GameAssetsService},
    native_image_runtime::engines::EngineService,
    native_workflow::{
        image_nodes::{ImageNodeService, ImageOperationRequest},
        image_protocol, inputs,
        media::MediaService,
        Result, WorkflowError,
    },
    workflow_engine::RunCancellation,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub(crate) type ModelCatalog = Arc<dyn Fn() -> BoxFuture<'static, Result<Value>> + Send + Sync>;
#[derive(Clone)]
struct Api {
    service: Arc<GameAssetsService>,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    engines: Arc<EngineService>,
    models: ModelCatalog,
}
struct Failure(WorkflowError);
impl From<WorkflowError> for Failure {
    fn from(error: WorkflowError) -> Self {
        Self(error)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let error = self.0;
        let recognized = error.code.starts_with("game_assets_")
            || error.code.starts_with("workflow_input_")
            || [
                "workflow_image_invalid",
                "workflow_image_source_required",
                "workflow_image_too_large",
                "workflow_image_cancelled",
                "workflow_image_closed",
                "workflow_image_generation_failed",
                "workflow_image_processing_failed",
                "workflow_image_timeout",
                "workflow_image_invalid_edits",
                "workflow_media_invalid",
                "workflow_media_missing",
                "workflow_media_too_large",
                "sprite_engine_missing",
                "sprite_engine_invalid",
                "sprite_engine_download_failed",
            ]
            .contains(&error.code.as_str());
        let (status, code) = if recognized {
            (error.status, error.code)
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "game_assets_failed".into(),
            )
        };
        (status, Json(json!({"error":code,"code":code}))).into_response()
    }
}
type HttpResult<T> = std::result::Result<T, Failure>;
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(
    service: Arc<GameAssetsService>,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    engines: Arc<EngineService>,
    models: ModelCatalog,
) -> Router<S> {
    Router::new()
        .route("/api/game-assets", get(catalog))
        .route("/api/game-assets/projects", post(create))
        .route(
            "/api/game-assets/projects/{projectId}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/game-assets/projects/{projectId}/run", post(run))
        .route("/api/game-assets/jobs/{jobId}", get(job))
        .route("/api/game-assets/jobs/{jobId}/stop", post(stop))
        .route("/api/game-assets/jobs/{jobId}/frames", post(edit))
        .route("/api/game-assets/media", post(upload))
        .route("/api/game-assets/media/{mediaId}/content", get(content))
        .route("/api/game-assets/process", post(process))
        .route(
            "/api/game-assets/engines/{engineId}/download",
            post(download),
        )
        .route("/api/game-assets/engines/{engineId}/cancel", post(cancel))
        .with_state(Api {
            service,
            media,
            images,
            engines,
            models,
        })
}
async fn body(request: Request) -> Result<Value> {
    let bytes = to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(|_| protocol::invalid())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| protocol::invalid())?;
    if !value.is_object() {
        return Err(protocol::invalid());
    }
    Ok(value)
}
async fn catalog(State(api): State<Api>) -> HttpResult<Json<Value>> {
    let (catalog, models, engines) =
        tokio::try_join!(api.service.catalog(), (api.models)(), api.engines.catalog())?;
    let mut result = catalog;
    result["models"] = json!(models
        .as_array()
        .ok_or_else(protocol::invalid)?
        .iter()
        .map(|model| {
        json!({"id":format!("{}/{}",model["providerId"].as_str().unwrap_or(""),model["id"].as_str().unwrap_or("")),"name":model["name"],"providerId":model["providerId"],
            "providerName":model.get("providerName").unwrap_or(&model["providerId"])})
        })
        .collect::<Vec<_>>());
    result["engines"] = engines["engines"].clone();
    Ok(Json(result))
}
async fn create(State(api): State<Api>, request: Request) -> HttpResult<(StatusCode, Json<Value>)> {
    let input = protocol::project_input(&body(request).await?)?;
    if input.get("id").is_some() {
        return Err(protocol::invalid().into());
    }
    Ok((StatusCode::CREATED, Json(api.service.save(input).await?)))
}
async fn update(
    State(api): State<Api>,
    Path(id): Path<String>,
    request: Request,
) -> HttpResult<Json<Value>> {
    protocol::id(&id)?;
    let mut input = body(request).await?;
    if input
        .get("id")
        .is_some_and(|value| value.as_str() != Some(id.as_str()))
    {
        return Err(protocol::invalid().into());
    }
    input["id"] = json!(id);
    Ok(Json(api.service.save(input).await?))
}
async fn remove(State(api): State<Api>, Path(id): Path<String>) -> HttpResult<Json<Value>> {
    protocol::id(&id)?;
    api.service.remove(&id).await?;
    Ok(Json(json!({"deleted":true})))
}
async fn run(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> HttpResult<(StatusCode, Json<Value>)> {
    protocol::id(&id)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job":api.service.run(&id).await?})),
    ))
}
async fn job(State(api): State<Api>, Path(id): Path<String>) -> HttpResult<Json<Value>> {
    protocol::id(&id)?;
    Ok(Json(api.service.get_job(&id).await?.ok_or_else(|| {
        protocol::error("game_assets_not_found", StatusCode::NOT_FOUND)
    })?))
}
async fn stop(State(api): State<Api>, Path(id): Path<String>) -> HttpResult<Json<Value>> {
    protocol::id(&id)?;
    Ok(Json(json!({"job":api.service.stop(&id).await?})))
}
async fn edit(
    State(api): State<Api>,
    Path(id): Path<String>,
    request: Request,
) -> HttpResult<Json<Value>> {
    let value = body(request).await?;
    protocol::record(&value, &["frames"])?;
    let edits = protocol::edits(&value)?;
    protocol::id(&id)?;
    Ok(Json(api.service.edit(&id, edits).await?))
}
async fn upload(
    State(api): State<Api>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> HttpResult<(StatusCode, Json<Value>)> {
    let mime = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !["image/png", "image/jpeg", "image/webp"].contains(&mime.as_str()) {
        return Err(protocol::error(
            "game_assets_media_invalid",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )
        .into());
    }
    if request
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<f64>().ok())
        .is_some_and(|size| size > 8.0 * 1024.0 * 1024.0)
    {
        return Err(
            protocol::error("workflow_media_too_large", StatusCode::PAYLOAD_TOO_LARGE).into(),
        );
    }
    let bytes = to_bytes(request.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|_| protocol::error("game_assets_media_invalid", StatusCode::BAD_REQUEST))?;
    Ok((
        StatusCode::CREATED,
        Json(
            api.media
                .upload(
                    query
                        .get("name")
                        .filter(|name| !name.is_empty())
                        .map(String::as_str)
                        .unwrap_or("reference.png"),
                    &mime,
                    &bytes,
                )
                .await?,
        ),
    ))
}
async fn content(State(api): State<Api>, Path(id): Path<String>) -> HttpResult<Response> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(protocol::error("game_assets_media_invalid", StatusCode::BAD_REQUEST).into());
    }
    let media = api.media.read(&id).await?;
    let mime = media.metadata["media"]["mimeType"].as_str().unwrap_or("");
    if !mime.starts_with("image/") {
        return Err(protocol::error("game_assets_media_invalid", StatusCode::BAD_REQUEST).into());
    }
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Content-Length", media.buffer.len())
        .header("Cache-Control", "private, max-age=60")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(media.buffer))
        .map_err(|_| {
            Failure(protocol::error(
                "game_assets_failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        })
}
struct CancelOnDrop(Arc<RunCancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
async fn process(State(api): State<Api>, request: Request) -> HttpResult<Json<Value>> {
    let input = body(request).await?;
    protocol::record(&input, &["reference", "operation", "image"])?;
    let operation = input["operation"]
        .as_str()
        .filter(|operation| ["background", "inpaint"].contains(operation))
        .ok_or_else(protocol::invalid)?;
    let source = inputs::media(&input["reference"])?;
    if !source["mimeType"]
        .as_str()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return Err(protocol::error("game_assets_media_invalid", StatusCode::BAD_REQUEST).into());
    }
    let settings = image_protocol::settings(input.get("image"))?;
    let cancellation = Arc::new(RunCancellation::default());
    let _owner = CancelOnDrop(cancellation.clone());
    let source = api
        .images
        .operate(
            ImageOperationRequest {
                operation: "input".into(),
                source: Some(source),
                images: vec![],
                settings: json!({}),
                prompt: String::new(),
                model: Value::Null,
                resume_output: None,
                edits: None,
            },
            cancellation.clone(),
        )
        .await?;
    let result = api
        .images
        .operate(
            ImageOperationRequest {
                operation: operation.into(),
                source: None,
                images: source.output["frames"].as_array().unwrap().clone(),
                settings,
                prompt: String::new(),
                model: Value::Null,
                resume_output: None,
                edits: None,
            },
            cancellation,
        )
        .await?;
    if result.output["frames"]
        .as_array()
        .is_none_or(|frames| frames.len() != 1)
    {
        return Err(protocol::invalid().into());
    }
    Ok(Json(result.output["frames"][0]["media"].clone()))
}
fn engine_id(id: &str) -> Result<&str> {
    if ["background", "inpaint"].contains(&id) {
        Ok(id)
    } else {
        Err(protocol::invalid())
    }
}
async fn download(
    State(api): State<Api>,
    Path(id): Path<String>,
) -> HttpResult<(StatusCode, Json<Value>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(api.engines.download(engine_id(&id)?).await?),
    ))
}
async fn cancel(State(api): State<Api>, Path(id): Path<String>) -> HttpResult<Json<Value>> {
    Ok(Json(api.engines.cancel(engine_id(&id)?).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_workflow::{
        engine_cache::{release_definitions, EngineCache},
        image_nodes::{GeneratedImage, ImageGenerationRequest, ImageGenerator},
        image_processing::{encode_png, ImageProcessor},
        test_support::TempDirectory,
    };
    use axum::http::Request as HttpRequest;
    use image::{Rgba, RgbaImage};
    use std::time::Duration;
    use tower::ServiceExt;

    struct Generator;
    impl ImageGenerator for Generator {
        fn generate(
            &self,
            request: ImageGenerationRequest,
            _cancel: Arc<RunCancellation>,
        ) -> BoxFuture<'_, Result<GeneratedImage>> {
            Box::pin(async move {
                let directory = request.cwd.join("generated").join("visuals");
                std::fs::create_dir_all(&directory).unwrap();
                let path = directory.join(format!("{}.png", request.output_name));
                let pixels = RgbaImage::from_fn(4, 2, |x, y| {
                    if x % 2 == 0 && y == 0 {
                        Rgba([220, 30, 50, 255])
                    } else {
                        Rgba([255, 0, 255, 255])
                    }
                });
                std::fs::write(&path, encode_png(&pixels).unwrap()).unwrap();
                Ok(GeneratedImage {
                    path,
                    mime_type: "image/png".into(),
                })
            })
        }
    }
    struct Fixture {
        directory: TempDirectory,
        router: Router,
        service: Arc<GameAssetsService>,
        media: Arc<MediaService>,
        images: Arc<ImageNodeService>,
        processor: Arc<ImageProcessor>,
        engines: Arc<EngineService>,
        png: Vec<u8>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = TempDirectory::new();
            let game = directory.path.join("game");
            let media = MediaService::open(&game).unwrap();
            let processor = ImageProcessor::new(None);
            let images = ImageNodeService::open(
                &game,
                media.clone(),
                processor.clone(),
                Arc::new(Generator),
            )
            .unwrap();
            let service = GameAssetsService::open(&game, media.clone(), images.clone()).unwrap();
            let engines = EngineService::open(
                EngineCache::open(&directory.path, release_definitions()).unwrap(),
            )
            .unwrap();
            let models: ModelCatalog = Arc::new(|| {
                Box::pin(async {
                    Ok(json!([{"id":"paint","name":"Paint","providerId":"fixture",
                "providerName":"Fixture","apiKey":"must-not-expose","baseUrl":"secret fixture"}]))
                })
            });
            let router = routes(
                service.clone(),
                media.clone(),
                images.clone(),
                engines.clone(),
                models,
            );
            let png = encode_png(&RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]))).unwrap();
            Self {
                directory,
                router,
                service,
                media,
                images,
                processor,
                engines,
                png,
            }
        }
        async fn send(
            &self,
            path: &str,
            method: &str,
            content_type: &str,
            bytes: Vec<u8>,
        ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
            let request = HttpRequest::builder()
                .uri(path)
                .method(method)
                .header("content-type", content_type)
                .body(Body::from(bytes))
                .unwrap();
            let response = self.router.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec();
            (status, headers, bytes)
        }
        async fn json(&self, path: &str, method: &str, value: Value) -> (StatusCode, Value) {
            let (status, _, bytes) = self
                .send(
                    path,
                    method,
                    "application/json",
                    serde_json::to_vec(&value).unwrap(),
                )
                .await;
            (status, serde_json::from_slice(&bytes).unwrap())
        }
        async fn upload(&self) -> Value {
            let (status, _, bytes) = self
                .send(
                    "/api/game-assets/media?name=reference.png",
                    "POST",
                    "image/png",
                    self.png.clone(),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED);
            serde_json::from_slice(&bytes).unwrap()
        }
        async fn close(&self) {
            self.service.dispose().await.unwrap();
            self.images.dispose().await;
            self.processor.dispose().await;
            self.media.dispose().await;
            self.engines.dispose().await;
        }
    }
    #[tokio::test]
    async fn game_http_contract_catalog_crud_malformed_body_and_isolated_binary_media() {
        let fixture = Fixture::new();
        let (status, catalog) = fixture.json("/api/game-assets", "GET", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(catalog["models"][0]["id"], "fixture/paint");
        assert!(!catalog.to_string().contains("must-not-expose"));
        assert!(!catalog.to_string().contains("baseUrl"));
        let reference = fixture.upload().await;
        let (status, headers, bytes) = fixture
            .send(
                &format!(
                    "/api/game-assets/media/{}/content",
                    reference["id"].as_str().unwrap()
                ),
                "GET",
                "application/json",
                vec![],
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "image/png");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(bytes, fixture.png);
        let (status, project) = fixture
            .json(
                "/api/game-assets/projects",
                "POST",
                json!({"name":"Draft","reference":reference}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, updated) = fixture
            .json(
                &format!(
                    "/api/game-assets/projects/{}",
                    project["id"].as_str().unwrap()
                ),
                "PATCH",
                json!({"name":"Updated","reference":reference}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["name"], "Updated");
        let (status, _) = fixture
            .json(
                &format!(
                    "/api/game-assets/projects/{}",
                    project["id"].as_str().unwrap()
                ),
                "PATCH",
                json!({"name":"Spoof","id":crate::native_workflow::id().unwrap()}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, bytes) = fixture
            .send(
                "/api/game-assets/projects",
                "POST",
                "application/json",
                b"{ broken secret".to_vec(),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"error":"game_assets_invalid","code":"game_assets_invalid"})
        );
        let (status, _, _) = fixture
            .send(
                "/api/game-assets/media",
                "POST",
                "video/mp4",
                fixture.png.clone(),
            )
            .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let foreign = MediaService::open(&fixture.directory.path.join("workflow")).unwrap();
        let other = foreign
            .upload("foreign.png", "image/png", &fixture.png)
            .await
            .unwrap();
        let (status, _) = fixture
            .json(
                &format!(
                    "/api/game-assets/media/{}/content",
                    other["id"].as_str().unwrap()
                ),
                "GET",
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = fixture
            .json(
                "/api/game-assets/projects",
                "POST",
                json!({"name":"Foreign","reference":other}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        for (path, value) in [
            (
                "/api/game-assets/projects",
                json!({"name":"Bad","workflowId":"coupled"}),
            ),
            (
                "/api/game-assets/process",
                json!({"reference":reference,"operation":"generate"}),
            ),
            ("/api/game-assets/engines/unknown/download", json!({})),
        ] {
            assert_eq!(
                fixture.json(path, "POST", value).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let (status, deleted) = fixture
            .json(
                &format!(
                    "/api/game-assets/projects/{}",
                    project["id"].as_str().unwrap()
                ),
                "DELETE",
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deleted, json!({"deleted":true}));
        foreign.dispose().await;
        fixture.close().await;
    }
    #[tokio::test]
    async fn game_http_generate_edit_synchronous_job_export_and_background_process() {
        let fixture = Fixture::new();
        let reference = fixture.upload().await;
        let (status,processed)=fixture.json("/api/game-assets/process","POST",json!({"reference":reference,"operation":"background","image":{"colors":["#000000"]}})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(processed["mimeType"], "image/png");
        let pixels = image::load_from_memory(
            &fixture
                .media
                .read(processed["id"].as_str().unwrap())
                .await
                .unwrap()
                .buffer,
        )
        .unwrap()
        .into_rgba8();
        assert!(pixels.pixels().all(|pixel| pixel[3] == 0));
        let (_, project) = fixture
            .json(
                "/api/game-assets/projects",
                "POST",
                json!({"name":"Character","reference":reference,"frameCount":2,"directions":["S"],
            "actions":[{"id":"idle","name":"Idle","enabled":true}]}),
            )
            .await;
        let (status, started) = fixture
            .json(
                &format!(
                    "/api/game-assets/projects/{}/run",
                    project["id"].as_str().unwrap()
                ),
                "POST",
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let job_id = started["job"]["id"].as_str().unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (status, job) = fixture
                    .json(
                        &format!("/api/game-assets/jobs/{job_id}"),
                        "GET",
                        Value::Null,
                    )
                    .await;
                assert_eq!(status, StatusCode::OK);
                if job["status"] != "running" {
                    break job;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed["status"], "completed");
        let (status,edited)=fixture.json(&format!("/api/game-assets/jobs/{job_id}/frames"),"POST",json!({"frames":[{"sourceIndex":1,"durationMs":300},{"sourceIndex":0,"opacity":0.5}]})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(edited.get("job").is_none());
        assert_eq!(edited["revision"], 1);
        assert_eq!(edited["originalOutput"], completed["output"]);
        assert_eq!(edited["output"]["frames"][0]["durationMs"], 300);
        let atlas_id = edited["output"]["atlas"]["media"]["id"].as_str().unwrap();
        let (status, _, bytes) = fixture
            .send(
                &format!("/api/game-assets/media/{atlas_id}/content"),
                "GET",
                "image/png",
                vec![],
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(image::load_from_memory(&bytes).is_ok());
        let (status, stop) = fixture
            .json(
                &format!("/api/game-assets/jobs/{job_id}/stop"),
                "POST",
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stop["job"], edited);
        fixture.close().await;
    }
}
