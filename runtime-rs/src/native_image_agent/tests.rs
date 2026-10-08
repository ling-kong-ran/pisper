use super::*;
use crate::{
    native_image_runtime::cpu::CpuImageAlgorithms,
    native_workflow::{
        bundle::EngineBundleStore,
        engine_cache::{EngineCache, EngineDefinition, EngineFile},
        image_nodes::{
            GeneratedImage, ImageGenerationRequest, ImageGenerator, ImageOperationRequest,
        },
        image_processing::{encode_png, ImageProcessor},
        media::{self, MediaService, StoredMedia},
    },
    workflow_engine::RunCancellation,
};
use futures::future::BoxFuture;
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

struct Directory {
    path: PathBuf,
}
impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pisper-image-agent-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self {
            path: fs::canonicalize(path).unwrap(),
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        if let Ok(path) = fs::canonicalize(&self.path) {
            assert!(path.starts_with(parent));
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pisper-image-agent-"));
            fs::remove_dir_all(path).unwrap();
        }
    }
}
fn png() -> Vec<u8> {
    let mut pixels = RgbaImage::from_pixel(16, 16, Rgba([255, 255, 255, 255]));
    for y in 4..12 {
        for x in 4..12 {
            pixels.put_pixel(x, y, Rgba([30, 60, 90, 255]));
        }
    }
    encode_png(&pixels).unwrap()
}
struct Generator {
    calls: Mutex<Vec<Value>>,
}
impl ImageGenerator for Generator {
    fn generate(
        &self,
        request: ImageGenerationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<GeneratedImage>> {
        Box::pin(async move {
            active(&cancellation, false)?;
            for path in &request.source_images {
                assert!(path.is_file());
            }
            let index = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(json!({"prompt":request.prompt,"model":request.model}));
                calls.len()
            };
            // Release saveVisualOutput and readGeneratedImage require the
            // real output below this job's generated/visuals directory.
            let directory = request.cwd.join("generated/visuals");
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join(format!("controlled-generator-{index}.png"));
            // This fixture really writes PNG pixels; it is not evidence for a
            // paid Provider driver or the unavailable complete visual API.
            fs::write(&path, png()).unwrap();
            Ok(GeneratedImage {
                path,
                mime_type: "image/png".into(),
            })
        })
    }
}
struct Fixture {
    directory: Directory,
    cwd: PathBuf,
    agent: PathBuf,
    service: Arc<ImageAgentService>,
    media: Arc<MediaService>,
    processor: Arc<ImageProcessor>,
    engines: Arc<EngineCache>,
    enabled: Arc<AtomicBool>,
    generator: Arc<Generator>,
    generated: Arc<Mutex<Vec<(GeneratedFile, ToolContext)>>>,
}
impl Fixture {
    async fn new() -> Self {
        let directory = Directory::new();
        let cwd = directory.path.join("workspace");
        let agent = directory.path.join("agent");
        fs::create_dir(&cwd).unwrap();
        fs::create_dir(&agent).unwrap();
        let bytes =
            b"synthetic verified engine presence; native Telea performs the operation".to_vec();
        let engines = EngineCache::open(
            &agent,
            vec![EngineDefinition {
                id: "inpaint".into(),
                name: "Synthetic engine presence".into(),
                version: "fixture-1".into(),
                files: vec![EngineFile {
                    name: "opencv.js".into(),
                    bytes: bytes.len(),
                    sha256: media::digest(&bytes),
                    mime_type: "text/javascript".into(),
                    urls: vec!["https://fixture.invalid/never-downloaded".into()],
                }],
            }],
        )
        .unwrap();
        engines
            .install_files(
                [("engines/inpaint/opencv.js".into(), bytes)]
                    .into_iter()
                    .collect(),
            )
            .await
            .unwrap();
        let processor = ImageProcessor::new(Some(CpuImageAlgorithms::new(engines.clone())));
        let enabled = Arc::new(AtomicBool::new(true));
        let gate = enabled.clone();
        let generator = Arc::new(Generator {
            calls: Mutex::new(vec![]),
        });
        let service = ImageAgentService::open(
            &agent,
            processor.clone(),
            generator.clone(),
            Arc::new(move || {
                let enabled = gate.load(Ordering::SeqCst);
                Box::pin(async move { Ok(enabled) })
            }),
        )
        .unwrap();
        let media = MediaService::open(&agent.join("image-tools-agent")).unwrap();
        Self {
            directory,
            cwd,
            agent,
            service,
            media,
            processor,
            engines,
            enabled,
            generator,
            generated: Arc::new(Mutex::new(vec![])),
        }
    }
    fn context(&self) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
            session_id: "actual-native-fixture-session".into(),
        }
    }
    fn sink(&self) -> GeneratedFilePort {
        let values = self.generated.clone();
        Arc::new(move |file, context| {
            values.lock().unwrap().push((file, context));
            Box::pin(async { Ok(()) })
        })
    }
    async fn call(&self, args: Value) -> Result<Value> {
        let result = self
            .service
            .call(
                self.context(),
                args.clone(),
                Arc::new(RunCancellation::default()),
                self.sink(),
            )
            .await;
        if let Err(error) = &result {
            // These are only generated UUID fixtures, never personal inputs.
            // Keep the original error and give each composite-operation failure
            // its real operation, schema paths and normalization stage.
            let validator = jsonschema::validator_for(&schema::schema().unwrap()).unwrap();
            let violations = validator
                .iter_errors(&args)
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            eprintln!("image_agent_operation={} args={} error={:?} schema_errors={:?} argument_parse={:?}",args["operation"],args,error,violations,schema::parse(&args).err());
        }
        result
    }
    async fn close(&self) {
        self.service.close().await;
        self.media.dispose().await;
        self.processor.dispose().await;
        self.engines.dispose().await;
    }
}
fn gate() -> AgentEnabledPort {
    Arc::new(|| Box::pin(async { Ok(true) }))
}
fn request() -> ImageOperationRequest {
    ImageOperationRequest {
        operation: "preview".into(),
        source: None,
        images: vec![],
        settings: json!({}),
        prompt: String::new(),
        model: Value::Null,
        resume_output: None,
        edits: None,
    }
}
fn operation_error() -> WorkflowError {
    error(
        "workflow_image_source_required",
        axum::http::StatusCode::BAD_REQUEST,
    )
}
struct NoOperations;
impl AgentOperations for NoOperations {
    fn execute(
        &self,
        _: ImageOperationRequest,
        _: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async { Err(operation_error()) })
    }
}

#[test]
fn exact_tool_schema_rejects_extra_fields_and_validates_before_import() {
    assert_eq!(
        schema::schema().unwrap()["properties"]["operation"]["enum"],
        json!([
            "input",
            "background",
            "inpaint",
            "generate",
            "frames",
            "transform",
            "preview",
            "export",
            "edit"
        ])
    );
    for invalid in [
        json!({"operation":"input","sourceImage":"safe.png","consumer":"workbench"}),
        json!({"operation":"input","sourceImage":"safe.png","model":"missing-separator"}),
        json!({"operation":"input","sourceImage":"safe.png","edits":{"frames":[{"sourceIndex":0}]}}),
        json!({"operation":"background","settings":{"method":"remote"}}),
        json!({"operation":"transform","settings":{"transforms":[{"index":0,"opacity":2}]}}),
        json!({"operation":"edit","edits":{"frames":[{"sourceIndex":0,"extra":true}]}}),
        json!({"operation":"input","sourceImage":"safe.png","source":{"id":"image","name":"image.png","mimeType":"image/png","size":1}}),
        json!({"operation":"preview","images":[]}),
        json!({"operation":"input","source":{"id":"image","name":"image.png","mimeType":"video/mp4","size":1}}),
    ] {
        assert!(schema::parse(&invalid).is_err(), "{invalid}");
    }
    assert_eq!(
        schema::parse(&json!({"operation":"generate","model":"provider/folder/model"}))
            .unwrap()
            .model,
        json!({"provider":"provider","model":"folder/model"})
    );
}

#[tokio::test]
async fn all_nine_operations_use_native_media_pixels_and_separate_agent_store() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("character.png"), png()).unwrap();
    let input = fixture
        .call(json!({"operation":"input","sourceImage":"character.png"}))
        .await
        .unwrap();
    let frames = input["details"]["output"]["frames"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(input["content"][0]["text"].as_str().unwrap()).unwrap(),
        input["details"]
    );
    let media_id = frames[0]["media"]["id"].as_str().unwrap();
    assert!(fixture
        .agent
        .join("image-tools-agent/workflow-media")
        .join(media_id)
        .join("data.bin")
        .is_file());
    let workflow = MediaService::open(&fixture.agent).unwrap();
    let game = MediaService::open(&fixture.agent.join("game-assets")).unwrap();
    assert!(workflow.read(media_id).await.is_err());
    assert!(game.read(media_id).await.is_err());
    let background=fixture.call(json!({"operation":"background","images":frames,"settings":{"method":"color","colors":["#ffffff"],"tolerance":0,"softness":0}})).await.unwrap();
    let stored = fixture
        .media
        .read(
            background["details"]["output"]["frames"][0]["media"]["id"]
                .as_str()
                .unwrap(),
        )
        .await
        .unwrap();
    let pixels = image::load_from_memory(&stored.buffer)
        .unwrap()
        .into_rgba8();
    assert_eq!(pixels.get_pixel(0, 0)[3], 0);
    assert_eq!(pixels.get_pixel(8, 8)[3], 255);
    let repaired=fixture.call(json!({"operation":"inpaint","images":frames,"settings":{"region":{"x":40,"y":40,"width":10,"height":10}}})).await.unwrap();
    assert_eq!(
        repaired["details"]["output"]["frames"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let generated=fixture.call(json!({"operation":"generate","images":frames,"prompt":"controlled fixture","model":"fixture/image","settings":{"directions":["S","W"],"frameCount":1,"colors":["#ffffff"]}})).await.unwrap();
    assert_eq!(
        generated["details"]["output"]["frames"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fixture.generator.calls.lock().unwrap().len(), 2);
    let split=fixture.call(json!({"operation":"frames","images":frames,"settings":{"columns":2,"rows":1,"frameCount":2,"trim":false,"padding":0}})).await.unwrap();
    assert_eq!(
        split["details"]["output"]["frames"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let transform=fixture.call(json!({"operation":"transform","images":frames,"settings":{"transforms":[{"index":0,"opacity":0.5}]}})).await.unwrap();
    let transformed = fixture
        .media
        .read(
            transform["details"]["output"]["frames"][0]["media"]["id"]
                .as_str()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        image::load_from_memory(&transformed.buffer)
            .unwrap()
            .into_rgba8()
            .get_pixel(8, 8)[3],
        128
    );
    let preview = fixture
        .call(json!({"operation":"preview","images":frames}))
        .await
        .unwrap();
    assert_eq!(preview["details"]["output"]["frames"], frames);
    let edited=fixture.call(json!({"operation":"edit","images":frames,"edits":{"frames":[{"sourceIndex":0,"opacity":0.25,"durationMs":240},{"sourceIndex":0,"durationMs":300}]}})).await.unwrap();
    assert_eq!(
        edited["details"]["output"]["frames"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        edited["details"]["output"]["frames"][0]["durationMs"].as_f64(),
        Some(240.0)
    );
    let export = fixture
        .call(json!({"operation":"export","images":edited["details"]["output"]["frames"]}))
        .await
        .unwrap();
    let files = export["details"]["files"]["files"].as_array();
    assert!(files.is_none(), "files must be the release flat array");
    let files = export["details"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let atlas_bytes = fs::read(files[0]["path"].as_str().unwrap()).unwrap();
    let stored = fixture
        .media
        .read(
            export["details"]["output"]["atlas"]["media"]["id"]
                .as_str()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(atlas_bytes, stored.buffer);
    let metadata_text = fs::read_to_string(files[1]["path"].as_str().unwrap()).unwrap();
    let metadata: Value = serde_json::from_str(&metadata_text).unwrap();
    assert_eq!(metadata["image"], "atlas.png");
    assert_eq!(
        metadata["frames"],
        export["details"]["output"]["atlas"]["frames"]
    );
    assert!(!metadata_text.contains("\"media\"") && !metadata_text.contains(media_id));
    let notifications = fixture.generated.lock().unwrap();
    assert_eq!(notifications.len(), 2);
    assert!(notifications
        .iter()
        .all(|(file, context)| file.path.is_file()
            && context.session_id == "actual-native-fixture-session"
            && context.cwd == fixture.cwd));
    drop(notifications);
    fixture.service.close().await;
    let reopened = MediaService::open(&fixture.agent.join("image-tools-agent")).unwrap();
    assert_eq!(reopened.read(media_id).await.unwrap().buffer, png());
    reopened.dispose().await;
    workflow.dispose().await;
    game.dispose().await;
    fixture.close().await;
}

#[tokio::test]
async fn import_limits_paths_and_pure_tool_errors_precede_media_commit() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    fs::write(fixture.directory.path.join("private.png"), png()).unwrap();
    for (source, code) in [
        ("../private.png", "image_tools_source_outside_workspace"),
        (".", "image_tools_source_outside_workspace"),
        ("missing.png", "image_tools_source_unavailable"),
        (
            "https://fixture.invalid/a.png",
            "image_tools_source_invalid",
        ),
        ("file:///private.png", "image_tools_source_invalid"),
        ("data:image/png;base64,x", "image_tools_source_invalid"),
        ("\0hidden.png", "image_tools_source_invalid"),
    ] {
        let error = fixture
            .service
            .import_image(
                fixture.cwd.clone(),
                source.into(),
                Arc::new(RunCancellation::default()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.message, code);
    }
    fs::write(fixture.cwd.join("fake.png"), b"not pixels").unwrap();
    let large = fs::File::create(fixture.cwd.join("large.png")).unwrap();
    large.set_len(MAX_IMAGE_BYTES as u64 + 1).unwrap();
    let mut dimensions = png();
    dimensions[16..20].copy_from_slice(&8000_u32.to_be_bytes());
    fs::write(fixture.cwd.join("dimensions.png"), dimensions).unwrap();
    let mut pixel_count = png();
    pixel_count[16..20].copy_from_slice(&4001_u32.to_be_bytes());
    pixel_count[20..24].copy_from_slice(&4000_u32.to_be_bytes());
    fs::write(fixture.cwd.join("pixel-count.png"), pixel_count).unwrap();
    for (source, code) in [
        ("fake.png", "image_tools_source_invalid"),
        ("large.png", "image_tools_source_too_large"),
        ("dimensions.png", "image_tools_source_too_large"),
        ("pixel-count.png", "image_tools_source_too_large"),
    ] {
        assert_eq!(
            fixture
                .service
                .import_image(
                    fixture.cwd.clone(),
                    source.into(),
                    Arc::new(RunCancellation::default())
                )
                .await
                .unwrap_err()
                .code,
            code
        );
    }
    assert!(fixture
        .call(json!({"operation":"input","sourceImage":"safe.png","model":"bad"}))
        .await
        .is_err());
    assert_eq!(
        fs::read_dir(fixture.agent.join("image-tools-agent/workflow-media"))
            .unwrap()
            .count(),
        0
    );
    fixture.close().await;
}

#[tokio::test]
async fn jpeg_and_webp_imports_use_detected_mime_and_survive_native_decode() {
    let fixture = Fixture::new().await;
    for (format, name, mime) in [
        (
            image::ImageFormat::Jpeg,
            "mislabelled-jpeg.bin",
            "image/jpeg",
        ),
        (
            image::ImageFormat::WebP,
            "mislabelled-webp.bin",
            "image/webp",
        ),
    ] {
        let pixels = RgbaImage::from_pixel(4, 3, Rgba([10, 40, 100, 255]));
        let image = image::DynamicImage::ImageRgba8(pixels);
        let mut bytes = std::io::Cursor::new(Vec::new());
        if format == image::ImageFormat::Jpeg {
            image.to_rgb8().write_to(&mut bytes, format).unwrap();
        } else {
            image.write_to(&mut bytes, format).unwrap();
        }
        fs::write(fixture.cwd.join(name), bytes.get_ref()).unwrap();
        let imported = fixture
            .service
            .import_image(
                fixture.cwd.clone(),
                name.into(),
                Arc::new(RunCancellation::default()),
            )
            .await
            .unwrap();
        assert_eq!(imported["mimeType"], mime);
        assert_eq!(
            fixture
                .media
                .read(imported["id"].as_str().unwrap())
                .await
                .unwrap()
                .buffer,
            *bytes.get_ref()
        );
        let result = fixture
            .call(json!({"operation":"input","source":imported}))
            .await
            .unwrap();
        assert_eq!(
            result["details"]["output"]["frames"][0]["width"].as_f64(),
            Some(4.0)
        );
        assert_eq!(
            result["details"]["output"]["frames"][0]["height"].as_f64(),
            Some(3.0)
        );
        let decoded = fixture
            .call(json!({"operation":"transform","images":result["details"]["output"]["frames"]}))
            .await
            .unwrap();
        let stored = fixture
            .media
            .read(
                decoded["details"]["output"]["frames"][0]["media"]["id"]
                    .as_str()
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stored.metadata["media"]["mimeType"], "image/png");
        let actual = image::load_from_memory(&stored.buffer)
            .unwrap()
            .into_rgba8();
        // Oracle settings default padding=4 and transformFrames adds it on
        // both sides of an opaque source: 4x3 -> 12x11 with image at (4,4).
        assert_eq!(actual.dimensions(), (12, 11));
        for (x, y, pixel) in actual.enumerate_pixels() {
            assert_eq!(
                pixel[3],
                if (4..8).contains(&x) && (4..7).contains(&y) {
                    255
                } else {
                    0
                },
                "{mime} ({x},{y})"
            );
        }
    }
    fixture.close().await;
}

#[test]
fn replace_same_sized_file_or_parent_is_detected_by_native_identity() {
    let directory = Directory::new();
    let nested = directory.path.join("nested");
    fs::create_dir(&nested).unwrap();
    let file = nested.join("image.png");
    fs::write(&file, png()).unwrap();
    let root = fs_boundary::root(&directory.path).unwrap();
    let entries = fs_boundary::inspect_file(&root, Path::new("nested/image.png")).unwrap();
    fs::rename(&file, nested.join("old.png")).unwrap();
    fs::write(&file, png()).unwrap();
    assert!(matches!(
        fs_boundary::read_file(
            &root,
            &entries,
            MAX_IMAGE_BYTES,
            &RunCancellation::default()
        ),
        Err(fs_boundary::Failure::Changed)
    ));
    let entries = fs_boundary::inspect_file(&root, Path::new("nested/image.png")).unwrap();
    fs::rename(&nested, directory.path.join("old-nested")).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::write(&file, png()).unwrap();
    assert!(matches!(
        fs_boundary::read_file(
            &root,
            &entries,
            MAX_IMAGE_BYTES,
            &RunCancellation::default()
        ),
        Err(fs_boundary::Failure::Changed)
    ));
}

#[test]
fn cleanup_never_removes_a_replacement_uuid_directory() {
    let directory = Directory::new();
    let root = fs_boundary::root(&directory.path).unwrap();
    let mut entries = vec![root.clone()];
    fs_boundary::child(&root.path, &mut entries, "generated").unwrap();
    fs_boundary::child(&root.path, &mut entries, "image-assets").unwrap();
    fs_boundary::new_directory(&root.path, &mut entries, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let own = entries.last().unwrap().path.clone();
    let token = RunCancellation::default();
    fs_boundary::write_file(
        &root.path,
        &entries,
        "atlas.png",
        b"own original bytes",
        &token,
    )
    .unwrap();
    let moved = own.with_file_name("preserved-original");
    fs::rename(&own, &moved).unwrap();
    fs::create_dir(&own).unwrap();
    fs::write(own.join("foreign.txt"), "must retain").unwrap();
    fs_boundary::cleanup(&root.path, &entries).unwrap();
    assert_eq!(
        fs::read_to_string(own.join("foreign.txt")).unwrap(),
        "must retain"
    );
    assert_eq!(
        fs::read(moved.join("atlas.png")).unwrap(),
        b"own original bytes"
    );
}

struct ErrorMedia;
impl AgentMedia for ErrorMedia {
    fn upload(&self, _: String, _: String, _: Vec<u8>) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async { Err(WorkflowError::io("secret /private/native/store")) })
    }
    fn read(&self, _: String) -> BoxFuture<'_, Result<StoredMedia>> {
        Box::pin(async { Err(WorkflowError::io("secret /private/native/store")) })
    }
}
#[tokio::test]
async fn storage_failures_never_expose_paths_or_upstream_details() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let output = atlas(&fixture).await;
    let service =
        ImageAgentService::with_services(Arc::new(ErrorMedia), Arc::new(NoOperations), gate());
    let failure = service
        .import_image(
            fixture.cwd.clone(),
            "safe.png".into(),
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, "image_tools_import_failed");
    assert_eq!(failure.message, failure.code);
    let failure = service
        .export_images(
            fixture.cwd.clone(),
            output,
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, "image_tools_export_failed");
    assert_eq!(failure.message, failure.code);
    service.close().await;
    fixture.close().await;
}

#[tokio::test]
async fn fresh_gate_cancellation_and_mid_read_disable_do_not_create_resources() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    fixture.enabled.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .call(json!({"operation":"input","sourceImage":"safe.png"}))
            .await
            .unwrap_err()
            .code,
        "image_tools_agent_disabled"
    );
    fixture.enabled.store(true, Ordering::SeqCst);
    let checks = Arc::new(AtomicUsize::new(0));
    let counter = checks.clone();
    let service = ImageAgentService::with_services(
        fixture.media.clone(),
        Arc::new(NoOperations),
        Arc::new(move || {
            let allowed = counter.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move { Ok(allowed) })
        }),
    );
    assert_eq!(
        service
            .import_image(
                fixture.cwd.clone(),
                "safe.png".into(),
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_agent_disabled"
    );
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    let cancelled = Arc::new(RunCancellation::default());
    cancelled.cancel();
    assert_eq!(
        fixture
            .service
            .import_image(fixture.cwd.clone(), "safe.png".into(), cancelled)
            .await
            .unwrap_err()
            .code,
        "workflow_image_cancelled"
    );
    assert_eq!(
        fs::read_dir(fixture.agent.join("image-tools-agent/workflow-media"))
            .unwrap()
            .count(),
        0
    );
    service.close().await;
    fixture.close().await;
}

async fn atlas(fixture: &Fixture) -> Value {
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let input = fixture
        .call(json!({"operation":"input","sourceImage":"safe.png"}))
        .await
        .unwrap();
    fixture
        .call(json!({"operation":"export","images":input["details"]["output"]["frames"]}))
        .await
        .unwrap()["details"]["output"]
        .clone()
}
#[tokio::test]
async fn export_verifies_atlas_bytes_and_revocation_cleans_only_new_directory() {
    let fixture = Fixture::new().await;
    let output = atlas(&fixture).await;
    let base = fixture.cwd.join("generated/image-assets");
    let previous = fs::read_dir(&base).unwrap().next().unwrap().unwrap().path();
    let original = fs::read(previous.join("atlas.png")).unwrap();
    for forged in [
        {
            let mut value = output.clone();
            value["atlas"]["media"]["name"] = json!("forged.png");
            value
        },
        {
            let mut value = output.clone();
            value["atlas"]["width"] = json!(1);
            value
        },
        {
            let mut value = output.clone();
            value["atlas"]["frames"][0]["x"] = json!(4096);
            value
        },
        {
            let mut value = output.clone();
            value["atlas"]["media"]["mimeType"] = json!("image/jpeg");
            value
        },
    ] {
        assert_eq!(
            fixture
                .service
                .export_images(
                    fixture.cwd.clone(),
                    forged,
                    Arc::new(RunCancellation::default())
                )
                .await
                .unwrap_err()
                .code,
            "image_tools_export_invalid"
        );
    }
    let checks = Arc::new(AtomicUsize::new(0));
    let counter = checks.clone();
    let service = ImageAgentService::with_services(
        fixture.media.clone(),
        Arc::new(NoOperations),
        Arc::new(move || {
            let allowed = counter.fetch_add(1, Ordering::SeqCst) < 2;
            Box::pin(async move { Ok(allowed) })
        }),
    );
    assert_eq!(
        service
            .export_images(
                fixture.cwd.clone(),
                output.clone(),
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_agent_disabled"
    );
    assert_eq!(checks.load(Ordering::SeqCst), 3);
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    assert_eq!(fs::read(previous.join("atlas.png")).unwrap(), original);
    let cancellation = Arc::new(RunCancellation::default());
    let token = cancellation.clone();
    let checks = Arc::new(AtomicUsize::new(0));
    let counter = checks.clone();
    let cancelled = ImageAgentService::with_services(
        fixture.media.clone(),
        Arc::new(NoOperations),
        Arc::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) == 2 {
                token.cancel();
            }
            Box::pin(async { Ok(true) })
        }),
    );
    assert_eq!(
        cancelled
            .export_images(fixture.cwd.clone(), output, cancellation)
            .await
            .unwrap_err()
            .code,
        "image_tools_export_cancelled"
    );
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    cancelled.close().await;
    service.close().await;
    fixture.close().await;
}

fn directory_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Junction setup is confined to this UUID fixture. No system/profile
        // path is moved or removed and the helper window remains hidden.
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(fs_boundary::display_path(link))
            .arg(fs_boundary::display_path(target))
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(output.status.success(), "synthetic junction setup failed");
    }
}
#[tokio::test]
async fn linked_child_directories_cannot_escape_but_host_workspace_alias_is_allowed() {
    let fixture = Fixture::new().await;
    let outside = fixture.directory.path.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("safe.png"), png()).unwrap();
    directory_link(&outside, &fixture.cwd.join("redirect"));
    assert_eq!(
        fixture
            .service
            .import_image(
                fixture.cwd.clone(),
                "redirect/safe.png".into(),
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_source_invalid"
    );
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let alias = fixture.directory.path.join("workspace-alias");
    directory_link(&fixture.cwd, &alias);
    let imported = fixture
        .service
        .import_image(
            alias,
            "safe.png".into(),
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .media
            .read(imported["id"].as_str().unwrap())
            .await
            .unwrap()
            .buffer,
        png()
    );
    let output = atlas(&fixture).await;
    // A link at either generated directory level must preserve every outside file.
    let export_workspace = fixture.directory.path.join("export-workspace");
    fs::create_dir(&export_workspace).unwrap();
    directory_link(&outside, &export_workspace.join("generated"));
    assert_eq!(
        fixture
            .service
            .export_images(
                export_workspace,
                output.clone(),
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_export_invalid"
    );
    let export_workspace = fixture.directory.path.join("export-workspace-two");
    fs::create_dir(&export_workspace).unwrap();
    fs::create_dir(export_workspace.join("generated")).unwrap();
    directory_link(&outside, &export_workspace.join("generated/image-assets"));
    assert_eq!(
        fixture
            .service
            .export_images(
                export_workspace,
                output,
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_export_invalid"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(fs::read(outside.join("safe.png")).unwrap(), png());
    fixture.close().await;
}

struct HoldingMedia {
    real: Arc<MediaService>,
    hold_upload: AtomicBool,
    hold_read: AtomicBool,
    entered: Notify,
    finish: Semaphore,
    committed: Mutex<Option<Value>>,
}
impl HoldingMedia {
    fn new(real: Arc<MediaService>, upload: bool, read: bool) -> Arc<Self> {
        Arc::new(Self {
            real,
            hold_upload: AtomicBool::new(upload),
            hold_read: AtomicBool::new(read),
            entered: Notify::new(),
            finish: Semaphore::new(0),
            committed: Mutex::new(None),
        })
    }
}
impl AgentMedia for HoldingMedia {
    fn upload(&self, name: String, mime: String, bytes: Vec<u8>) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            if self.hold_upload.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.finish.acquire().await.unwrap().forget();
            }
            let result = self.real.upload(&name, &mime, &bytes).await?;
            *self.committed.lock().unwrap() = Some(result.clone());
            Ok(result)
        })
    }
    fn read(&self, id: String) -> BoxFuture<'_, Result<StoredMedia>> {
        Box::pin(async move {
            if self.hold_read.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.finish.acquire().await.unwrap().forget();
            }
            self.real.read(&id).await
        })
    }
}
#[tokio::test]
async fn close_waits_for_real_import_commit_and_does_not_close_borrowed_media() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let media = HoldingMedia::new(fixture.media.clone(), true, false);
    let service = ImageAgentService::with_services(media.clone(), Arc::new(NoOperations), gate());
    let (owned, cwd) = (service.clone(), fixture.cwd.clone());
    let task = tokio::spawn(async move {
        owned
            .import_image(cwd, "safe.png".into(), Arc::new(RunCancellation::default()))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), media.entered.notified())
        .await
        .unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    let owned = service.clone();
    let close = tokio::spawn(async move {
        owned.close().await;
        flag.store(true, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert!(!closed.load(Ordering::SeqCst));
    assert_eq!(
        service
            .import_image(
                fixture.cwd.clone(),
                "safe.png".into(),
                Arc::new(RunCancellation::default())
            )
            .await
            .unwrap_err()
            .code,
        "image_tools_closed"
    );
    media.finish.add_permits(1);
    let result = task.await.unwrap().unwrap();
    close.await.unwrap();
    assert!(closed.load(Ordering::SeqCst));
    assert_eq!(
        fixture
            .media
            .read(result["id"].as_str().unwrap())
            .await
            .unwrap()
            .buffer,
        png()
    );
    service.close().await;
    fixture.close().await;
}
#[tokio::test]
async fn dropping_import_caller_still_drains_the_already_started_native_commit() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let media = HoldingMedia::new(fixture.media.clone(), true, false);
    let service = ImageAgentService::with_services(media.clone(), Arc::new(NoOperations), gate());
    let (owned, cwd) = (service.clone(), fixture.cwd.clone());
    let task = tokio::spawn(async move {
        owned
            .import_image(cwd, "safe.png".into(), Arc::new(RunCancellation::default()))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), media.entered.notified())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    let owned = service.clone();
    let closing = tokio::spawn(async move { owned.close().await });
    tokio::task::yield_now().await;
    assert!(!closing.is_finished());
    media.finish.add_permits(1);
    closing.await.unwrap();
    let reference = media.committed.lock().unwrap().clone().unwrap();
    assert_eq!(
        fixture
            .media
            .read(reference["id"].as_str().unwrap())
            .await
            .unwrap()
            .buffer,
        png()
    );
    fixture.close().await;
}
#[tokio::test]
async fn close_waits_for_export_read_and_cancels_before_workspace_writes() {
    let fixture = Fixture::new().await;
    let output = atlas(&fixture).await;
    let media = HoldingMedia::new(fixture.media.clone(), false, true);
    let service = ImageAgentService::with_services(media.clone(), Arc::new(NoOperations), gate());
    let cwd = fixture.directory.path.join("empty-workspace");
    fs::create_dir(&cwd).unwrap();
    let (owned, path) = (service.clone(), cwd.clone());
    let task = tokio::spawn(async move {
        owned
            .export_images(path, output, Arc::new(RunCancellation::default()))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), media.entered.notified())
        .await
        .unwrap();
    let owned = service.clone();
    let close = tokio::spawn(async move { owned.close().await });
    tokio::task::yield_now().await;
    assert!(!close.is_finished());
    media.finish.add_permits(1);
    assert_eq!(
        task.await.unwrap().unwrap_err().code,
        "image_tools_export_cancelled"
    );
    close.await.unwrap();
    assert_eq!(fs::read_dir(cwd).unwrap().count(), 0);
    fixture.close().await;
}
struct HoldingOperations {
    entered: Notify,
    cancelled: Notify,
    finish: Semaphore,
    completed: AtomicBool,
}
impl AgentOperations for HoldingOperations {
    fn execute(
        &self,
        _: ImageOperationRequest,
        cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            self.entered.notify_one();
            cancellation.cancelled().await;
            self.cancelled.notify_one();
            self.finish.acquire().await.unwrap().forget();
            self.completed.store(true, Ordering::SeqCst);
            Err(error(
                "workflow_image_cancelled",
                axum::http::StatusCode::CONFLICT,
            ))
        })
    }
}
#[tokio::test]
async fn close_waits_for_actual_operation_cleanup_and_rejects_new_work() {
    let fixture = Fixture::new().await;
    let operations = Arc::new(HoldingOperations {
        entered: Notify::new(),
        cancelled: Notify::new(),
        finish: Semaphore::new(0),
        completed: AtomicBool::new(false),
    });
    let service =
        ImageAgentService::with_services(fixture.media.clone(), operations.clone(), gate());
    let owned = service.clone();
    let task = tokio::spawn(async move {
        owned
            .execute(request(), Arc::new(RunCancellation::default()))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), operations.entered.notified())
        .await
        .unwrap();
    let owned = service.clone();
    let close = tokio::spawn(async move { owned.close().await });
    tokio::time::timeout(Duration::from_secs(5), operations.cancelled.notified())
        .await
        .unwrap();
    assert!(!close.is_finished());
    assert!(!operations.completed.load(Ordering::SeqCst));
    assert_eq!(
        service
            .execute(request(), Arc::new(RunCancellation::default()))
            .await
            .unwrap_err()
            .code,
        "image_tools_closed"
    );
    operations.finish.add_permits(1);
    assert_eq!(
        task.await.unwrap().unwrap_err().code,
        "workflow_image_cancelled"
    );
    close.await.unwrap();
    assert!(operations.completed.load(Ordering::SeqCst));
    fixture.close().await;
}
#[tokio::test]
async fn caller_cancel_survives_async_gate_and_a_later_call_is_independent() {
    let fixture = Fixture::new().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let first = Arc::new(AtomicBool::new(true));
    let (notify, semaphore, flag) = (entered.clone(), release.clone(), first.clone());
    let service = ImageAgentService::with_services(
        fixture.media.clone(),
        Arc::new(NoOperations),
        Arc::new(move || {
            let (notify, semaphore) = (notify.clone(), semaphore.clone());
            let hold = flag.swap(false, Ordering::SeqCst);
            Box::pin(async move {
                if hold {
                    notify.notify_one();
                    semaphore.acquire().await.unwrap().forget();
                }
                Ok(true)
            })
        }),
    );
    let cancellation = Arc::new(RunCancellation::default());
    let (owned, token) = (service.clone(), cancellation.clone());
    let task = tokio::spawn(async move { owned.execute(request(), token).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    cancellation.cancel();
    release.add_permits(1);
    assert_eq!(
        task.await.unwrap().unwrap_err().code,
        "workflow_image_cancelled"
    );
    assert_eq!(
        service
            .execute(request(), Arc::new(RunCancellation::default()))
            .await
            .unwrap_err()
            .code,
        "workflow_image_source_required"
    );
    service.close().await;
    fixture.close().await;
}
#[tokio::test]
async fn generated_index_failures_keep_real_export_paths_and_close_joins_sink() {
    let fixture = Fixture::new().await;
    fs::write(fixture.cwd.join("safe.png"), png()).unwrap();
    let input = fixture
        .call(json!({"operation":"input","sourceImage":"safe.png"}))
        .await
        .unwrap();
    let entered = Arc::new(Notify::new());
    let finish = Arc::new(Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let (notify, release, count) = (entered.clone(), finish.clone(), calls.clone());
    let sink: GeneratedFilePort = Arc::new(move |file, context| {
        assert!(file.path.is_file());
        assert_eq!(context.session_id, "actual-native-fixture-session");
        let (notify, release) = (notify.clone(), release.clone());
        let first = count.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                notify.notify_one();
                release.acquire().await.unwrap().forget();
            }
            Err(error(
                "fixture_index_failure",
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            ))
        })
    });
    let (service, context, args) = (
        fixture.service.clone(),
        fixture.context(),
        json!({"operation":"export","images":input["details"]["output"]["frames"]}),
    );
    let task = tokio::spawn(async move {
        service
            .call(context, args, Arc::new(RunCancellation::default()), sink)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let service = fixture.service.clone();
    let close = tokio::spawn(async move { service.close().await });
    tokio::task::yield_now().await;
    assert!(!close.is_finished());
    finish.add_permits(1);
    let result = task.await.unwrap().unwrap();
    close.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(result["details"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| Path::new(file["path"].as_str().unwrap()).is_file()));
    fixture.close().await;
}
