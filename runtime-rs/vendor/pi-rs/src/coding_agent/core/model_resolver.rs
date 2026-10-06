//! Port of upstream `coding-agent/src/core/model-resolver.ts`: model
//! resolution, scoping, and initial selection.
//!
//! Behavior is pinned against the real upstream module under node in
//! `tests/fixtures/core_oracle_model/model_resolver.oracle.json` (generator
//! `oracle_model_resolver.mjs`): the `defaultModelPerProvider` table,
//! `parseModelPattern` (normal + strict CLI mode), glob and non-glob
//! scoping with diagnostics, the `resolveCliModel` battery, fallback
//! thinking-level handling, `findInitialModel`, and
//! `restoreModelFromSession` over the upstream test fixtures.
//!
//! Seams:
//! - **ModelRuntime** — the resolver only reads `getModels`,
//!   `getAvailableSnapshot`, `getModel`, `hasConfiguredAuth`, and the async
//!   `getAvailable`; the port abstracts those behind the
//!   [`ModelRuntimeReads`] trait, which
//!   [`ModelRuntime`](super::model_runtime::ModelRuntime) implements through
//!   the [`PrefetchedRuntime`] adapter (the runtime's model list is read
//!   once, then the pure resolution bodies run synchronously — the same
//!   shape as the upstream tests' structural stubs).
//! - **minimatch** (glob patterns in `resolveModelScopeFromModels`,
//!   `{ nocase: true }`): no glob crate is available offline, so the
//!   [`minimatch`] module vendors a minimatch-faithful matcher covering the
//!   option set upstream uses — `*`, `?`, character classes (`[…]`,
//!   negation, ranges, literal `]` first), `**` path spans, and `{a,b}`
//!   brace expansion — case-insensitively. Extended globs (`+(…)`, `@(...)`)
//!   are upstream-off by default and stay literals. Behavior is pinned by
//!   the oracle's glob scenarios (npm-bundled minimatch 10.2.4 ran the
//!   capture; upstream pins 10.2.6 — patch-level drift, disclosed).
//! - **chalk** — upstream warns through `chalk.yellow`; the port emits the
//!   same plain text on stderr (upstream chalk renders without color on
//!   non-TTYs) through injectable hooks for tests.
//! - **`isValidThinkingLevel`** (cli/args.ts) — vendored: the canonical
//!   level list. The resolver's `ThinkingLevel` is the ai-layer
//!   `ModelThinkingLevel` (the `pi-agent-core` union, which includes
//!   `"off"`).
//! - **`process.exit(1)`** in `findInitialModel`'s CLI-error path: the port
//!   returns [`FindInitialModelError`] instead of terminating the process
//!   (library-code seam).

use futures::future::BoxFuture;

use crate::ai::auth::types::AuthOperationOptions;
use crate::ai::types::Model;

/// The resolver's thinking-level union (`pi-agent-core`'s `ThinkingLevel`;
/// the ai-layer `ModelThinkingLevel` carries the same literals).
pub use crate::ai::types::primitives::ModelThinkingLevel as ThinkingLevel;

/// Upstream `DEFAULT_THINKING_LEVEL` (`defaults.ts`, `"medium"`).
const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;

/// The canonical thinking levels (`cli/args.ts` `VALID_THINKING_LEVELS`).
const VALID_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Upstream `isValidThinkingLevel` (vendored from cli/args.ts).
pub fn is_valid_thinking_level(level: &str) -> bool {
    VALID_THINKING_LEVELS.contains(&level)
}

fn parse_thinking_level(level: &str) -> ThinkingLevel {
    serde_json::from_value(serde_json::Value::String(level.to_string()))
        .unwrap_or(ThinkingLevel::Off)
}

/// Upstream `defaultModelPerProvider` (insertion order preserved — the
/// fallback loops iterate `Object.keys` order).
pub const DEFAULT_MODEL_PER_PROVIDER: &[(&str, &str)] = &[
    ("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
    ("ant-ling", "Ring-2.6-1T"),
    ("anthropic", "claude-opus-4-8"),
    ("openai", "gpt-5.5"),
    ("azure-openai-responses", "gpt-5.4"),
    ("openai-codex", "gpt-5.5"),
    ("radius", "balanced"),
    ("nvidia", "nvidia/nemotron-3-ultra-550b-a55b"),
    ("deepseek", "deepseek-v4-pro"),
    ("google", "gemini-3.1-pro-preview"),
    ("google-vertex", "gemini-3.1-pro-preview"),
    ("github-copilot", "gpt-5.4"),
    ("openrouter", "moonshotai/kimi-k2.6"),
    ("vercel-ai-gateway", "zai/glm-5.1"),
    ("xai", "grok-4.6"),
    ("groq", "openai/gpt-oss-120b"),
    ("cerebras", "gpt-oss-120b"),
    ("zai", "glm-5.3"),
    ("zai-coding-cn", "glm-5.3"),
    ("mistral", "devstral-medium-latest"),
    ("minimax", "MiniMax-M2.7"),
    ("minimax-cn", "MiniMax-M2.7"),
    ("moonshotai", "kimi-k2.6"),
    ("moonshotai-cn", "kimi-k2.6"),
    ("huggingface", "moonshotai/Kimi-K2.6"),
    ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
    ("together", "moonshotai/Kimi-K2.6"),
    ("baseten", "zai-org/GLM-5.2"),
    ("opencode", "kimi-k2.6"),
    ("opencode-go", "kimi-k2.6"),
    ("kimi-coding", "kimi-for-coding"),
    ("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
    (
        "cloudflare-ai-gateway",
        "workers-ai/@cf/moonshotai/kimi-k2.6",
    ),
    ("qwen-token-plan", "qwen3.7-max"),
    ("qwen-token-plan-cn", "qwen3.7-max"),
    ("qwen-token-plan-individual", "qwen3.8-max"),
    ("xiaomi", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-sgp", "mimo-v2.5-pro"),
];

/// Upstream `defaultModelPerProvider[provider]`.
pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
    DEFAULT_MODEL_PER_PROVIDER
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, model)| *model)
}

/// Upstream `ScopedModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    /// Thinking level if explicitly specified in the pattern
    /// (e.g. `model:high`), `None` otherwise.
    pub thinking_level: Option<ThinkingLevel>,
}

/// Upstream `ParsedModelResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
}

/// Upstream `ParseModelPattern` options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseModelPatternOptions {
    pub allow_invalid_thinking_level_fallback: bool,
}

/// Upstream `ModelScopeDiagnostic`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelScopeDiagnostic {
    pub code: ModelScopeDiagnosticCode,
    pub message: String,
    pub pattern: String,
}

/// Upstream `ModelScopeDiagnostic["code"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelScopeDiagnosticCode {
    NoMatch,
    InvalidThinkingLevel,
}

impl ModelScopeDiagnosticCode {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelScopeDiagnosticCode::NoMatch => "no-match",
            ModelScopeDiagnosticCode::InvalidThinkingLevel => "invalid-thinking-level",
        }
    }
}

/// Upstream `ResolveModelScopeResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolveModelScopeResult {
    pub scoped_models: Vec<ScopedModel>,
    pub diagnostics: Vec<ModelScopeDiagnostic>,
}

/// Upstream `ResolveCliModelResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolveCliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
    /// Error message suitable for CLI display. When set, `model` is `None`.
    pub error: Option<String>,
}

/// Upstream `InitialModelResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub fallback_message: Option<String>,
}

/// The `console.warn`/`console.log` channel (upstream writes chalk-colored
/// lines to the console). Injectable so tests can capture output.
pub type ConsoleLog = Arc<dyn Fn(&str) + Send + Sync>;

use std::sync::Arc;

/// The `ModelRuntime` surface the resolver reads (upstream passes the
/// runtime; the structural subset is abstracted for object safety and test
/// stubs). Async selection may run inside a runtime replacement factory,
/// so borrowed reads must remain safe to send with that factory's future.
pub trait ModelRuntimeReads: Send + Sync {
    fn get_models(&self) -> Vec<Model>;
    fn has_configured_auth(&self, provider_id: &str) -> bool;
    fn get_available_snapshot(&self) -> Vec<Model>;
    fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model>;
    fn get_available<'a>(
        &'a self,
        options: Option<&AuthOperationOptions>,
    ) -> BoxFuture<'a, Vec<Model>>;
}

/// A [`ModelRuntimeReads`] built once over the real
/// [`ModelRuntime`](super::model_runtime::ModelRuntime)'s lists (the port's
/// adapter from the async collection to the resolver's sync surface).
pub struct PrefetchedRuntime {
    pub models: Vec<Model>,
    pub available: Vec<Model>,
    pub configured_auth: std::collections::HashSet<String>,
    pub model_lookup: std::collections::HashMap<(String, String), Model>,
}

impl ModelRuntimeReads for PrefetchedRuntime {
    fn get_models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn has_configured_auth(&self, provider_id: &str) -> bool {
        self.configured_auth.contains(provider_id)
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        self.available.clone()
    }

    fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        self.model_lookup
            .get(&(provider_id.to_string(), model_id.to_string()))
            .cloned()
    }

    fn get_available<'a>(
        &'a self,
        _options: Option<&AuthOperationOptions>,
    ) -> BoxFuture<'a, Vec<Model>> {
        Box::pin(async move { self.get_available_snapshot() })
    }
}

/// JS `a.localeCompare(b)` ordering (plain lexicographic for the model ids
/// and provider references under test).
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

/// Upstream `modelsAreEqual` (pi-ai): id + provider identity.
pub fn models_are_equal(a: &Model, b: &Model) -> bool {
    a.id == b.id && a.provider == b.provider
}

// ---------------------------------------------------------------------------
// Reference matching
// ---------------------------------------------------------------------------

/// Upstream `isAlias`: aliases lack an 8-digit date suffix; `-latest` is
/// always an alias.
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    // `!/-\d{8}$/.test(id)` — ids shorter than the pattern are aliases.
    let bytes = id.as_bytes();
    !(bytes.len() >= 9
        && bytes[bytes.len() - 8..].iter().all(|b| b.is_ascii_digit())
        && bytes[bytes.len() - 9] == b'-')
}

/// Upstream `findExactModelReferenceMatch`: bare ids or canonical
/// `provider/modelId` references; ambiguous bare-id matches are rejected.
pub fn find_exact_model_reference_match(
    model_reference: &str,
    available_models: &[Model],
) -> Option<Model> {
    let trimmed_reference = model_reference.trim();
    if trimmed_reference.is_empty() {
        return None;
    }

    let normalized_reference = trimmed_reference.to_lowercase();

    let canonical_matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| {
            format!("{}/{}", model.provider, model.id).to_lowercase() == normalized_reference
        })
        .collect();
    if canonical_matches.len() == 1 {
        return Some(canonical_matches[0].clone());
    }
    if canonical_matches.len() > 1 {
        return None;
    }

    if let Some(slash_index) = trimmed_reference.find('/') {
        let provider = trimmed_reference[..slash_index].trim();
        let model_id = trimmed_reference[slash_index + 1..].trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let provider_matches: Vec<&Model> = available_models
                .iter()
                .filter(|model| {
                    model.provider.to_lowercase() == provider.to_lowercase()
                        && model.id.to_lowercase() == model_id.to_lowercase()
                })
                .collect();
            if provider_matches.len() == 1 {
                return Some(provider_matches[0].clone());
            }
            if provider_matches.len() > 1 {
                return None;
            }
        }
    }

    let id_matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| model.id.to_lowercase() == normalized_reference)
        .collect();
    if id_matches.len() == 1 {
        Some(id_matches[0].clone())
    } else {
        None
    }
}

/// Upstream `tryMatchModel`: exact first, then partial id/name matching with
/// alias preference (multiple aliases pick the one that sorts highest).
fn try_match_model(model_pattern: &str, available_models: &[Model]) -> Option<Model> {
    if let Some(exact_match) = find_exact_model_reference_match(model_pattern, available_models) {
        return Some(exact_match);
    }

    let lower_pattern = model_pattern.to_lowercase();
    let matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| {
            model.id.to_lowercase().contains(&lower_pattern)
                || model.name.to_lowercase().contains(&lower_pattern)
        })
        .collect();

    if matches.is_empty() {
        return None;
    }

    let mut aliases: Vec<&Model> = matches
        .iter()
        .filter(|m| is_alias(&m.id))
        .copied()
        .collect();
    if !aliases.is_empty() {
        aliases.sort_by(|a, b| locale_compare(&b.id, &a.id));
        return Some(aliases[0].clone());
    }
    let mut dated_versions: Vec<&Model> = matches
        .iter()
        .filter(|m| !is_alias(&m.id))
        .copied()
        .collect();
    dated_versions.sort_by(|a, b| locale_compare(&b.id, &a.id));
    dated_versions.first().map(|model| (*model).clone())
}

/// Upstream `parseModelPattern`.
pub fn parse_model_pattern(
    pattern: &str,
    available_models: &[Model],
    options: Option<ParseModelPatternOptions>,
) -> ParsedModelResult {
    if let Some(exact_match) = try_match_model(pattern, available_models) {
        return ParsedModelResult {
            model: Some(exact_match),
            thinking_level: None,
            warning: None,
        };
    }

    let Some(last_colon_index) = pattern.rfind(':') else {
        return ParsedModelResult::default();
    };

    let prefix = &pattern[..last_colon_index];
    let suffix = &pattern[last_colon_index + 1..];

    if is_valid_thinking_level(suffix) {
        let result = parse_model_pattern(prefix, available_models, options);
        if result.model.is_some() {
            return ParsedModelResult {
                model: result.model,
                // Only use this level when the inner recursion did not warn.
                thinking_level: if result.warning.is_some() {
                    None
                } else {
                    Some(parse_thinking_level(suffix))
                },
                warning: result.warning,
            };
        }
        result
    } else {
        let allow_fallback = options
            .map(|options| options.allow_invalid_thinking_level_fallback)
            .unwrap_or(true);
        if !allow_fallback {
            // Strict mode (CLI --model parsing): treat it as part of the
            // model id and fail.
            return ParsedModelResult::default();
        }

        let result = parse_model_pattern(prefix, available_models, options);
        if result.model.is_some() {
            return ParsedModelResult {
                model: result.model,
                thinking_level: None,
                warning: Some(format!(
                    "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
                )),
            };
        }
        result
    }
}

/// Upstream `buildFallbackModel`.
fn build_fallback_model(
    provider: &str,
    model_id: &str,
    available_models: &[Model],
) -> Option<Model> {
    let provider_models: Vec<&Model> = available_models
        .iter()
        .filter(|model| model.provider == provider)
        .collect();
    if provider_models.is_empty() {
        return None;
    }

    let default_id = default_model_per_provider(provider);
    let base_model = default_id
        .and_then(|default_id| provider_models.iter().find(|model| model.id == default_id))
        .or_else(|| provider_models.first())
        .expect("provider models are non-empty");

    let mut model = (*base_model).clone();
    model.id = model_id.to_string();
    model.name = model_id.to_string();
    Some(model)
}

/// Upstream `resolveModelScopeFromModels`.
pub fn resolve_model_scope_from_models(
    patterns: &[String],
    models: &[Model],
) -> ResolveModelScopeResult {
    let available_models = models.to_vec();
    let mut result = ResolveModelScopeResult::default();

    for pattern in patterns {
        // Glob patterns take the minimatch path.
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let colon_idx = pattern.rfind(':');
            let mut glob_pattern = pattern.clone();
            let mut thinking_level: Option<ThinkingLevel> = None;

            if let Some(colon_idx) = colon_idx {
                let suffix = &pattern[colon_idx + 1..];
                if is_valid_thinking_level(suffix) {
                    thinking_level = Some(parse_thinking_level(suffix));
                    glob_pattern = pattern[..colon_idx].to_string();
                }
            }

            if let Some(exact_match) =
                find_exact_model_reference_match(&glob_pattern, &available_models)
            {
                if !result
                    .scoped_models
                    .iter()
                    .any(|scoped| models_are_equal(&scoped.model, &exact_match))
                {
                    result.scoped_models.push(ScopedModel {
                        model: exact_match,
                        thinking_level,
                    });
                }
                continue;
            }

            let matching_models: Vec<Model> = available_models
                .iter()
                .filter(|model| {
                    let full_id = format!("{}/{}", model.provider, model.id);
                    minimatch::matches(&full_id, &glob_pattern)
                        || minimatch::matches(&model.id, &glob_pattern)
                })
                .cloned()
                .collect();

            if matching_models.is_empty() {
                result.diagnostics.push(ModelScopeDiagnostic {
                    code: ModelScopeDiagnosticCode::NoMatch,
                    message: format!("No models match pattern \"{pattern}\""),
                    pattern: pattern.clone(),
                });
                continue;
            }

            for model in matching_models {
                if !result
                    .scoped_models
                    .iter()
                    .any(|scoped| models_are_equal(&scoped.model, &model))
                {
                    result.scoped_models.push(ScopedModel {
                        model,
                        thinking_level,
                    });
                }
            }
            continue;
        }

        let parsed = parse_model_pattern(pattern, &available_models, None);

        if let Some(warning) = &parsed.warning {
            result.diagnostics.push(ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::InvalidThinkingLevel,
                message: warning.clone(),
                pattern: pattern.clone(),
            });
        }

        let Some(model) = parsed.model else {
            result.diagnostics.push(ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::NoMatch,
                message: format!("No models match pattern \"{pattern}\""),
                pattern: pattern.clone(),
            });
            continue;
        };

        if !result
            .scoped_models
            .iter()
            .any(|scoped| models_are_equal(&scoped.model, &model))
        {
            result.scoped_models.push(ScopedModel {
                model,
                thinking_level: parsed.thinking_level,
            });
        }
    }

    result
}

/// Upstream `resolveModelScopeWithDiagnostics`.
pub async fn resolve_model_scope_with_diagnostics(
    patterns: &[String],
    model_runtime: &dyn ModelRuntimeReads,
) -> ResolveModelScopeResult {
    let available = model_runtime.get_available(None).await;
    resolve_model_scope_from_models(patterns, &available)
}

/// Upstream `resolveModelScope` (warns per diagnostic through the console).
pub async fn resolve_model_scope(
    patterns: &[String],
    model_runtime: &dyn ModelRuntimeReads,
    warn: Option<&ConsoleLog>,
) -> Vec<ScopedModel> {
    let result = resolve_model_scope_with_diagnostics(patterns, model_runtime).await;
    for diagnostic in &result.diagnostics {
        let line = format!("Warning: {}", diagnostic.message);
        match warn {
            Some(warn) => warn(&line),
            None => eprintln!("{line}"),
        }
    }
    result.scoped_models
}

// ---------------------------------------------------------------------------
// CLI resolution
// ---------------------------------------------------------------------------

/// Upstream `resolveCliModel` options.
pub struct ResolveCliModelOptions<'a> {
    pub cli_provider: Option<&'a str>,
    pub cli_model: Option<&'a str>,
    pub cli_thinking: Option<ThinkingLevel>,
    pub model_runtime: &'a dyn ModelRuntimeReads,
}

/// Upstream `resolveCliModel`.
pub fn resolve_cli_model(options: ResolveCliModelOptions<'_>) -> ResolveCliModelResult {
    let ResolveCliModelOptions {
        cli_provider,
        cli_model,
        cli_thinking,
        model_runtime,
    } = options;

    let Some(cli_model) = cli_model else {
        return ResolveCliModelResult::default();
    };

    // Use *all* models here, not just models with pre-configured auth —
    // this allows `--api-key` for first-time setup.
    let available_models = model_runtime.get_models();
    if available_models.is_empty() {
        return ResolveCliModelResult {
            model: None,
            thinking_level: None,
            warning: None,
            error: Some(
                "No models available. Check your installation or add models to models.json."
                    .to_string(),
            ),
        };
    }

    // Canonical provider lookup (case-insensitive).
    let mut provider_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for model in &available_models {
        provider_map.insert(model.provider.to_lowercase(), model.provider.clone());
    }

    let mut provider = cli_provider.and_then(|p| provider_map.get(&p.to_lowercase()).cloned());
    if let Some(cli_provider) = cli_provider {
        if provider.is_none() {
            return ResolveCliModelResult {
                model: None,
                thinking_level: None,
                warning: None,
                error: Some(format!(
                    "Unknown provider \"{cli_provider}\". Use --list-models to see available providers/models."
                )),
            };
        }
    }

    // If no explicit --provider, interpret "provider/model" first: when the
    // prefix before the first slash matches a known provider, prefer that
    // over matching ids that literally contain slashes.
    let mut pattern = cli_model.to_string();
    let mut inferred_provider = false;

    if provider.is_none() {
        if let Some(slash_index) = cli_model.find('/') {
            let maybe_provider = &cli_model[..slash_index];
            if let Some(canonical) = provider_map.get(&maybe_provider.to_lowercase()) {
                provider = Some(canonical.clone());
                pattern = cli_model[slash_index + 1..].to_string();
                inferred_provider = true;
            }
        }
    }

    // Exact matches without provider inference (ids that naturally contain
    // slashes). Bare exact ids can exist in multiple providers; prefer the
    // sole authenticated provider, otherwise require an explicit provider.
    if provider.is_none() {
        let lower = cli_model.to_lowercase();
        let exact_matches: Vec<&Model> = available_models
            .iter()
            .filter(|model| {
                model.id.to_lowercase() == lower
                    || format!("{}/{}", model.provider, model.id).to_lowercase() == lower
            })
            .collect();
        if exact_matches.len() == 1 {
            return ResolveCliModelResult {
                model: Some(exact_matches[0].clone()),
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
        if exact_matches.len() > 1 {
            let authenticated_exact_matches: Vec<&Model> = exact_matches
                .iter()
                .filter(|model| model_runtime.has_configured_auth(&model.provider))
                .copied()
                .collect();
            if authenticated_exact_matches.len() == 1 {
                return ResolveCliModelResult {
                    model: Some(authenticated_exact_matches[0].clone()),
                    thinking_level: None,
                    warning: None,
                    error: None,
                };
            }

            let mut references: Vec<String> = exact_matches
                .iter()
                .map(|model| format!("{}/{}", model.provider, model.id))
                .collect();
            references.sort_by(|a, b| locale_compare(a, b));
            let auth_hint = if authenticated_exact_matches.is_empty() {
                "No matching provider is authenticated."
            } else {
                "More than one matching provider is authenticated."
            };
            return ResolveCliModelResult {
                model: None,
                thinking_level: None,
                warning: None,
                error: Some(format!(
                    "Model \"{cli_model}\" is ambiguous across providers: {}. {auth_hint} Use --provider or provider/model.",
                    references.join(", ")
                )),
            };
        }
    }

    if cli_provider.is_some() {
        if let Some(provider) = &provider {
            // Tolerate --model <provider>/<pattern> by stripping the prefix
            // (both were provided).
            let prefix = format!("{provider}/");
            if cli_model.to_lowercase().starts_with(&prefix.to_lowercase()) {
                pattern = cli_model[prefix.len()..].to_string();
            }
        }
    }

    let candidates: Vec<Model> = match &provider {
        Some(provider) => available_models
            .iter()
            .filter(|model| &model.provider == provider)
            .cloned()
            .collect(),
        None => available_models.clone(),
    };
    let parsed = parse_model_pattern(
        &pattern,
        &candidates,
        Some(ParseModelPatternOptions {
            allow_invalid_thinking_level_fallback: false,
        }),
    );

    if let Some(model) = &parsed.model {
        // Prefer one exact raw model-id match that is authenticated when the
        // inference picked an unauthenticated provider/model pair (ids whose
        // literal id starts with a known provider name).
        if inferred_provider {
            let raw_exact_matches: Vec<&Model> = available_models
                .iter()
                .filter(|candidate| {
                    candidate.id.to_lowercase() == cli_model.to_lowercase()
                        && !models_are_equal(candidate, model)
                })
                .collect();
            if !raw_exact_matches.is_empty() && !model_runtime.has_configured_auth(&model.provider)
            {
                let authenticated_raw_matches: Vec<&Model> = raw_exact_matches
                    .iter()
                    .filter(|model| model_runtime.has_configured_auth(&model.provider))
                    .copied()
                    .collect();
                if authenticated_raw_matches.len() == 1 {
                    return ResolveCliModelResult {
                        model: Some(authenticated_raw_matches[0].clone()),
                        thinking_level: None,
                        warning: None,
                        error: None,
                    };
                }
            }
        }
        return ResolveCliModelResult {
            model: Some(model.clone()),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    // Fall back to matching the full input as a raw model id across all
    // models (OpenRouter-style ids like "openai/gpt-4o:extended").
    if inferred_provider {
        let lower = cli_model.to_lowercase();
        if let Some(exact) = available_models.iter().find(|model| {
            model.id.to_lowercase() == lower
                || format!("{}/{}", model.provider, model.id).to_lowercase() == lower
        }) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                thinking_level: None,
                warning: None,
                error: None,
            };
        }
        let fallback = parse_model_pattern(
            cli_model,
            &available_models,
            Some(ParseModelPatternOptions {
                allow_invalid_thinking_level_fallback: false,
            }),
        );
        if let Some(fallback_model) = fallback.model {
            return ResolveCliModelResult {
                model: Some(fallback_model),
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(provider) = &provider {
        // Parse the thinking-level suffix before building the fallback model,
        // but only when --thinking is not explicitly provided.
        let mut fallback_pattern = pattern.clone();
        let mut fallback_thinking: Option<ThinkingLevel> = None;
        if cli_thinking.is_none() {
            if let Some(last_colon) = pattern.rfind(':') {
                let suffix = &pattern[last_colon + 1..];
                if is_valid_thinking_level(suffix) {
                    fallback_pattern = pattern[..last_colon].to_string();
                    fallback_thinking = Some(parse_thinking_level(suffix));
                }
            }
        }

        if let Some(mut fallback_model) =
            build_fallback_model(provider, &fallback_pattern, &available_models)
        {
            let requested_thinking = cli_thinking.or(fallback_thinking);
            if let Some(thinking) = requested_thinking {
                if thinking != ThinkingLevel::Off {
                    fallback_model.reasoning = true;
                }
            }
            let warning = match &parsed.warning {
                Some(parsed_warning) => format!(
                    "{parsed_warning} Model \"{fallback_pattern}\" not found for provider \"{provider}\". Using custom model id."
                ),
                None => format!(
                    "Model \"{fallback_pattern}\" not found for provider \"{provider}\". Using custom model id."
                ),
            };
            return ResolveCliModelResult {
                model: Some(fallback_model),
                thinking_level: fallback_thinking,
                warning: Some(warning),
                error: None,
            };
        }
    }

    let display = match &provider {
        Some(provider) => format!("{provider}/{pattern}"),
        None => cli_model.to_string(),
    };
    ResolveCliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

// ---------------------------------------------------------------------------
// Initial selection and session restore
// ---------------------------------------------------------------------------

/// Upstream `findInitialModel` options.
pub struct FindInitialModelOptions<'a> {
    pub cli_provider: Option<&'a str>,
    pub cli_model: Option<&'a str>,
    pub scoped_models: Vec<ScopedModel>,
    pub is_continuing: bool,
    pub default_provider: Option<&'a str>,
    pub default_model_id: Option<&'a str>,
    pub default_thinking_level: Option<ThinkingLevel>,
    pub model_thinking_levels: std::collections::HashMap<String, ThinkingLevel>,
    pub model_runtime: &'a dyn ModelRuntimeReads,
}

/// The CLI-error exit of upstream `findInitialModel` (`process.exit(1)` —
/// the port surfaces it instead of terminating the process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindInitialModelError(pub String);

impl std::fmt::Display for FindInitialModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FindInitialModelError {}

/// Upstream `findInitialModel`.
pub async fn find_initial_model(
    options: FindInitialModelOptions<'_>,
) -> Result<InitialModelResult, FindInitialModelError> {
    let FindInitialModelOptions {
        cli_provider,
        cli_model,
        scoped_models,
        is_continuing,
        default_provider,
        default_model_id,
        default_thinking_level,
        model_thinking_levels,
        model_runtime,
    } = options;

    // 1. CLI args take priority.
    if let (Some(cli_provider), Some(cli_model)) = (cli_provider, cli_model) {
        let resolved = resolve_cli_model(ResolveCliModelOptions {
            cli_provider: Some(cli_provider),
            cli_model: Some(cli_model),
            cli_thinking: None,
            model_runtime,
        });
        if let Some(error) = resolved.error {
            return Err(FindInitialModelError(error));
        }
        if let Some(model) = resolved.model {
            return Ok(InitialModelResult {
                model: Some(model),
                thinking_level: DEFAULT_THINKING_LEVEL,
                fallback_message: None,
            });
        }
    }

    // 2. First model from scoped models (skipped when continuing).
    if !scoped_models.is_empty() && !is_continuing {
        let scoped_model = &scoped_models[0];
        let per_model = model_thinking_levels
            .get(&format!(
                "{}/{}",
                scoped_model.model.provider, scoped_model.model.id
            ))
            .copied();
        return Ok(InitialModelResult {
            model: Some(scoped_model.model.clone()),
            thinking_level: scoped_model
                .thinking_level
                .or(per_model)
                .or(default_thinking_level)
                .unwrap_or(DEFAULT_THINKING_LEVEL),
            fallback_message: None,
        });
    }

    // 3. Saved default from settings when auth is configured.
    if let (Some(default_provider), Some(default_model_id)) = (default_provider, default_model_id) {
        if let Some(found) = model_runtime.get_model(default_provider, default_model_id) {
            if model_runtime.has_configured_auth(&found.provider) {
                let key = format!("{default_provider}/{default_model_id}");
                let thinking_level = model_thinking_levels
                    .get(&key)
                    .copied()
                    .or(default_thinking_level)
                    .unwrap_or(DEFAULT_THINKING_LEVEL);
                return Ok(InitialModelResult {
                    model: Some(found),
                    thinking_level,
                    fallback_message: None,
                });
            }
        }
    }

    // 4. First available model with a valid API key.
    let available_models = model_runtime.get_available_snapshot();
    if !available_models.is_empty() {
        // Try to find a default model from known providers (keys order).
        for (provider, default_id) in DEFAULT_MODEL_PER_PROVIDER {
            if let Some(matched) = available_models
                .iter()
                .find(|model| model.provider == *provider && model.id == *default_id)
            {
                return Ok(InitialModelResult {
                    model: Some(matched.clone()),
                    thinking_level: DEFAULT_THINKING_LEVEL,
                    fallback_message: None,
                });
            }
        }
        return Ok(InitialModelResult {
            model: Some(available_models[0].clone()),
            thinking_level: DEFAULT_THINKING_LEVEL,
            fallback_message: None,
        });
    }

    // 5. No model found.
    Ok(InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL,
        fallback_message: None,
    })
}

/// Upstream `restoreModelFromSession`.
pub async fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    should_print_messages: bool,
    model_runtime: &dyn ModelRuntimeReads,
    log: Option<&ConsoleLog>,
) -> (Option<Model>, Option<String>) {
    let restored_model = model_runtime.get_model(saved_provider, saved_model_id);
    let has_configured_auth = restored_model
        .as_ref()
        .is_some_and(|model| model_runtime.has_configured_auth(&model.provider));

    if let (Some(restored_model), true) = (&restored_model, has_configured_auth) {
        if should_print_messages {
            let line = format!("Restored model: {saved_provider}/{saved_model_id}");
            match log {
                Some(log) => log(&line),
                None => println!("{line}"),
            }
        }
        return (Some(restored_model.clone()), None);
    }

    // Model not found or no API key — fall back.
    let reason = if restored_model.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };

    if should_print_messages {
        let line = format!(
            "Warning: Could not restore model {saved_provider}/{saved_model_id} ({reason})."
        );
        match log {
            Some(log) => log(&line),
            None => eprintln!("{line}"),
        }
    }

    if let Some(current_model) = current_model {
        if should_print_messages {
            let line = format!(
                "Falling back to: {}/{}",
                current_model.provider, current_model.id
            );
            match log {
                Some(log) => log(&line),
                None => println!("{line}"),
            }
        }
        return (
            Some(current_model.clone()),
            Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                current_model.provider, current_model.id
            )),
        );
    }

    let available_models = model_runtime.get_available_snapshot();
    if !available_models.is_empty() {
        let mut fallback_model: Option<Model> = None;
        for (provider, default_id) in DEFAULT_MODEL_PER_PROVIDER {
            if let Some(matched) = available_models
                .iter()
                .find(|model| model.provider == *provider && model.id == *default_id)
            {
                fallback_model = Some(matched.clone());
                break;
            }
        }
        let fallback_model = fallback_model.unwrap_or_else(|| available_models[0].clone());
        if should_print_messages {
            let line = format!(
                "Falling back to: {}/{}",
                fallback_model.provider, fallback_model.id
            );
            match log {
                Some(log) => log(&line),
                None => println!("{line}"),
            }
        }
        return (
            Some(fallback_model.clone()),
            Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                fallback_model.provider, fallback_model.id
            )),
        );
    }

    (None, None)
}

// ---------------------------------------------------------------------------
// minimatch (vendored, `nocase` option set)
// ---------------------------------------------------------------------------

/// A minimatch-faithful glob matcher for the option set upstream uses
/// (`{ nocase: true }`, defaults otherwise). See the module docs.
pub(crate) mod minimatch {
    /// Case-insensitive match (`{ nocase: true }`).
    pub fn matches(text: &str, pattern: &str) -> bool {
        for pattern in expand_braces(pattern) {
            if match_pattern(&text.to_lowercase(), &pattern.to_lowercase()) {
                return true;
            }
        }
        false
    }

    /// `{a,b}` brace expansion (non-nested; a `{` without a matching `}`
    /// stays literal, like minimatch's default-off brace handling would
    /// leave the pattern unmatched-literal).
    fn expand_braces(pattern: &str) -> Vec<String> {
        let Some(open) = pattern.find('{') else {
            return vec![pattern.to_string()];
        };
        let Some(close) = pattern[open..].find('}').map(|offset| offset + open) else {
            return vec![pattern.to_string()];
        };
        let (prefix, body, suffix) = (
            &pattern[..open],
            &pattern[open + 1..close],
            &pattern[close + 1..],
        );
        let mut out = Vec::new();
        for option in body.split(',') {
            out.extend(expand_braces(&format!("{prefix}{option}{suffix}")));
        }
        out
    }

    /// Split on `/` — `**` spans segments; every other segment is a
    /// single-segment glob.
    fn match_pattern(text: &str, pattern: &str) -> bool {
        let path: Vec<&str> = text.split('/').collect();
        let pattern_parts: Vec<&str> = pattern.split('/').collect();
        match_segments(&path, &pattern_parts)
    }

    fn match_segments(text: &[&str], pattern: &[&str]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((&"**", rest)) => {
                // `**` matches zero or more path segments.
                for skip in 0..=text.len() {
                    if match_segments(&text[skip..], rest) {
                        return true;
                    }
                }
                false
            }
            Some((first, rest)) => {
                let Some((head, tail)) = text.split_first() else {
                    return false;
                };
                match_segment(first, head) && match_segments(tail, rest)
            }
        }
    }

    /// Single-segment glob match (`*`, `?`, `[…]` classes; `\` escapes).
    fn match_segment(pattern: &str, text: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let text: Vec<char> = text.chars().collect();
        glob_chars(&pattern, 0, &text, 0)
    }

    fn glob_chars(pattern: &[char], mut p: usize, text: &[char], mut t: usize) -> bool {
        while p < pattern.len() {
            match pattern[p] {
                '*' => {
                    let mut next = p;
                    while next < pattern.len() && pattern[next] == '*' {
                        next += 1;
                    }
                    if next == pattern.len() {
                        return true;
                    }
                    for skip in t..=text.len() {
                        if glob_chars(pattern, next, text, skip) {
                            return true;
                        }
                    }
                    return false;
                }
                '?' => {
                    if t >= text.len() {
                        return false;
                    }
                    p += 1;
                    t += 1;
                }
                '[' => {
                    if t >= text.len() {
                        return false;
                    }
                    let (matched, next) = match_class(pattern, p, text[t]);
                    if !matched {
                        return false;
                    }
                    p = next;
                    t += 1;
                }
                '\\' if p + 1 < pattern.len() => {
                    if t >= text.len() || text[t] != pattern[p + 1] {
                        return false;
                    }
                    p += 2;
                    t += 1;
                }
                literal => {
                    if t >= text.len() || text[t] != literal {
                        return false;
                    }
                    p += 1;
                    t += 1;
                }
            }
        }
        t == text.len()
    }

    /// Parse a `[…]` class starting at `pattern[start]` (the `[`). Returns
    /// whether `c` matches and the index just past the class. A leading `!`
    /// negates; a leading `]` is literal; `a-z` ranges.
    fn match_class(pattern: &[char], start: usize, c: char) -> (bool, usize) {
        let mut i = start + 1;
        let negate = pattern.get(i) == Some(&'!');
        if negate {
            i += 1;
        }
        let mut matched = false;
        let mut first = true;
        while i < pattern.len() {
            if pattern[i] == ']' && !first {
                return (matched != negate, i + 1);
            }
            first = false;
            let low = if pattern[i] == '\\' && i + 1 < pattern.len() {
                i += 1;
                pattern[i]
            } else {
                pattern[i]
            };
            if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|&c| c != ']') {
                let high = if pattern[i + 3] == '\\' {
                    pattern[i + 4]
                } else {
                    pattern[i + 3]
                };
                if c >= low && c <= high {
                    matched = true;
                }
                i += if pattern[i + 3] == '\\' { 5 } else { 4 };
            } else {
                if c == low {
                    matched = true;
                }
                i += 1;
            }
        }
        // Unterminated class: minimatch treats it as a literal `[`.
        (c == '[', start + 1)
    }
}

#[cfg(test)]
#[path = "model_resolver_tests.rs"]
mod tests;
