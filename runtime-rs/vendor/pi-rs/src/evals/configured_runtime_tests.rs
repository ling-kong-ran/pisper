//! Ports of `pi/packages/evals/test/configured-runtime.test.ts` — provider
//! probes through the ported `ModelRuntime` against the loopback acme fixture
//! server (NO_PROXY loopback only, no real credentials).

use super::{
    inspect_added_model, inspect_provider, load_configured_model_runtime, AddedModelResult,
    CostFields, ModelFields, ProviderRuntimeResult, ProviderScenario,
};
use crate::evals::acme_server::{
    AcmeServer, ServerMode, OPENAI_MODEL_ID, OPENAI_PROBE_PROMPT, OPENAI_PROVIDER_ID,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct StartedServer {
    server: Arc<AcmeServer>,
}

/// Upstream runs this file's tests sequentially in one vitest worker; the
/// ported runtime registers the provider configuration process-globally, so
/// the parallel Rust test harness must serialize these tests the same way.
static PROVIDER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn lock_provider_tests() -> tokio::sync::MutexGuard<'static, ()> {
    PROVIDER_TEST_LOCK.lock().await
}

impl Default for StartedServer {
    fn default() -> Self {
        Self {
            server: AcmeServer::start(ServerMode::OpenAi).expect("bind loopback"),
        }
    }
}

impl Drop for StartedServer {
    fn drop(&mut self) {
        self.server.stop();
    }
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let path = std::env::temp_dir().join(format!(
        "pi-eval-provider-probe-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("mkdir");
    TempDir(path)
}

fn expected_model(provider: &str, id: &str, name: &str, reasoning: bool) -> ModelFields {
    ModelFields {
        id: id.to_string(),
        name: name.to_string(),
        provider: provider.to_string(),
        reasoning,
        input: vec!["text".to_string()],
        cost: CostFields {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        context_window: 32768,
        max_tokens: 4096,
    }
}

fn write_models_json(agent_dir: &Path, models_json: &serde_json::Value) {
    std::fs::write(
        agent_dir.join("models.json"),
        format!("{}\n", serde_json::to_string(models_json).unwrap()),
    )
    .expect("write models.json");
}

fn acme_models_json(base_url: &str) -> serde_json::Value {
    serde_json::json!({
        "providers": {
            OPENAI_PROVIDER_ID: {
                "baseUrl": base_url,
                "api": "openai-completions",
                "apiKey": "$ACME_API_KEY",
                "models": [
                    {
                        "id": OPENAI_MODEL_ID,
                        "name": "Acme Chat",
                        "reasoning": false,
                        "input": ["text"],
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                        "contextWindow": 32768,
                        "maxTokens": 4096,
                    }
                ],
            }
        }
    })
}

fn probe(server: &StartedServer, content: &str, api_key: &str) -> ProviderScenario {
    let mut env = BTreeMap::new();
    env.insert("ACME_API_KEY".to_string(), api_key.to_string());
    let valid_server = Arc::clone(&server.server);
    let valid_request_received: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || valid_server.valid_request_received());
    ProviderScenario {
        provider_id: OPENAI_PROVIDER_ID.to_string(),
        model_id: OPENAI_MODEL_ID.to_string(),
        prompt: content.to_string(),
        env: Some(env),
        max_tokens: Some(32),
        valid_request_received,
    }
}

async fn probe_result(
    server: &StartedServer,
    content: &str,
    api_key: &str,
) -> ProviderRuntimeResult {
    let agent_dir = temp_dir("runtime");
    write_models_json(&agent_dir.0, &acme_models_json(&server.server.base_url()));
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    server.server.reset();
    inspect_provider(&runtime, probe(server, content, api_key))
        .await
        .result
}

#[tokio::test]
async fn probes_a_models_json_provider_through_model_runtime() {
    let _guard = lock_provider_tests().await;
    let server = StartedServer::default();
    let result = probe_result(&server, OPENAI_PROBE_PROMPT, "resolved-acme-key").await;
    assert_eq!(
        result,
        ProviderRuntimeResult::Probed {
            valid_request_received: true,
            model: expected_model(OPENAI_PROVIDER_ID, OPENAI_MODEL_ID, "Acme Chat", false),
            response: super::ResponseSummary {
                text: "ACME_OK".to_string(),
                stop_reason: "stop".to_string(),
                input_tokens: 3,
                output_tokens: 2,
            },
        },
        "result: {result:?}"
    );
}

#[tokio::test]
async fn keeps_a_non_probe_completion_distinct_from_a_valid_probe() {
    let _guard = lock_provider_tests().await;
    let server = StartedServer::default();
    let result = probe_result(&server, "hello", "resolved-acme-key").await;
    assert_eq!(
        result,
        ProviderRuntimeResult::Probed {
            valid_request_received: false,
            model: expected_model(OPENAI_PROVIDER_ID, OPENAI_MODEL_ID, "Acme Chat", false),
            response: super::ResponseSummary {
                text: "ACME_OK".to_string(),
                stop_reason: "stop".to_string(),
                input_tokens: 3,
                output_tokens: 2,
            },
        },
        "result: {result:?}"
    );
}

#[tokio::test]
async fn returns_a_structured_error_when_the_configured_model_is_missing() {
    let _guard = lock_provider_tests().await;
    let server = StartedServer::default();
    let agent_dir = temp_dir("missing");
    write_models_json(&agent_dir.0, &serde_json::json!({ "providers": {} }));
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    server.server.reset();
    let output = inspect_provider(
        &runtime,
        probe(&server, OPENAI_PROBE_PROMPT, "resolved-acme-key"),
    )
    .await;
    match output.result {
        ProviderRuntimeResult::Failed { error } => {
            assert_eq!(
                error,
                format!(
                    "Model {OPENAI_PROVIDER_ID}/{OPENAI_MODEL_ID} is unavailable after reload."
                )
            );
        }
        other => panic!("expected structured error, got {other:?}"),
    }
    assert!(!server.server.valid_request_received());
}

#[tokio::test]
async fn returns_a_structured_error_when_models_json_cannot_be_parsed() {
    let _guard = lock_provider_tests().await;
    let server = StartedServer::default();
    let agent_dir = temp_dir("broken");
    std::fs::write(agent_dir.0.join("models.json"), "{").expect("write broken models.json");
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    let output = inspect_provider(
        &runtime,
        probe(&server, OPENAI_PROBE_PROMPT, "resolved-acme-key"),
    )
    .await;
    match output.result {
        ProviderRuntimeResult::Failed { error } => {
            assert!(error.contains("Failed to parse models.json"), "{error}");
        }
        other => panic!("expected structured error, got {other:?}"),
    }
    assert!(!server.server.valid_request_received());
}

#[tokio::test]
async fn does_not_treat_an_unauthorized_completion_as_a_valid_probe() {
    let _guard = lock_provider_tests().await;
    let server = StartedServer::default();
    let result = probe_result(&server, OPENAI_PROBE_PROMPT, "wrong-key").await;
    assert_eq!(
        result,
        ProviderRuntimeResult::Probed {
            valid_request_received: false,
            model: expected_model(OPENAI_PROVIDER_ID, OPENAI_MODEL_ID, "Acme Chat", false),
            response: super::ResponseSummary {
                text: String::new(),
                stop_reason: "error".to_string(),
                input_tokens: 0,
                output_tokens: 0,
            },
        },
        "result: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// inspectAddedModel
// ---------------------------------------------------------------------------

const ADDED_PROVIDER: &str = "openai";
const ADDED_MODEL_ID: &str = "fixture-chat";

fn added_models_json() -> serde_json::Value {
    serde_json::json!({
        "providers": {
            ADDED_PROVIDER: {
                "models": [
                    {
                        "id": ADDED_MODEL_ID,
                        "name": "Fixture Chat",
                        "reasoning": true,
                        "input": ["text"],
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
                        "contextWindow": 32768,
                        "maxTokens": 4096,
                    }
                ],
            }
        }
    })
}

#[tokio::test]
async fn loads_an_added_model_without_dropping_built_in_models() {
    let _guard = lock_provider_tests().await;
    let agent_dir = temp_dir("added");
    write_models_json(&agent_dir.0, &added_models_json());
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    let output = inspect_added_model(&runtime, ADDED_PROVIDER, ADDED_MODEL_ID).await;
    match output.result {
        AddedModelResult::Added {
            model,
            existing_models_preserved,
        } => {
            assert_eq!(
                model,
                expected_model(ADDED_PROVIDER, ADDED_MODEL_ID, "Fixture Chat", true)
            );
            assert!(existing_models_preserved, "built-in models preserved");
        }
        other => panic!("expected added model, got {other:?}"),
    }
}

#[tokio::test]
async fn returns_a_structured_error_when_the_added_model_is_missing() {
    let _guard = lock_provider_tests().await;
    let agent_dir = temp_dir("added-missing");
    write_models_json(&agent_dir.0, &serde_json::json!({ "providers": {} }));
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    let output = inspect_added_model(&runtime, ADDED_PROVIDER, ADDED_MODEL_ID).await;
    match output.result {
        AddedModelResult::Failed { error } => {
            assert_eq!(
                error,
                format!("Model {ADDED_PROVIDER}/{ADDED_MODEL_ID} is unavailable after reload.")
            );
        }
        other => panic!("expected structured error, got {other:?}"),
    }
}

#[tokio::test]
async fn returns_a_structured_error_when_added_models_json_cannot_be_parsed() {
    let _guard = lock_provider_tests().await;
    let agent_dir = temp_dir("added-broken");
    std::fs::write(agent_dir.0.join("models.json"), "{").expect("write broken models.json");
    let runtime = load_configured_model_runtime(&agent_dir.0)
        .await
        .expect("runtime");
    let output = inspect_added_model(&runtime, ADDED_PROVIDER, ADDED_MODEL_ID).await;
    match output.result {
        AddedModelResult::Failed { error } => {
            assert!(error.contains("Failed to parse models.json"), "{error}");
        }
        other => panic!("expected structured error, got {other:?}"),
    }
}
