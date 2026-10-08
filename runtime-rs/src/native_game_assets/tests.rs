use super::{protocol, GameAssetsService};
use crate::{
    native_workflow::{
        image_nodes::{
            GeneratedImage, ImageGenerationRequest, ImageGenerator, ImageNodeService,
            ImageOperationRequest,
        },
        image_processing::{encode_png, ImageAlgorithms, ImageProcessor, RasterFrame},
        media::MediaService,
        test_support::TempDirectory,
        Result, WorkflowError,
    },
    workflow_engine::RunCancellation,
};
use futures::future::BoxFuture;
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

struct Generator {
    calls: Mutex<Vec<String>>,
    hold: AtomicBool,
    fail_call: AtomicUsize,
    entered: Notify,
}
impl ImageGenerator for Generator {
    fn generate(
        &self,
        request: ImageGenerationRequest,
        _cancellation: Arc<RunCancellation>,
    ) -> BoxFuture<'_, Result<GeneratedImage>> {
        Box::pin(async move {
            assert!(request.source_images.iter().all(|path| path.is_file()));
            let count = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(request.prompt);
                calls.len()
            };
            self.entered.notify_one();
            if self.hold.load(Ordering::Acquire) {
                std::future::pending::<()>().await;
            }
            if self.fail_call.load(Ordering::Acquire) == count {
                return Err(WorkflowError::invalid(
                    "private fixture upstream token must not leak",
                ));
            }
            let directory = request.cwd.join("generated").join("visuals");
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join(format!("{}.png", request.output_name));
            let pixels = RgbaImage::from_fn(8, 4, |x, y| {
                if (1..=2).contains(&(x % 4)) && (1..=2).contains(&y) {
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
struct Algorithms {
    hold: AtomicBool,
    entered: Notify,
}
impl ImageAlgorithms for Algorithms {
    fn process(
        &self,
        _operation: &str,
        frames: Vec<RasterFrame>,
        _settings: &Value,
        cancellation: &RunCancellation,
    ) -> Result<Vec<RasterFrame>> {
        self.entered.notify_one();
        while self.hold.load(Ordering::Acquire) {
            if cancellation.is_cancelled() {
                return Err(WorkflowError::coded(
                    "workflow_image_cancelled",
                    "workflow_image_cancelled",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(frames)
    }
}
struct Fixture {
    directory: TempDirectory,
    media: Arc<MediaService>,
    images: Arc<ImageNodeService>,
    processor: Arc<ImageProcessor>,
    generator: Arc<Generator>,
    algorithms: Arc<Algorithms>,
    service: Arc<GameAssetsService>,
    reference: Value,
}
impl Fixture {
    async fn new() -> Self {
        let directory = TempDirectory::new();
        let media = MediaService::open(&directory.path).unwrap();
        let reference = media
            .upload(
                "reference.png",
                "image/png",
                &encode_png(&RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]))).unwrap(),
            )
            .await
            .unwrap();
        let generator = Arc::new(Generator {
            calls: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            fail_call: AtomicUsize::new(0),
            entered: Notify::new(),
        });
        let algorithms = Arc::new(Algorithms {
            hold: AtomicBool::new(false),
            entered: Notify::new(),
        });
        let processor = ImageProcessor::new(Some(algorithms.clone()));
        let images = ImageNodeService::open(
            &directory.path,
            media.clone(),
            processor.clone(),
            generator.clone(),
        )
        .unwrap();
        let service =
            GameAssetsService::open(&directory.path, media.clone(), images.clone()).unwrap();
        Self {
            directory,
            media,
            images,
            processor,
            generator,
            algorithms,
            service,
            reference,
        }
    }
    fn input(&self, name: &str) -> Value {
        json!({"name":name,"prompt":"Keep style","reference":self.reference,
        "originalReference":self.reference,"frameCount":2,"directions":["S","N"],"model":null,
        "actions":[{"id":"idle","name":"Idle","prompt":"Breathe","enabled":true},
            {"id":"walk","name":"Walk","prompt":"Forward","enabled":true}]})
    }
    async fn completed(&self, job: &Value) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let job = self
                    .service
                    .get_job(job["id"].as_str().unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                if job["status"] != "running" {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(&self) {
        self.service.dispose().await.unwrap();
        self.images.dispose().await;
        self.processor.dispose().await;
        self.media.dispose().await;
    }
}
#[test]
fn game_protocol_validates_release_defaults_unknown_fields_and_job_invariants() {
    let input = protocol::project_input(&json!({"name":" Draft "})).unwrap();
    assert_eq!(input["name"], "Draft");
    assert_eq!(input["frameCount"], 4);
    assert_eq!(input["directions"].as_array().unwrap().len(), 8);
    assert!(input["reference"].is_null());
    assert!(input["model"].is_null());
    for patch in [
        json!({"frameCount":1.5}),
        json!({"frameCount":17}),
        json!({"directions":["S","S"]}),
        json!({"directions":[]}),
        json!({"model":{"provider":"","model":"m"}}),
        json!({"actions":[{"id":"../x","name":"x","enabled":true}]}),
        json!({"workflowId":"coupled"}),
    ] {
        let mut value = json!({"name":"Draft"});
        for (key, value_patch) in patch.as_object().unwrap() {
            value[key] = value_patch.clone();
        }
        assert_eq!(
            protocol::project_input(&value).unwrap_err().code,
            "game_assets_invalid"
        );
    }
    let id = crate::native_workflow::id().unwrap();
    let mut project = input;
    project["id"] = json!(id);
    project["createdAt"] = json!(crate::native_workflow::now());
    project["updatedAt"] = project["createdAt"].clone();
    let job = json!({"id":crate::native_workflow::id().unwrap(),"projectId":id,"status":"running","startedAt":crate::native_workflow::now(),
        "finishedAt":null,"completed":0,"total":1,"error":null,"output":protocol::empty_output(),"originalOutput":protocol::empty_output(),"revision":0});
    assert!(protocol::catalog(&json!({"projects":[project.clone()],"jobs":[job.clone()]})).is_ok());
    assert!(protocol::catalog(&json!({"projects":[project.clone(),project],"jobs":[]})).is_err());
    for (key, value) in [
        ("status", json!("completed")),
        ("finishedAt", json!(crate::native_workflow::now())),
        ("error", json!("secret token")),
        ("revision", json!(9_007_199_254_740_992_u64)),
    ] {
        let mut invalid = job.clone();
        invalid[key] = value;
        assert!(protocol::job(&invalid).is_err());
    }
}
#[tokio::test]
async fn game_project_generation_saves_real_png_atlas_and_reopens_without_workflows() {
    let fixture = Fixture::new().await;
    let project = fixture
        .service
        .save(fixture.input("Character"))
        .await
        .unwrap();
    let done = fixture
        .completed(
            &fixture
                .service
                .run(project["id"].as_str().unwrap())
                .await
                .unwrap(),
        )
        .await;
    assert_eq!(done["status"], "completed");
    assert_eq!(done["completed"], 2);
    assert_eq!(done["output"]["frames"].as_array().unwrap().len(), 8);
    assert_eq!(done["originalOutput"], done["output"]);
    assert_eq!(fixture.generator.calls.lock().unwrap().len(), 4);
    let atlas = fixture
        .media
        .read(done["output"]["atlas"]["media"]["id"].as_str().unwrap())
        .await
        .unwrap();
    let pixels = image::load_from_memory(&atlas.buffer).unwrap().into_rgba8();
    assert!(pixels.pixels().any(|pixel| pixel.0 == [220, 30, 50, 255]));
    assert!(pixels.pixels().any(|pixel| pixel[3] == 0));
    let serialized =
        std::fs::read_to_string(fixture.directory.path.join("game-assets.json")).unwrap();
    for forbidden in [
        "workflowId",
        "nodeId",
        "\"nodes\"",
        "\"edges\"",
        "private fixture",
    ] {
        assert!(!serialized.contains(forbidden));
    }
    let reopened = GameAssetsService::open(
        &fixture.directory.path,
        fixture.media.clone(),
        fixture.images.clone(),
    )
    .unwrap();
    assert_eq!(
        reopened.catalog().await.unwrap(),
        fixture.service.catalog().await.unwrap()
    );
    reopened.dispose().await.unwrap();
    fixture
        .service
        .remove(project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        fixture.service.catalog().await.unwrap(),
        json!({"projects":[],"jobs":[]})
    );
    fixture.close().await;
}
#[tokio::test]
async fn game_admission_cancellation_and_dispose_preserve_per_project_global_limits() {
    let fixture = Fixture::new().await;
    fixture.generator.hold.store(true, Ordering::Release);
    let first = fixture.service.save(fixture.input("First")).await.unwrap();
    let second = fixture.service.save(fixture.input("Second")).await.unwrap();
    let third = fixture.service.save(fixture.input("Third")).await.unwrap();
    let run = fixture
        .service
        .run(first["id"].as_str().unwrap())
        .await
        .unwrap();
    fixture.generator.entered.notified().await;
    assert_eq!(
        fixture
            .service
            .run(first["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_busy"
    );
    let mut patch = fixture.input("changed");
    patch["id"] = first["id"].clone();
    assert_eq!(
        fixture.service.save(patch).await.err().unwrap().code,
        "game_assets_busy"
    );
    assert_eq!(
        fixture
            .service
            .remove(first["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_busy"
    );
    fixture
        .service
        .run(second["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .run(third["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_busy"
    );
    let stopped = fixture
        .service
        .stop(run["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(stopped["status"], "cancelled");
    assert_eq!(stopped["error"], "game_assets_cancelled");
    fixture
        .service
        .run(third["id"].as_str().unwrap())
        .await
        .unwrap();
    fixture.service.dispose().await.unwrap();
    assert!(fixture.service.catalog().await.unwrap()["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|job| job["status"] == "cancelled"));
    assert_eq!(
        fixture
            .service
            .run(first["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_closed"
    );
    assert_eq!(
        std::fs::read_dir(fixture.directory.path.join("image-operation-jobs"))
            .unwrap()
            .count(),
        0
    );
    fixture.close().await;
}
#[tokio::test]
async fn game_failed_paid_direction_keeps_prior_actions_and_safe_partial_media() {
    let fixture = Fixture::new().await;
    fixture.generator.fail_call.store(4, Ordering::Release);
    let project = fixture
        .service
        .save(fixture.input("Partial"))
        .await
        .unwrap();
    let done = fixture
        .completed(
            &fixture
                .service
                .run(project["id"].as_str().unwrap())
                .await
                .unwrap(),
        )
        .await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["completed"], 1);
    assert_eq!(done["output"]["frames"].as_array().unwrap().len(), 5);
    assert_eq!(done["error"], "workflow_image_generation_failed");
    assert!(!done.to_string().contains("private"));
    for frame in done["output"]["frames"].as_array().unwrap() {
        assert!(fixture
            .media
            .read(frame["media"]["id"].as_str().unwrap())
            .await
            .is_ok());
    }
    assert_eq!(fixture.generator.calls.lock().unwrap().len(), 4);
    fixture.close().await;
}
#[tokio::test]
async fn game_manual_edit_replays_original_output_and_atomically_revisions_real_atlas() {
    let fixture = Fixture::new().await;
    let project = fixture
        .service
        .save(fixture.input("Editable"))
        .await
        .unwrap();
    let original = fixture
        .completed(
            &fixture
                .service
                .run(project["id"].as_str().unwrap())
                .await
                .unwrap(),
        )
        .await;
    let count = fixture.generator.calls.lock().unwrap().len();
    let first = fixture
        .service
        .edit(
            original["id"].as_str().unwrap(),
            json!({"frames":[{"sourceIndex":6,"durationMs":200,"opacity":0.5},
        {"sourceIndex":1,"rotation":45},{"sourceIndex":1,"scale":1.5}]}),
        )
        .await
        .unwrap();
    assert_eq!(first["revision"], 1);
    assert_eq!(first["output"]["frames"].as_array().unwrap().len(), 3);
    assert_eq!(first["originalOutput"], original["output"]);
    let second = fixture
        .service
        .edit(
            original["id"].as_str().unwrap(),
            json!({"frames":[{"sourceIndex":7}]}),
        )
        .await
        .unwrap();
    assert_eq!(second["revision"], 2);
    assert_eq!(second["originalOutput"], original["output"]);
    assert_eq!(second["output"]["frames"].as_array().unwrap().len(), 1);
    assert_eq!(fixture.generator.calls.lock().unwrap().len(), count);
    assert_eq!(
        fixture
            .service
            .edit(
                original["id"].as_str().unwrap(),
                json!({"frames":[{"sourceIndex":8}]})
            )
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_invalid"
    );
    assert_eq!(
        fixture
            .service
            .get_job(original["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap(),
        second
    );
    let reopened = GameAssetsService::open(
        &fixture.directory.path,
        fixture.media.clone(),
        fixture.images.clone(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .get_job(original["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap(),
        second
    );
    reopened.dispose().await.unwrap();
    fixture.close().await;
}
#[tokio::test]
async fn game_cancelled_edit_keeps_previous_revision_and_waits_owned_queue() {
    let fixture = Fixture::new().await;
    let project = fixture
        .service
        .save(fixture.input("Cancel edit"))
        .await
        .unwrap();
    let original = fixture
        .completed(
            &fixture
                .service
                .run(project["id"].as_str().unwrap())
                .await
                .unwrap(),
        )
        .await;
    fixture.algorithms.hold.store(true, Ordering::Release);
    let images = fixture.images.clone();
    let frames = original["output"]["frames"].as_array().unwrap().clone();
    let blocker = tokio::spawn(async move {
        images
            .operate(
                ImageOperationRequest {
                    operation: "inpaint".into(),
                    source: None,
                    images: frames,
                    settings: json!({}),
                    prompt: String::new(),
                    model: Value::Null,
                    resume_output: None,
                    edits: None,
                },
                Arc::new(RunCancellation::default()),
            )
            .await
    });
    fixture.algorithms.entered.notified().await;
    let service = fixture.service.clone();
    let id = original["id"].as_str().unwrap().to_string();
    let edit_id = id.clone();
    let editing = tokio::spawn(async move {
        service
            .edit(&edit_id, json!({"frames":[{"sourceIndex":0}]}))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if fixture
                .service
                .reserved(project["id"].as_str().unwrap())
                .await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture
            .service
            .run(project["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_busy"
    );
    fixture.service.stop(&id).await.unwrap();
    assert_eq!(
        editing.await.unwrap().err().unwrap().code,
        "game_assets_cancelled"
    );
    assert_eq!(
        fixture.service.get_job(&id).await.unwrap().unwrap(),
        original
    );
    fixture.algorithms.hold.store(false, Ordering::Release);
    blocker.await.unwrap().unwrap();
    fixture.close().await;
}
#[tokio::test]
async fn game_restart_marks_interrupted_without_model_calls_and_corrupt_storage_is_preserved() {
    let fixture = Fixture::new().await;
    let project = fixture
        .service
        .save(fixture.input("Recovery"))
        .await
        .unwrap();
    let job = json!({"id":crate::native_workflow::id().unwrap(),"projectId":project["id"],"status":"running","startedAt":crate::native_workflow::now(),
        "finishedAt":null,"completed":0,"total":2,"error":null,"output":protocol::empty_output(),"originalOutput":protocol::empty_output(),"revision":0});
    let path = fixture.directory.path.join("game-assets.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"version":1,"projects":[project],"jobs":[job.clone()]}))
            .unwrap(),
    )
    .unwrap();
    let restored = GameAssetsService::open(
        &fixture.directory.path,
        fixture.media.clone(),
        fixture.images.clone(),
    )
    .unwrap();
    assert_eq!(
        restored
            .get_job(job["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap()["status"],
        "interrupted"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["jobs"][0]
            ["error"],
        "game_assets_interrupted"
    );
    assert!(fixture.generator.calls.lock().unwrap().is_empty());
    restored.dispose().await.unwrap();
    for content in [
        "null",
        "{\"version\":2,\"projects\":[],\"jobs\":[]}",
        "{ private-data",
    ] {
        std::fs::write(&path, content).unwrap();
        assert_eq!(
            GameAssetsService::open(
                &fixture.directory.path,
                fixture.media.clone(),
                fixture.images.clone()
            )
            .err()
            .unwrap()
            .code,
            "game_assets_storage_invalid"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }
    fixture.close().await;
}
#[tokio::test]
async fn game_failed_atomic_project_and_edit_saves_preserve_published_values() {
    let fixture = Fixture::new().await;
    let project = fixture.service.save(fixture.input("Atomic")).await.unwrap();
    let job = fixture
        .completed(
            &fixture
                .service
                .run(project["id"].as_str().unwrap())
                .await
                .unwrap(),
        )
        .await;
    let path = fixture.directory.path.join("game-assets.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let mut input = fixture.input("Changed");
    input["id"] = project["id"].clone();
    assert_eq!(
        fixture.service.save(input).await.err().unwrap().code,
        "game_assets_storage_failed"
    );
    assert_eq!(
        fixture.service.catalog().await.unwrap()["projects"],
        json!([project])
    );
    assert_eq!(
        fixture
            .service
            .edit(
                job["id"].as_str().unwrap(),
                json!({"frames":[{"sourceIndex":0}]})
            )
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_storage_failed"
    );
    assert_eq!(
        fixture
            .service
            .get_job(job["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap(),
        job
    );
    assert_eq!(
        fixture.service.dispose().await.err().unwrap().code,
        "game_assets_storage_failed"
    );
    std::fs::remove_dir(&path).unwrap();
    fixture.images.dispose().await;
    fixture.processor.dispose().await;
    fixture.media.dispose().await;
}
#[tokio::test]
async fn game_drafts_foreign_media_and_project_update_identity_are_validated() {
    let fixture = Fixture::new().await;
    let foreign_dir = TempDirectory::new();
    let foreign = MediaService::open(&foreign_dir.path).unwrap();
    let other = foreign
        .upload(
            "other.png",
            "image/png",
            &encode_png(&RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))).unwrap(),
        )
        .await
        .unwrap();
    let mut input = fixture.input("Foreign");
    input["reference"] = other;
    assert_eq!(
        fixture.service.save(input).await.err().unwrap().code,
        "game_assets_media_invalid"
    );
    let mut forged = fixture.input("Forged");
    forged["reference"]["name"] = json!("forged.png");
    assert_eq!(
        fixture.service.save(forged).await.err().unwrap().code,
        "game_assets_media_invalid"
    );
    let draft = fixture.service.save(json!({"name":"Draft"})).await.unwrap();
    assert_eq!(
        fixture
            .service
            .run(draft["id"].as_str().unwrap())
            .await
            .err()
            .unwrap()
            .code,
        "game_assets_source_required"
    );
    let mut input = fixture.input("Updated");
    input["id"] = draft["id"].clone();
    let saved = fixture.service.save(input).await.unwrap();
    assert_eq!(saved["createdAt"], draft["createdAt"]);
    let mut missing = fixture.input("missing");
    missing["id"] = json!(crate::native_workflow::id().unwrap());
    assert_eq!(
        fixture.service.save(missing).await.err().unwrap().code,
        "game_assets_not_found"
    );
    foreign.dispose().await;
    fixture.close().await;
}
#[tokio::test]
async fn game_failed_export_preserves_previous_output_original_and_revision() {
    let fixture = Fixture::new().await;
    let project = fixture
        .service
        .save(fixture.input("Export rollback"))
        .await
        .unwrap();
    let reference = fixture
        .media
        .upload(
            "wide.png",
            "image/png",
            &encode_png(&RgbaImage::from_pixel(4096, 1, Rgba([30, 70, 100, 200]))).unwrap(),
        )
        .await
        .unwrap();
    let output = json!({"type":"workflow-images","version":1,"frames":[{"media":reference,"width":4096,"height":1,"durationMs":125,
        "action":"idle","direction":"S","columns":1,"rows":1,"frameCount":1}]});
    let original=protocol::job(&json!({"id":crate::native_workflow::id().unwrap(),"projectId":project["id"],"status":"completed",
        "startedAt":crate::native_workflow::now(),"finishedAt":crate::native_workflow::now(),"completed":1,"total":1,"error":null,
        "output":output,"originalOutput":output,"revision":0})).unwrap();
    std::fs::write(
        fixture.directory.path.join("game-assets.json"),
        serde_json::to_vec(&json!({"version":1,"projects":[project],"jobs":[original.clone()]}))
            .unwrap(),
    )
    .unwrap();
    let restored = GameAssetsService::open(
        &fixture.directory.path,
        fixture.media.clone(),
        fixture.images.clone(),
    )
    .unwrap();
    // 编辑固定画布合法，随后带 padding 的图集超过 4096；失败不能发布新修订。
    assert_eq!(
        restored
            .edit(
                original["id"].as_str().unwrap(),
                json!({"frames":[{"sourceIndex":0,"opacity":0.5}]})
            )
            .await
            .err()
            .unwrap()
            .code,
        "workflow_image_too_large"
    );
    assert_eq!(
        restored
            .get_job(original["id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(
        serde_json::from_slice::<Value>(
            &std::fs::read(fixture.directory.path.join("game-assets.json")).unwrap()
        )
        .unwrap()["jobs"][0],
        original
    );
    restored.dispose().await.unwrap();
    fixture.close().await;
}
