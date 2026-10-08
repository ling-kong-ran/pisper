use super::{
    catalog::{self, Catalog},
    downloads::SpeechDownloads,
    storage, terms,
};
use serde_json::json;
use std::{fs, path::PathBuf};
use tokio_util::sync::CancellationToken;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pisper-speech-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn shared_catalog_and_release_public_contract_are_real() {
    let catalog = Catalog::shared().unwrap();
    assert_eq!(catalog.models.len(), 2);
    assert_eq!(catalog.models[0].total_bytes(), 133895136);
    assert_eq!(
        catalog.models[0].fingerprint,
        "0beb61309fe0482240d08282ad4d5c0849fb67c7eb94ffb8bec232a1c94671d8"
    );
    assert_eq!(
        catalog.models[1].fingerprint,
        "f813f57500db3e2f12624f54420ab23ae0e3af8f75a1d721dc04054f3971a3e2"
    );
    let model = &catalog.models[0];
    let public = model.public("not-installed", 0, None);
    assert!(public.get("files").is_none());
    assert!(public.get("config").is_none());
    assert_eq!(public["filesBytes"], 169347218u64);
    assert!(!catalog::safe_relative("../escape"));
    assert!(!catalog::safe_relative("CON.onnx"));
    assert!(!catalog::safe_relative("dict/zero\u{200b}.txt"));
    for url in [
        "http://models.example/a",
        "https://127.0.0.1/a",
        "https://user:secret@models.example/a",
        "https://models.local/a",
    ] {
        assert!(!catalog::public_url(url));
    }
}
#[test]
fn pcm_wav_limits_follow_release_float_and_rounding_rules() {
    use super::native;
    assert!(native::decode_pcm(&[0, 0, 0]).is_err());
    assert!(native::decode_pcm(&f32::NAN.to_le_bytes()).is_err());
    let wav = native::encode_wav(&[-2.0, -0.5, 0.0, 0.5, 2.0], 16000).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(wav.len(), 54);
    let pcm = wav[44..]
        .chunks_exact(2)
        .map(|v| i16::from_le_bytes(v.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(pcm, [-32768, -16384, 0, 16384, 32767]);
    assert!(native::validate_text("🦀".repeat(201).as_str(), 400).is_err());
    assert!(native::validate_text("你好，世界。", 16).is_ok());
}
#[cfg(windows)]
#[test]
fn actual_staged_native_library_exports_pinned_sherpa_and_ort_versions() {
    let native_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../node_modules/sherpa-onnx-win-x64");
    let version = super::native::NativeInference::version(&native_dir).unwrap();
    assert_eq!(version["sherpa"], "1.13.7");
    assert!(version["onnxruntime"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));
}
#[cfg(windows)]
#[tokio::test]
#[ignore = "Downloads the actual shared ASR/TTS models into an isolated fixture; explicit CPU evidence run"]
async fn actual_catalog_download_native_tts_and_asr_cpu_roundtrip() {
    use super::native::NativeInference;
    use base64::Engine;
    let fixture = Fixture::new();
    let catalog = Catalog::shared().unwrap();
    let downloads = SpeechDownloads::new(&fixture.0, catalog).unwrap();
    for id in ["x-asr-480ms-int8", "vits-melo-tts-zh_en"] {
        eprintln!("native speech fixture downloading {id}");
        let result = downloads.download(id).await.unwrap();
        assert_eq!(result["status"], "installed");
    }
    let source: serde_json::Value = serde_json::from_str(catalog::CATALOG).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let staged_root = std::env::var_os("PISPER_SPEECH_TEST_APP_ROOT").map(PathBuf::from);
    let native_dir = fs::canonicalize(
        staged_root
            .as_ref()
            .map(|root| root.join("speech-native"))
            .unwrap_or_else(|| root.join("node_modules/sherpa-onnx-win-x64")),
    )
    .unwrap();
    let resource_dir =
        fs::canonicalize(staged_root.as_ref().unwrap_or(&root).join("shared")).unwrap();
    let worker_executable = std::env::var_os("PISPER_SPEECH_TEST_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("runtime-rs/target/debug/pisper-server.exe"));
    let parameters = |kind: &str, id: &str| json!({"kind":kind,"model":source["models"].as_array().unwrap().iter().find(|m|m["id"]==id).unwrap(),"modelDir":fixture.0.join("speech-models").join(id),"nativeLibraryDir":native_dir,"resourceDir":resource_dir,"hotwordsDir":fixture.0.join("speech")});
    let mut tts = NativeInference::new(&parameters("tts", "vits-melo-tts-zh_en")).unwrap();
    let generated = tts
        .handle("synthesize", &json!({"text":"你好，世界。"}))
        .unwrap();
    let wav = base64::engine::general_purpose::STANDARD
        .decode(generated["wav"].as_str().unwrap())
        .unwrap();
    assert!(wav.len() > 44 + 1600);
    let rate = generated["sampleRate"].as_u64().unwrap() as usize;
    let samples = wav[44..]
        .chunks_exact(2)
        .map(|v| i16::from_le_bytes(v.try_into().unwrap()) as f32 / 32768.0)
        .collect::<Vec<_>>();
    let length = samples.len() * 16000 / rate;
    let resampled = (0..length)
        .map(|index| {
            let position = index as f64 * rate as f64 / 16000.0;
            let base = position.floor() as usize;
            let fraction = (position - base as f64) as f32;
            samples[base] * (1.0 - fraction) + samples[(base + 1).min(samples.len() - 1)] * fraction
        })
        .collect::<Vec<_>>();
    let pcm = resampled
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(pcm);
    drop(tts);
    let mut asr = NativeInference::new(&parameters("asr", "x-asr-480ms-int8")).unwrap();
    let result = asr
        .handle("transcribe", &json!({"pcm":encoded,"terms":[]}))
        .unwrap();
    let text = result["text"].as_str().unwrap();
    eprintln!("native speech fixture transcript: {text}");
    assert!(text.contains("你好"));
    let started = asr.handle("startSession", &json!({"terms":[]})).unwrap();
    let id = started["id"].as_str().unwrap();
    for chunk in resampled.chunks(3200) {
        let pcm = chunk
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        asr.handle(
            "acceptChunk",
            &json!({"id":id,"pcm":base64::engine::general_purpose::STANDARD.encode(pcm)}),
        )
        .unwrap();
    }
    let finished = asr.handle("finishSession", &json!({"id":id})).unwrap();
    assert_eq!(finished["text"], result["text"]);
    drop(asr);
    // Run the same verified models through the installed Rust worker protocol and HTTP body lifecycle.
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use futures::StreamExt;
    use tower::ServiceExt;
    let service = super::SpeechService::with_executable(
        &fixture.0,
        &resource_dir,
        &native_dir,
        worker_executable,
    )
    .unwrap();
    let resolve: super::SpeechSessionResolver =
        std::sync::Arc::new(|_| Box::pin(async { Ok(None) }));
    let router = super::router::<()>(service.clone(), resolve);
    let prepare_id = uuid::Uuid::new_v4().to_string();
    let ready = router.clone().oneshot(Request::builder().method("POST").uri("/api/speech/session").header("content-type", "application/json").body(Body::from(json!({"requestId":prepare_id,"kinds":["asr","tts"],"hotwords":terms::BUILTIN.join("\n")}).to_string())).unwrap()).await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        ready.headers()["content-type"],
        "text/event-stream; charset=utf-8"
    );
    let mut events = ready.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&first).unwrap(),
        "event: ready\ndata: {\"ready\":true}\n\n"
    );
    assert_eq!(service.fixture_lease_count(), 1);
    let worker_ids = service.fixture_worker_ids().await;
    assert_eq!(worker_ids.len(), 2);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/speech/synthesize")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"requestId":uuid::Uuid::new_v4().to_string(),"text":"你好，世界。"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "audio/wav");
    let http_wav = to_bytes(response.into_body(), 5_000_000).await.unwrap();
    assert!(http_wav.len() > 1644);
    assert_eq!(&http_wav[..4], b"RIFF");
    let raw_pcm = resampled
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/speech/transcribe")
                .header("x-pisper-sample-rate", "16000")
                .body(Body::from(raw_pcm))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let dictation: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap();
    assert!(dictation["text"].as_str().unwrap().contains("你好"));
    let started = service
        .start_session(terms::BUILTIN.iter().map(|v| (*v).to_owned()).collect())
        .await
        .unwrap();
    let stream_id = started["id"].as_str().unwrap();
    for chunk in resampled.chunks(3200) {
        let raw = chunk
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        service.chunk(stream_id, &raw).await.unwrap();
    }
    assert_eq!(service.finish_session(stream_id).await.unwrap(), dictation);
    let cancel_id = uuid::Uuid::new_v4().to_string();
    let cancelled_id = cancel_id.clone();
    let operation = service.clone();
    let synthesis = tokio::spawn(async move {
        operation
            .synthesize(
                &json!({"requestId":cancel_id,"text":"这是语音识别和合成的真实测试"}),
                CancellationToken::new(),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            service.cancel_speech(&json!({"requestId":cancelled_id}))
        )
        .await
        .unwrap()
        .unwrap(),
        json!({"cancelled":true})
    );
    assert_eq!(synthesis.await.unwrap().unwrap_err().code, "cancelled");
    drop(events);
    assert_eq!(service.fixture_lease_count(), 0);
    service.shutdown().await;
    assert!(service.fixture_worker_ids().await.is_empty());
    for id in worker_ids {
        assert!(!fixture_process_alive(id));
    }
    downloads.shutdown().await;
}

#[cfg(windows)]
fn fixture_process_alive(id: u32) -> bool {
    // The IDs are captured only from this fixture's owned worker children, never enumerated from the user's processes.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(0x00100000, 0, id);
        if handle.is_null() {
            return false;
        }
        let alive = WaitForSingleObject(handle, 0) == 0x102;
        CloseHandle(handle);
        alive
    }
}
#[test]
fn terms_settings_restart_manifest_and_spelling_boundaries() {
    let fixture = Fixture::new();
    let terms = terms::SpeechTerms::new(&fixture.0);
    assert_eq!(terms.settings().unwrap()["projectTermsEnabled"], true);
    assert!(terms.update(&json!({"customTerms":["ignored"]})).is_err());
    assert!(terms.update(&json!({"projectTermsEnabled":1})).is_err());
    fs::write(
        fixture.0.join("package.json"),
        r#"{"name":"@fixture/MyLibrary","dependencies":{"some-otherPackage":"*","rust":"*"}}"#,
    )
    .unwrap();
    fs::write(fixture.0.join("Cargo.toml"),"[package]\nname='nativeSample'\n[dependencies]\nserde_json='1'\n[target.'cfg(windows)'.dependencies]\nwindows-sys='0.61'\n").unwrap();
    let words = terms.workspace(Some(&fixture.0)).unwrap();
    assert!(words.contains(&"My Library".to_owned()));
    assert!(words.contains(&"native Sample".to_owned()));
    assert!(words.contains(&"windows sys".to_owned()));
    let formatted = terms::format("typescript cargo test pi python myreact React.ts", &words);
    assert_eq!(
        formatted,
        "TypeScript cargo test pi Python myreact React.ts"
    );
    terms.update(&json!({"projectTermsEnabled":false})).unwrap();
    let reopened = terms::SpeechTerms::new(&fixture.0);
    assert_eq!(reopened.settings().unwrap()["projectTermsEnabled"], false);
    assert_eq!(
        reopened.workspace(Some(&fixture.0)).unwrap().len(),
        terms::BUILTIN.len()
    );
}
#[tokio::test]
async fn legacy_marker_restart_hash_and_tree_tampering_are_verified() {
    let fixture = Fixture::new();
    let bytes = b"actual fixture model bytes";
    let sha = catalog::hash(bytes);
    let catalog=Catalog::parse(&json!({"defaults":{"asr":"fixture"},"models":[{"id":"fixture","kind":"asr","engine":"online-transducer","name":"Fixture","languages":["en"],"license":{},"config":{},"files":[{"path":"model.onnx","bytes":bytes.len(),"sha256":sha,"urls":["https://models.example/model"]}]}]}).to_string()).unwrap();
    let model = catalog.models[0].clone();
    let path = fixture.0.join("speech-models/fixture");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("model.onnx"), bytes).unwrap();
    storage::write_json(&path.join(".installation.json"), &model.marker()).unwrap();
    let downloads = SpeechDownloads::new(&fixture.0, catalog.clone()).unwrap();
    assert_eq!(
        downloads.status("fixture").await.unwrap()["status"],
        "installed"
    );
    assert_eq!(downloads.model_directory("fixture").await.unwrap(), path);
    fs::write(path.join("model.onnx"), b"replacement with bad bytes").unwrap();
    assert!(downloads.model_directory("fixture").await.is_err());
    assert_eq!(
        downloads.status("fixture").await.unwrap()["status"],
        "not-installed"
    );
    fs::write(path.join("model.onnx"), bytes).unwrap();
    fs::write(path.join("unexpected.txt"), b"extra").unwrap();
    let reopened = SpeechDownloads::new(&fixture.0, catalog).unwrap();
    assert!(reopened.model_directory("fixture").await.is_err());
    downloads.shutdown().await;
    reopened.shutdown().await;
    assert!(storage::digest_file(
        &path.join("model.onnx"),
        bytes.len() as u64,
        &CancellationToken::new()
    )
    .is_ok());
}
#[tokio::test]
async fn actual_http_settings_models_and_pcm_rejections_follow_release() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let fixture = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let service = super::SpeechService::new(
        &fixture.0,
        &root.join("shared"),
        &root.join("node_modules/sherpa-onnx-win-x64"),
    )
    .unwrap();
    let resolve: super::SpeechSessionResolver =
        std::sync::Arc::new(|_| Box::pin(async { Ok(None) }));
    let router = super::router::<()>(service.clone(), resolve);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/speech/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["models"].as_array().unwrap().len(), 2);
    assert_eq!(body["models"][0]["status"], "not-installed");
    for (route, input, status, message) in [
        (
            "/api/speech/session",
            json!({"requestId":"invalid","kinds":["asr"],"extra":true}),
            StatusCode::BAD_REQUEST,
            "Invalid speech request.",
        ),
        (
            "/api/speech/models/download",
            json!({"modelId":"unknown"}),
            StatusCode::NOT_FOUND,
            "Unknown speech model.",
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(route)
                    .header("content-type", "application/json")
                    .body(Body::from(input.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(body["error"], message);
    }
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/speech/transcribe")
                .header("x-pisper-sample-rate", "16000")
                .body(Body::from(f32::NAN.to_le_bytes().to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/settings/speech")
                .header("content-type", "application/json")
                .body(Body::from("{\"projectTermsEnabled\":false}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        service.terms.settings().unwrap()["projectTermsEnabled"],
        false
    );
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        service
            .cancel_speech(&json!({"requestId":id}))
            .await
            .unwrap(),
        json!({"cancelled":false})
    );
    assert_eq!(
        service.cancel_session("missing").await.unwrap(),
        json!({"ok":true})
    );
    service.shutdown().await;
}
