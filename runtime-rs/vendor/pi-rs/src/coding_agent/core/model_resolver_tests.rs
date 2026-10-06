//! Tests for the ported `coding-agent/src/core/model-resolver.ts`.
//!
//! Every table comes from
//! `tests/fixtures/core_oracle_model/model_resolver.oracle.json` (the real upstream
//! module under node; generator `oracle_model_resolver.mjs`). The model
//! fixtures mirror the oracle script's `model()` factory and the upstream
//! test battery; results compare as canonical JSON (models render as
//! `provider/id` references like the capture).

use std::collections::HashMap;

use crate::ai::types::primitives::ModelCost;
use crate::ai::types::{Model, ModelInput};
use crate::coding_agent::core::model_resolver::{
    default_model_per_provider, find_exact_model_reference_match, find_initial_model,
    is_valid_thinking_level, models_are_equal, parse_model_pattern, resolve_cli_model,
    resolve_model_scope_from_models, restore_model_from_session, FindInitialModelOptions,
    ParseModelPatternOptions, ResolveCliModelOptions, ScopedModel, ThinkingLevel,
};

/// Upstream model-resolver capture (real upstream module under node;
/// generator `tests/fixtures/core_oracle_model/oracle_model_resolver.mjs`).
const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_model/model_resolver.oracle.json");

fn oracle() -> serde_json::Value {
    serde_json::from_str(ORACLE).unwrap()
}

/// The oracle script's `model()` fixture factory (upstream test battery).
fn model(provider: &str, id: &str) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: id.to_string(),
        name: id.to_string(),
        api: "anthropic-messages".to_string(),
        provider: provider.to_string(),
        base_url: "https://example.invalid".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 1.0,
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 8192,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

fn all_models() -> Vec<Model> {
    vec![
        with_reasoning(model("anthropic", "claude-sonnet-4-5"), true),
        model("openai", "gpt-4o"),
        with_reasoning(model("openrouter", "qwen/qwen3-coder:exacto"), true),
        model("openrouter", "openai/gpt-4o:extended"),
        model("openrouter", "openai/gpt-4o-20250101"),
        with_reasoning(model("custom", "bracketed-model[1m]"), true),
    ]
}

fn with_reasoning(mut model: Model, reasoning: bool) -> Model {
    model.reasoning = reasoning;
    model
}

/// Render a model the way the oracle capture does (`provider/id`).
fn reference(model: &Model) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn parsed_json(
    result: &crate::coding_agent::core::model_resolver::ParsedModelResult,
) -> serde_json::Value {
    serde_json::json!({
        "model": result.model.as_ref().map(reference),
        "thinkingLevel": result.thinking_level.map(level_string),
        "warning": result.warning,
    })
}

fn level_string(level: ThinkingLevel) -> serde_json::Value {
    serde_json::to_value(level).unwrap()
}

#[test]
fn default_model_table_matches_the_upstream_oracle() {
    let oracle_doc = oracle();
    let oracle = oracle_doc["defaultModelPerProvider"].as_object().unwrap();
    assert_eq!(
        crate::coding_agent::core::model_resolver::DEFAULT_MODEL_PER_PROVIDER.len(),
        oracle.len()
    );
    for (provider, model_id) in oracle {
        assert_eq!(
            default_model_per_provider(provider).unwrap(),
            model_id.as_str().unwrap(),
            "default for {provider}"
        );
    }
}

#[test]
fn thinking_level_validation_matches_the_canonical_list() {
    for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert!(is_valid_thinking_level(level), "{level}");
    }
    for level in ["", "OFF", "max:high", "medium ", "ultra"] {
        assert!(!is_valid_thinking_level(level), "{level}");
    }
}

/// The upstream `parseModelPattern` battery (normal and strict modes).
#[test]
fn parse_model_pattern_matches_the_upstream_oracle() {
    let oracle = oracle();
    let models = all_models();
    let cases: Vec<&serde_json::Value> = oracle["parseModelPattern"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert!(cases.len() >= 20, "oracle battery size");
    for case in cases {
        let pattern = case["pattern"].as_str().unwrap();
        let result = parse_model_pattern(pattern, &models, None);
        let actual = parsed_json(&result);
        let expected_value = serde_json::json!({
            "model": case["model"],
            "thinkingLevel": case["thinkingLevel"],
            "warning": case["warning"],
        });
        assert_eq!(actual, expected_value, "parse case {pattern:?}");
    }
}

/// The strict CLI mode (`allowInvalidThinkingLevelFallback: false`).
#[test]
fn parse_model_pattern_strict_matches_the_upstream_oracle() {
    let oracle = oracle();
    let models = all_models();
    for case in oracle["parseModelPatternStrict"].as_array().unwrap() {
        let pattern = case["pattern"].as_str().unwrap();
        let result = parse_model_pattern(
            pattern,
            &models,
            Some(ParseModelPatternOptions {
                allow_invalid_thinking_level_fallback: false,
            }),
        );
        let actual = parsed_json(&result);
        let expected_value = serde_json::json!({
            "model": case["model"],
            "thinkingLevel": case["thinkingLevel"],
            "warning": case["warning"],
        });
        assert_eq!(actual, expected_value, "strict case {pattern:?}");
    }
}

/// The glob + non-glob scoping battery.
#[test]
fn resolve_model_scope_matches_the_upstream_oracle() {
    let oracle = oracle();
    let models = all_models();
    let scope_json =
        |result: &crate::coding_agent::core::model_resolver::ResolveModelScopeResult| {
            serde_json::json!({
                "scopedModels": result.scoped_models.iter().map(|scoped: &ScopedModel| serde_json::json!({
                    "model": reference(&scoped.model),
                    "thinkingLevel": scoped.thinking_level.map(level_string),
                })).collect::<Vec<_>>(),
                "diagnostics": result.diagnostics.iter().map(|diagnostic| serde_json::json!({
                    "type": "warning",
                    "code": diagnostic.code.as_str(),
                    "message": diagnostic.message,
                    "pattern": diagnostic.pattern,
                })).collect::<Vec<_>>(),
            })
        };
    for (index, case) in oracle["resolveModelScope"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        // Rebuild the pattern list from the diagnostics + scopedModels order
        // is impossible from the capture alone, so the capture stores the
        // scenario inputs implicitly; drive the same ten scenarios as the
        // generator (see oracle_model_resolver.mjs `results.resolveModelScope`).
        let scenarios: [&[&str]; 10] = [
            &["sonnet:high", "gpt-4o:invalid", "missing"],
            &["anthropic/*"],
            &["*sonnet*"],
            &["custom/bracketed-model[1m]"],
            &["custom/bracketed-model[1m]:high"],
            &["claude-sonnet-4-5", "sonnet:low"],
            &["*/gpt-4o*", "openai/gpt-4o"],
            &["?pt-4o"],
            &["openrouter/qwen/qwen3-coder:exacto:high"],
            &["claude-sonnet-4-5:high", "claude-sonnet-4-5"],
        ];
        let patterns: Vec<String> = scenarios[index].iter().map(|p| p.to_string()).collect();
        let result = resolve_model_scope_from_models(&patterns, &models);
        assert_eq!(
            scope_json(&result),
            *case,
            "scope scenario {index}: {patterns:?}"
        );
    }
}

/// The structural stub runtime used by the CLI/initial/restore batteries
/// (upstream tests use the same duck-typed shape).
struct StubRuntime {
    models: Vec<Model>,
    available: Vec<Model>,
    model_lookup: HashMap<(String, String), Model>,
    configured_auth: Vec<String>,
    /// The oracle's `hasConfiguredAuth: () => true` stubs.
    all_auth: bool,
}

impl StubRuntime {
    fn with_auth(mut self, providers: &[&str]) -> Self {
        self.configured_auth = providers.iter().map(|p| p.to_string()).collect();
        self
    }

    fn with_all_auth(mut self) -> Self {
        self.all_auth = true;
        self
    }
}

impl Default for StubRuntime {
    fn default() -> Self {
        Self {
            models: all_models(),
            available: Vec::new(),
            model_lookup: HashMap::new(),
            configured_auth: Vec::new(),
            all_auth: false,
        }
    }
}

impl crate::coding_agent::core::model_resolver::ModelRuntimeReads for StubRuntime {
    fn get_models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn has_configured_auth(&self, provider_id: &str) -> bool {
        self.all_auth
            || self
                .configured_auth
                .iter()
                .any(|provider| provider == provider_id)
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        self.available.clone()
    }

    fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        if let Some(model) = self
            .model_lookup
            .get(&(provider_id.to_string(), model_id.to_string()))
        {
            return Some(model.clone());
        }
        self.models
            .iter()
            .find(|model| model.provider == provider_id && model.id == model_id)
            .cloned()
    }

    fn get_available<'a>(
        &'a self,
        _options: Option<&crate::ai::auth::types::AuthOperationOptions>,
    ) -> futures::future::BoxFuture<'a, Vec<Model>> {
        Box::pin(async move { self.get_available_snapshot() })
    }
}

fn cli_result_json(
    result: &crate::coding_agent::core::model_resolver::ResolveCliModelResult,
) -> serde_json::Value {
    serde_json::json!({
        "model": result.model.as_ref().map(reference),
        "reasoning": result.model.as_ref().map(|model| model.reasoning),
        "thinkingLevel": result.thinking_level.map(level_string),
        "warning": result.warning,
        "error": result.error,
    })
}

/// The upstream `resolveCliModel` battery (all 20 cases).
#[test]
fn resolve_cli_model_matches_the_upstream_oracle() {
    let oracle = oracle();
    let run = |runtime: &StubRuntime,
               provider: Option<&str>,
               model: Option<&str>,
               thinking: Option<ThinkingLevel>| {
        cli_result_json(&resolve_cli_model(ResolveCliModelOptions {
            cli_provider: provider,
            cli_model: model,
            cli_thinking: thinking,
            model_runtime: runtime,
        }))
    };

    let cases = oracle["resolveCliModel"].as_array().unwrap();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let expected = serde_json::json!({
            "model": case["model"],
            "reasoning": case["reasoning"],
            "thinkingLevel": case["thinkingLevel"],
            "warning": case["warning"],
            "error": case["error"],
        });
        let actual = match name {
            "provider_id_no_provider" => {
                run(&StubRuntime::default(), None, Some("openai/gpt-4o"), None)
            }
            "fuzzy_in_provider" => run(&StubRuntime::default(), Some("openai"), Some("4o"), None),
            "pattern_with_thinking" => {
                run(&StubRuntime::default(), None, Some("sonnet:high"), None)
            }
            "prefers_exact_raw_id" => run(
                &StubRuntime::default(),
                None,
                Some("openai/gpt-4o:extended"),
                None,
            ),
            "invalid_suffix_kept_strict" => run(
                &StubRuntime::default(),
                Some("openai"),
                Some("gpt-4o:extended"),
                None,
            ),
            "double_prefix_custom_id" => run(
                &StubRuntime::default(),
                Some("openrouter"),
                Some("openrouter/openai/ghost-model"),
                None,
            ),
            "no_models" => run(
                &StubRuntime {
                    models: Vec::new(),
                    ..StubRuntime::default()
                },
                Some("openai"),
                Some("gpt-4o"),
                None,
            ),
            "unknown_provider" => run(
                &StubRuntime::default(),
                Some("not-a-provider"),
                Some("x"),
                None,
            ),
            "provider_prefixed_fuzzy" => {
                run(&StubRuntime::default(), None, Some("openrouter/qwen"), None)
            }
            "prefers_provider_split" => {
                let mut runtime = StubRuntime::default();
                let mut zai = model("zai", "glm-5");
                zai.name = "GLM-5".to_string();
                zai.reasoning = true;
                zai.base_url = "https://open.bigmodel.cn/api/paas/v4".to_string();
                let mut gateway = model("vercel-ai-gateway", "zai/glm-5");
                gateway.name = "GLM-5".to_string();
                gateway.reasoning = true;
                gateway.base_url = "https://ai-gateway.vercel.sh".to_string();
                runtime.models.push(zai);
                runtime.models.push(gateway);
                run(&runtime.with_all_auth(), None, Some("zai/glm-5"), None)
            }
            "ambiguous_bare_id_no_auth" => {
                let runtime = StubRuntime {
                    models: vec![
                        model("azure-openai-responses", "dup-model"),
                        model("openai-codex", "dup-model"),
                    ],
                    ..StubRuntime::default()
                };
                run(&runtime, None, Some("dup-model"), None)
            }
            "ambiguous_bare_id_one_auth" => {
                let runtime = StubRuntime {
                    models: vec![
                        model("azure-openai-responses", "dup-model"),
                        model("openai-codex", "dup-model"),
                    ],
                    ..StubRuntime::default()
                }
                .with_auth(&["openai-codex"]);
                run(&runtime, None, Some("dup-model"), None)
            }
            "authenticated_raw_beats_unauth_inferred" => {
                let mut runtime = StubRuntime::default();
                let mut commandcode = model("commandcode", "xiaomi/mimo-v2.5-pro");
                commandcode.name = "Xiaomi MiMo via Commandcode".to_string();
                let mut xiaomi = model("xiaomi", "mimo-v2.5-pro");
                xiaomi.name = "Xiaomi MiMo".to_string();
                xiaomi.base_url = "https://api.xiaomimimo.com".to_string();
                runtime.models.push(commandcode);
                runtime.models.push(xiaomi);
                run(
                    &runtime.with_auth(&["commandcode"]),
                    None,
                    Some("xiaomi/mimo-v2.5-pro"),
                    None,
                )
            }
            "fallback_strips_thinking" => {
                fallback_case(None, Some("neuralwatt/zai-org/GLM-5.1-FP8:high"))
            }
            "fallback_no_suffix" => fallback_case(None, Some("neuralwatt/zai-org/GLM-5.1-FP8")),
            "fallback_invalid_suffix" => {
                fallback_case(None, Some("neuralwatt/zai-org/GLM-5.1-FP8:banana"))
            }
            "explicit_provider_fallback" => {
                fallback_case(Some("neuralwatt"), Some("zai-org/GLM-5.1-FP8:high"))
            }
            "explicit_thinking_keeps_suffix" => fallback_case_with_thinking(
                Some("neuralwatt"),
                Some("zai-org/GLM-5.1-FP8:high"),
                ThinkingLevel::Medium,
            ),
            "unknown_model_error" => run(
                &StubRuntime::default(),
                None,
                Some("openai/o3-missing"),
                None,
            ),
            "unknown_model_no_provider" => {
                run(&StubRuntime::default(), None, Some("o3-missing"), None)
            }
            other => panic!("unhandled oracle case {other}"),
        };
        assert_eq!(actual, expected, "cli case {name}");
    }
}

/// The neuralwatt fallback fixtures (a provider whose specific model id is
/// not in the registry triggers `buildFallbackModel`).
fn fallback_runtime() -> StubRuntime {
    let mut runtime = StubRuntime::default();
    let mut base = model("neuralwatt", "some-base-model");
    base.name = "Some Base Model".to_string();
    base.base_url = "https://api.neuralwatt.com".to_string();
    runtime.models.push(base);
    runtime
}

fn fallback_case(provider: Option<&str>, model: Option<&str>) -> serde_json::Value {
    cli_result_json(&resolve_cli_model(ResolveCliModelOptions {
        cli_provider: provider,
        cli_model: model,
        cli_thinking: None,
        model_runtime: &fallback_runtime(),
    }))
}

fn fallback_case_with_thinking(
    provider: Option<&str>,
    model: Option<&str>,
    thinking: ThinkingLevel,
) -> serde_json::Value {
    cli_result_json(&resolve_cli_model(ResolveCliModelOptions {
        cli_provider: provider,
        cli_model: model,
        cli_thinking: Some(thinking),
        model_runtime: &fallback_runtime(),
    }))
}

/// All valid thinking levels through the fallback path (oracle
/// `fallbackLevels`).
#[test]
fn fallback_levels_match_the_upstream_oracle() {
    let oracle = oracle();
    for case in oracle["fallbackLevels"].as_array().unwrap() {
        let level = case["level"].as_str().unwrap();
        let result = resolve_cli_model(ResolveCliModelOptions {
            cli_provider: None,
            cli_model: Some(&format!("neuralwatt/zai-org/GLM-5.1-FP8:{level}")),
            cli_thinking: None,
            model_runtime: &fallback_runtime(),
        });
        let actual = cli_result_json(&result);
        let expected = serde_json::json!({
            "model": case["model"],
            "reasoning": case["reasoning"],
            "thinkingLevel": case["thinkingLevel"],
            "warning": case["warning"],
            "error": case["error"],
        });
        assert_eq!(actual, expected, "fallback level {level}");
    }
}

/// The upstream `findInitialModel` battery.
#[tokio::test]
async fn find_initial_model_matches_the_upstream_oracle() {
    use crate::ai::auth::types::AuthOperationOptions;

    let oracle = oracle();
    let initial_json = |result: &crate::coding_agent::core::model_resolver::InitialModelResult| {
        serde_json::json!({
            "model": result.model.as_ref().map(reference),
            "thinkingLevel": level_string(result.thinking_level),
            "fallbackMessage": result.fallback_message,
        })
    };
    #[allow(clippy::too_many_arguments)]
    async fn run(
        runtime: &StubRuntime,
        scoped: Vec<ScopedModel>,
        continuing: bool,
        default_provider: Option<&str>,
        default_model_id: Option<&str>,
        default_thinking: Option<ThinkingLevel>,
        per_model: &[(&str, ThinkingLevel)],
    ) -> serde_json::Value {
        let result = find_initial_model(FindInitialModelOptions {
            cli_provider: None,
            cli_model: None,
            scoped_models: scoped,
            is_continuing: continuing,
            default_provider,
            default_model_id,
            default_thinking_level: default_thinking,
            model_thinking_levels: per_model
                .iter()
                .map(|(key, level)| (key.to_string(), *level))
                .collect(),
            model_runtime: runtime,
        })
        .await
        .unwrap();
        serde_json::json!({
            "model": result.model.as_ref().map(reference),
            "thinkingLevel": level_string(result.thinking_level),
            "fallbackMessage": result.fallback_message,
        })
    }

    let models = all_models();
    let cases = oracle["findInitialModel"].as_array().unwrap();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let actual = match name {
            "cli_provider_model" => {
                let runtime = StubRuntime::default();
                let result = find_initial_model(FindInitialModelOptions {
                    cli_provider: Some("openrouter"),
                    cli_model: Some("openrouter/openai/ghost-model"),
                    scoped_models: Vec::new(),
                    is_continuing: false,
                    default_provider: None,
                    default_model_id: None,
                    default_thinking_level: None,
                    model_thinking_levels: HashMap::new(),
                    model_runtime: &runtime,
                })
                .await
                .unwrap();
                initial_json(&result)
            }
            "scoped_first" => {
                let runtime = StubRuntime::default();
                run(
                    &runtime,
                    vec![ScopedModel {
                        model: models[0].clone(),
                        thinking_level: Some(ThinkingLevel::High),
                    }],
                    false,
                    None,
                    None,
                    None,
                    &[],
                )
                .await
            }
            "scoped_without_level_uses_default" => {
                let runtime = StubRuntime::default();
                run(
                    &runtime,
                    vec![ScopedModel {
                        model: models[1].clone(),
                        thinking_level: None,
                    }],
                    false,
                    None,
                    None,
                    Some(ThinkingLevel::Low),
                    &[],
                )
                .await
            }
            "per_model_level" => {
                let runtime = StubRuntime::default();
                run(
                    &runtime,
                    vec![ScopedModel {
                        model: models[1].clone(),
                        thinking_level: None,
                    }],
                    false,
                    None,
                    None,
                    None,
                    &[("openai/gpt-4o", ThinkingLevel::Xhigh)],
                )
                .await
            }
            "saved_default_authenticated" => {
                let runtime = StubRuntime::default().with_auth(&["anthropic"]);
                run(
                    &runtime,
                    Vec::new(),
                    false,
                    Some("anthropic"),
                    Some("claude-sonnet-4-5"),
                    None,
                    &[],
                )
                .await
            }
            "saved_default_unauthenticated" => {
                let mut local = model("spark-two", "deepseek-v4-flash");
                local.name = "Local".to_string();
                let runtime = StubRuntime {
                    available: vec![local],
                    ..StubRuntime::default()
                };
                run(
                    &runtime,
                    Vec::new(),
                    false,
                    Some("anthropic"),
                    Some("claude-sonnet-4-5"),
                    None,
                    &[],
                )
                .await
            }
            "available_default_match" => {
                let mut runtime = StubRuntime::default();
                let mut gateway = model("vercel-ai-gateway", "anthropic/claude-opus-4-6");
                gateway.name = "Claude Opus 4.6".to_string();
                gateway.reasoning = true;
                gateway.base_url = "https://ai-gateway.vercel.sh".to_string();
                runtime.available = vec![gateway];
                run(&runtime, Vec::new(), false, None, None, None, &[]).await
            }
            "available_fallback_first" => {
                let runtime = StubRuntime {
                    available: vec![model("spark-two", "local-1"), model("spark-two", "local-2")],
                    ..StubRuntime::default()
                };
                run(&runtime, Vec::new(), false, None, None, None, &[]).await
            }
            "continuing_scoped_skipped" => {
                let runtime = StubRuntime {
                    available: vec![model("spark-two", "local")],
                    ..StubRuntime::default()
                };
                run(
                    &runtime,
                    vec![ScopedModel {
                        model: models[0].clone(),
                        thinking_level: None,
                    }],
                    true,
                    None,
                    None,
                    None,
                    &[],
                )
                .await
            }
            "nothing" => {
                let runtime = StubRuntime::default();
                run(&runtime, Vec::new(), false, None, None, None, &[]).await
            }
            other => panic!("unhandled oracle case {other}"),
        };
        let expected = serde_json::json!({
            "model": case["model"],
            "thinkingLevel": case["thinkingLevel"],
            "fallbackMessage": case["fallbackMessage"],
        });
        assert_eq!(actual, expected, "initial case {name}");
    }
    // The AuthOperationOptions import witnesses the runtime-surface seam
    // (upstream getAvailable takes options; the stub ignores them).
    let _ = AuthOperationOptions::NONE;
}

/// The upstream `restoreModelFromSession` battery.
#[tokio::test]
async fn restore_model_from_session_matches_the_upstream_oracle() {
    let oracle = oracle();
    let models = all_models();
    let cases = oracle["restoreModelFromSession"].as_array().unwrap();
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let runtime = match name {
            "restored_with_auth" => StubRuntime::default().with_auth(&["anthropic"]),
            "restored_without_auth_falls_back_to_current" => StubRuntime::default(),
            "missing_falls_back_to_default" => StubRuntime {
                available: vec![model("spark-two", "local")],
                ..StubRuntime::default()
            },
            "missing_no_available" => StubRuntime::default(),
            other => panic!("unhandled oracle case {other}"),
        };
        let current = match name {
            "restored_without_auth_falls_back_to_current" => Some(&models[1]),
            _ => None,
        };
        let (saved_provider, saved_model_id) = match name {
            "missing_falls_back_to_default" | "missing_no_available" => ("gone", "nope"),
            _ => ("anthropic", "claude-sonnet-4-5"),
        };
        let (model, fallback_message) = restore_model_from_session(
            saved_provider,
            saved_model_id,
            current,
            false,
            &runtime,
            None,
        )
        .await;
        let actual = serde_json::json!({
            "model": model.as_ref().map(reference),
            "fallbackMessage": fallback_message,
        });
        let expected = serde_json::json!({
            "model": case["model"],
            "fallbackMessage": case["fallbackMessage"],
        });
        assert_eq!(actual, expected, "restore case {name}");
    }
}

/// Alias detection drives the partial-match preference (dated vs alias).
#[test]
fn alias_preference_prefers_latest_alias_and_dated_fallback() {
    let models = vec![
        model("anthropic", "claude-sonnet-4-5-20250929"),
        model("anthropic", "claude-sonnet-4-5-latest"),
        model("anthropic", "claude-sonnet-4-5-20241022"),
    ];
    // The `-latest` alias wins over dated versions.
    let result = parse_model_pattern("claude-sonnet", &models, None);
    assert_eq!(
        result.model.as_ref().unwrap().id,
        "claude-sonnet-4-5-latest"
    );
    // Reverse-lexicographic pick among dated versions when no alias exists.
    let dated = vec![
        model("anthropic", "claude-sonnet-4-5-20250929"),
        model("anthropic", "claude-sonnet-4-5-20241022"),
    ];
    let result = parse_model_pattern("claude-sonnet", &dated, None);
    assert_eq!(
        result.model.as_ref().unwrap().id,
        "claude-sonnet-4-5-20250929"
    );
}

/// Exact canonical/bare reference matching, including ambiguity rejection.
#[test]
fn exact_reference_matching_rejects_ambiguous_bare_ids() {
    let models = vec![model("a", "m"), model("b", "m"), model("a", "other")];
    assert!(find_exact_model_reference_match("a/m", &models).is_some());
    assert!(find_exact_model_reference_match("A/M", &models).is_some());
    // Ambiguous bare id → None.
    assert!(find_exact_model_reference_match("m", &models).is_none());
    // Unique bare id → Some.
    assert_eq!(
        find_exact_model_reference_match("other", &models)
            .unwrap()
            .id,
        "other"
    );
    // Empty/whitespace input → None.
    assert!(find_exact_model_reference_match("  ", &models).is_none());
    assert!(models_are_equal(&model("a", "m"), &model("a", "m")));
    assert!(!models_are_equal(&model("a", "m"), &model("b", "m")));
}

/// A duplicate-model guard scenario: `resolveModelScopeFromModels` avoids
/// duplicate scoped entries (upstream `modelsAreEqual` dedup).
#[test]
fn scoped_models_deduplicate_by_provider_and_id() {
    let models = vec![model("openrouter", "openai/gpt-4o:extended")];
    let result = resolve_model_scope_from_models(
        &[
            "openrouter/openai/gpt-4o:extended".to_string(),
            "openai/gpt-4o:extended".to_string(),
        ],
        &models,
    );
    assert_eq!(result.scoped_models.len(), 1);
    assert!(result.diagnostics.is_empty());
}
