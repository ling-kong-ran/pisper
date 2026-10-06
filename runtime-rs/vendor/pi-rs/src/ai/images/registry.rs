//! The image-side registries, ported from three upstream modules:
//!
//! - `packages/ai/src/images-api-registry.ts`: the process-wide
//!   `ImagesApiProvider` registry keyed by api id. Upstream wraps every
//!   registered `generateImages` so a model whose `api` does not match the
//!   registry key throws `"Mismatched api: ..."`; the port's wrapper returns
//!   that message through the [`ImagesApiFn`] error channel (the upstream
//!   synchronous-throw channel — providers map every `Err` to an
//!   [`AssistantImages`] error result, exactly like upstream's total
//!   try/catch in `Models.generateImages`).
//! - `packages/ai/src/images.ts`: the [`generate_images`] entry point plus
//!   the `providers/images/register-builtins.ts` import side effect, which
//!   the port models as [`ensure_builtin_images_apis_registered`] (Rust has
//!   no import side effects).
//! - `packages/ai/src/image-models.ts`: the static image-model catalog
//!   compat reads. Since #9948 upstream reads `IMAGE_MODELS` from the
//!   unified generated catalog (`models.generated.ts`, flattened
//!   `type: "image"` entries of `providers/data/*.json`); the port flattens
//!   the same embedded shards (`assets/model-data/`) — empty in this
//!   snapshot, whose data predates the image/classifier entry format (see
//!   `models::catalog`). Upstream's type-level narrowing erases to plain
//!   string lookups, like every other generic in the port.
//!
//! Also ported here is `parseOpenRouterImageModels`
//! (`scripts/generate-image-models.ts:36-90`, deleted upstream with the
//! generated image catalog it fed), the catalog generator's OpenRouter API
//! parser (oracle `image-model-data.test.ts`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::ai::types::images::{AssistantImages, ImagesContext, ImagesOptions};
use crate::ai::types::model::{ImageModel, ModelInput, ModelType};

use super::openrouter_images;

/// Upstream `ImagesApiFunction` (images-api-registry.ts:3-7) with the
/// upstream throw channel made explicit as `Err`: registry guards
/// (`Mismatched api`) and api-fn failures alike. The owning provider maps
/// `Err` to an [`AssistantImages`] error result — upstream's total try/catch
/// in `ImagesModels.generateImages` (images-models.ts:213-223) turns any
/// throw into the same shape.
pub type ImagesApiFn = Arc<
    dyn Fn(
            ImageModel,
            ImagesContext,
            Option<ImagesOptions>,
        ) -> BoxFuture<'static, Result<AssistantImages, String>>
        + Send
        + Sync,
>;

/// Upstream `RegisteredImagesApiProvider` (images-api-registry.ts:19-22).
struct RegisteredImagesApiProvider {
    generate_images: ImagesApiFn,
    #[allow(dead_code)]
    source_id: Option<String>,
}

/// Upstream `imagesApiProviderRegistry` (images-api-registry.ts:24): one
/// process-wide registry. A sorted map (deterministic iteration); access is
/// lock-per-call and no await runs under the lock (handlers are cloned out).
fn images_api_registry() -> &'static Mutex<BTreeMap<String, RegisteredImagesApiProvider>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, RegisteredImagesApiProvider>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock<T>(guard: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    guard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Upstream `wrapGenerateImages` (images-api-registry.ts:26-36): guard the
/// model's api against the registry key, then dispatch.
fn wrap_generate_images(api: &str, generate_images: ImagesApiFn) -> ImagesApiFn {
    let api = api.to_string();
    Arc::new(move |model, context, options| {
        let generate_images = Arc::clone(&generate_images);
        let api = api.clone();
        Box::pin(async move {
            if model.api != api {
                return Err(format!("Mismatched api: {} expected {}", model.api, api));
            }
            generate_images(model, context, options).await
        })
    })
}

/// Upstream `registerImagesApiProvider` (images-api-registry.ts:38-49).
pub fn register_images_api_provider(
    api: &str,
    generate_images: ImagesApiFn,
    source_id: Option<&str>,
) {
    lock(images_api_registry()).insert(
        api.to_string(),
        RegisteredImagesApiProvider {
            generate_images: wrap_generate_images(api, generate_images),
            source_id: source_id.map(str::to_string),
        },
    );
}

/// Upstream `getImagesApiProvider` (images-api-registry.ts:51-53): the
/// api-checked handler for one registry key, if registered.
pub fn get_images_api_provider(api: &str) -> Option<ImagesApiFn> {
    lock(images_api_registry())
        .get(api)
        .map(|registered| Arc::clone(&registered.generate_images))
}

/// Upstream `providers/images/register-builtins.ts` (import side effect of
/// `images.ts`): the built-in openrouter-images handler registers itself.
/// Called at the top of [`generate_images`]; exposed for tests.
pub fn ensure_builtin_images_apis_registered() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        register_images_api_provider(
            openrouter_images::OPENROUTER_IMAGES_API,
            openrouter_images::images_api_fn(),
            None,
        );
    });
}

/// Upstream `generateImages` (images.ts:14-21): dispatch to the registered
/// handler for the model's api. Upstream throws when unregistered; the port
/// returns the same message through the `Err` channel.
pub async fn generate_images(
    model: ImageModel,
    context: ImagesContext,
    options: Option<ImagesOptions>,
) -> Result<AssistantImages, String> {
    ensure_builtin_images_apis_registered();
    let handler = get_images_api_provider(&model.api)
        .ok_or_else(|| format!("No API provider registered for api: {}", model.api))?;
    handler(model, context, options).await
}

// ---------------------------------------------------------------------------
// Static image-model catalog (image-models.ts over the unified generated data)
// ---------------------------------------------------------------------------

/// Upstream `imageModelsByProvider` (image-models.ts): the one-time build of
/// the per-provider image-model lookup maps from `IMAGE_MODELS` — here the
/// `type: "image"` entries of the embedded unified shards. Providers with no
/// image models are omitted (upstream `if (imageModels.size > 0)`).
fn image_model_registry() -> &'static BTreeMap<String, BTreeMap<String, ImageModel>> {
    static REGISTRY: OnceLock<BTreeMap<String, BTreeMap<String, ImageModel>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry: BTreeMap<String, BTreeMap<String, ImageModel>> = BTreeMap::new();
        for provider in crate::ai::models::catalog::catalog_provider_ids() {
            let models = crate::ai::models::catalog::embedded_provider_image_catalog(provider);
            if models.is_empty() {
                continue;
            }
            registry.insert(
                provider.to_string(),
                models
                    .into_iter()
                    .map(|model| (model.id.clone(), model))
                    .collect(),
            );
        }
        registry
    })
}

/// Upstream `getImageModel` (image-models.ts): one static catalog entry,
/// `None` when the provider or model id is unknown (upstream returns
/// `undefined` typed as always-present — the port makes the miss honest).
pub fn get_image_model(provider: &str, model_id: &str) -> Option<ImageModel> {
    image_model_registry()
        .get(provider)
        .and_then(|models| models.get(model_id))
        .cloned()
}

/// Upstream `getImageProviders` (image-models.ts).
pub fn get_image_providers() -> Vec<String> {
    image_model_registry().keys().cloned().collect()
}

/// Upstream `getImageModels` (image-models.ts): every catalog entry of
/// one provider, empty when unknown.
pub fn get_image_models(provider: &str) -> Vec<ImageModel> {
    image_model_registry()
        .get(provider)
        .map(|models| models.values().cloned().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// parseOpenRouterImageModels (scripts/generate-image-models.ts:36-90)
// ---------------------------------------------------------------------------

/// Upstream `OPENROUTER_BASE_URL` (scripts/generate-image-models.ts:8).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// JS `parseFloat`: leading numeric prefix of a string, NaN as `f64::NAN`.
/// The pricing strings are well-formed, but `|| "0"` coercion semantics
/// (`""`/missing -> 0) and prefix tolerance are ported faithfully anyway.
fn js_parse_float(text: &str) -> f64 {
    let trimmed = text.trim_start();
    let mut end = 0usize;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let bytes = trimmed.as_bytes();
    if bytes.first().is_some_and(|&b| b == b'+' || b == b'-') {
        end = 1;
    }
    while let Some(&b) = bytes.get(end) {
        match b {
            b'0'..=b'9' => {
                seen_digit = true;
                end += 1;
            }
            b'.' if !seen_dot => {
                seen_dot = true;
                end += 1;
            }
            // Exponent part: JS `parseFloat("1.5e")` stops before the `e`,
            // `parseFloat("1.5e-2")` consumes it, `parseFloat("1.5e-")`
            // stops before the `e`.
            b'e' | b'E' if seen_digit => match bytes.get(end + 1) {
                Some(b'+' | b'-') if bytes.get(end + 2).is_some_and(|b| b.is_ascii_digit()) => {
                    end += 2;
                }
                Some(&b) if b.is_ascii_digit() => end += 1,
                _ => break,
            },
            _ => break,
        }
    }
    if !seen_digit {
        return f64::NAN;
    }
    trimmed[..end].parse().unwrap_or(f64::NAN)
}

/// Upstream `model.pricing?.X || "0"`: JS falsy coercion — `""`/missing
/// parse to `NaN`, and a falsy value falls back to `"0"`. Any non-finite
/// parse is therefore zero.
fn pricing_value(pricing: Option<&Value>, field: &str) -> f64 {
    let raw = pricing
        .and_then(|pricing| pricing.get(field))
        .and_then(Value::as_str)
        .unwrap_or("0");
    let parsed = js_parse_float(raw);
    if parsed.is_finite() {
        parsed
    } else {
        0.0
    }
}

/// Upstream `parseOpenRouterImageModels` (generate-image-models.ts:36-90):
/// reduce an OpenRouter `/models` payload to image models. `strict` mirrors
/// the generator's CLI mode: missing/empty data and zero usable models are
/// errors instead of empty results. The upstream `throw` channel maps to
/// `Err` carrying the same messages.
pub fn parse_open_router_image_models(
    payload: &Value,
    strict: bool,
) -> Result<Vec<ImageModel>, String> {
    let data = payload.get("data").and_then(Value::as_array);
    let Some(data) = data else {
        return if strict {
            Err("OpenRouter API returned a missing or empty image model list".to_string())
        } else {
            Ok(Vec::new())
        };
    };
    if data.is_empty() {
        return if strict {
            Err("OpenRouter API returned a missing or empty image model list".to_string())
        } else {
            Ok(Vec::new())
        };
    }

    let mut models = Vec::new();
    for entry in data {
        let architecture = entry.get("architecture");
        let mut input = modalities_of(architecture, "input_modalities");
        let output = modalities_of(architecture, "output_modalities");
        if !output.contains(&ModelInput::Image) {
            continue;
        }
        if input.is_empty() {
            input.push(ModelInput::Text);
        }
        let pricing = entry.get("pricing");
        models.push(ImageModel {
            id: str_field(entry, "id"),
            name: str_field(entry, "name"),
            api: openrouter_images::OPENROUTER_IMAGES_API.to_string(),
            provider: "openrouter".to_string(),
            base_url: OPENROUTER_BASE_URL.to_string(),
            input,
            input_limits: None,
            cost: crate::ai::types::primitives::ModelCost {
                input: pricing_value(pricing, "prompt") * 1_000_000.0,
                output: pricing_value(pricing, "completion") * 1_000_000.0,
                cache_read: pricing_value(pricing, "input_cache_read") * 1_000_000.0,
                cache_write: pricing_value(pricing, "input_cache_write") * 1_000_000.0,
                tiers: None,
            },
            headers: None,
            r#type: ModelType::Image,
            output,
        });
    }

    if strict && models.is_empty() {
        return Err("OpenRouter API returned no usable image models".to_string());
    }
    Ok(models)
}

/// Upstream `Array.from(new Set((model.architecture?.X ?? []).filter(...)))`:
/// keep only `"text"`/`"image"`, deduplicated in first-appearance order.
fn modalities_of(architecture: Option<&Value>, field: &str) -> Vec<ModelInput> {
    let mut modalities: Vec<ModelInput> = Vec::new();
    for value in architecture
        .and_then(|architecture| architecture.get(field))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let modality = match value.as_str() {
            Some("text") => ModelInput::Text,
            Some("image") => ModelInput::Image,
            _ => continue,
        };
        if !modalities.contains(&modality) {
            modalities.push(modality);
        }
    }
    modalities
}

/// `model.X` as a string, empty when absent (the catalog entries always
/// carry both fields; the port keeps the deserialization total).
fn str_field(entry: &Value, field: &str) -> String {
    entry
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::model::ModelInput;

    /// The unified generated catalog carries no `type: "image"` entries in
    /// this snapshot (the embedded 2026-09-21 data predates the image/
    /// classifier entry format), so the compat reads serve an empty registry,
    /// exactly like upstream running against the same data.
    #[test]
    fn image_catalog_serves_the_unified_generated_shards() {
        assert_eq!(get_image_providers(), Vec::<String>::new());
        assert!(get_image_models("openrouter").is_empty());
        assert!(get_image_model("openrouter", "nope").is_none());
    }

    // Oracle image-model-data.test.ts, "rejects a missing or empty strict
    // catalog" (it.each([{}, { data: [] }, { data: "invalid" }])).
    #[test]
    fn parser_rejects_missing_or_empty_strict_catalog() {
        for payload in [
            serde_json::json!({}),
            serde_json::json!({"data": []}),
            serde_json::json!({"data": "invalid"}),
        ] {
            let error = parse_open_router_image_models(&payload, true).unwrap_err();
            assert_eq!(
                error,
                "OpenRouter API returned a missing or empty image model list"
            );
            // Non-strict mode returns an empty list instead.
            assert!(parse_open_router_image_models(&payload, false)
                .unwrap()
                .is_empty());
        }
    }

    /// Oracle: "rejects a strict catalog with no usable image models".
    #[test]
    fn parser_rejects_strict_catalog_with_no_usable_models() {
        let payload = serde_json::json!({
            "data": [{
                "id": "example/text-only",
                "name": "Text Only",
                "architecture": {
                    "input_modalities": ["text"],
                    "output_modalities": ["text"],
                },
            }],
        });
        let error = parse_open_router_image_models(&payload, true).unwrap_err();
        assert_eq!(error, "OpenRouter API returned no usable image models");
    }

    /// Oracle: "parses a non-empty image model catalog".
    #[test]
    fn parser_parses_a_non_empty_catalog() {
        let payload = serde_json::json!({
            "data": [{
                "id": "example/image-model",
                "name": "Example Image Model",
                "architecture": {
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["image"],
                },
                "pricing": {
                    "prompt": "0.000001",
                    "completion": "0.000002",
                },
            }],
        });
        let models = parse_open_router_image_models(&payload, true).unwrap();
        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.id, "example/image-model");
        assert_eq!(model.input, vec![ModelInput::Text, ModelInput::Image]);
        assert_eq!(model.output, vec![ModelInput::Image]);
        assert_eq!(model.api, "openrouter-images");
        assert_eq!(model.provider, "openrouter");
        assert_eq!(model.base_url, OPENROUTER_BASE_URL);
        // Per-million rates scale the string pricing.
        assert_eq!(model.cost.input, 1.0);
        assert_eq!(model.cost.output, 2.0);
        assert_eq!(model.cost.cache_read, 0.0);
        assert_eq!(model.cost.cache_write, 0.0);

        // Empty input modalities default to ["text"].
        let payload = serde_json::json!({
            "data": [{
                "id": "example/inputless",
                "name": "Inputless",
                "architecture": {"output_modalities": ["image", "text", "image"]},
            }],
        });
        let models = parse_open_router_image_models(&payload, true).unwrap();
        assert_eq!(models[0].input, vec![ModelInput::Text]);
        // Deduplicated output in first-appearance order.
        assert_eq!(models[0].output, vec![ModelInput::Image, ModelInput::Text]);
    }

    /// Non-image entries (missing output modality) are skipped, not errors.
    #[test]
    fn parser_skips_non_image_entries() {
        let payload = serde_json::json!({
            "data": [
                {"id": "a", "name": "A", "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]}},
                {"id": "b", "name": "B", "architecture": {"input_modalities": ["text"], "output_modalities": ["image"]}},
                {"id": "c", "name": "C"},
            ],
        });
        let models = parse_open_router_image_models(&payload, false).unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
    }

    /// JS parseFloat port: prefix parsing and falsy fallbacks.
    #[test]
    fn pricing_coercion_matches_js() {
        assert_eq!(pricing_value(None, "prompt"), 0.0);
        assert_eq!(pricing_value(Some(&serde_json::json!({})), "prompt"), 0.0);
        assert_eq!(
            pricing_value(Some(&serde_json::json!({"prompt": ""})), "prompt"),
            0.0
        );
        assert_eq!(
            pricing_value(Some(&serde_json::json!({"prompt": "1.5e-2"})), "prompt"),
            0.015
        );
    }

    /// The registry: register/replace/get, mismatch check, and the free
    /// entry point's unregistered-api error (upstream `images.ts` throw).
    #[tokio::test]
    async fn images_api_registry_registers_and_dispatches() {
        let model = ImageModel {
            id: "m".into(),
            name: "m".into(),
            api: "custom-images".into(),
            provider: "p".into(),
            base_url: "https://example.test".into(),
            input: vec![ModelInput::Text],
            input_limits: None,
            cost: crate::ai::types::primitives::ModelCost::default(),
            headers: None,
            r#type: ModelType::Image,
            output: vec![ModelInput::Image],
        };
        let context = ImagesContext::default();
        let handler: ImagesApiFn = Arc::new(|model, _context, _options| {
            Box::pin(async move {
                Ok(AssistantImages {
                    api: model.api,
                    provider: model.provider,
                    model: model.id,
                    output: Vec::new(),
                    response_id: None,
                    usage: None,
                    stop_reason: crate::ai::types::images::ImagesStopReason::Stop,
                    error_message: None,
                    timestamp: crate::ai::now_ms(),
                })
            })
        });
        register_images_api_provider("custom-images", Arc::clone(&handler), Some("test"));

        // Dispatch through the free entry point.
        let result = generate_images(model.clone(), context.clone(), None)
            .await
            .unwrap();
        assert_eq!(result.provider, "p");

        // The registry wrapper checks the model's api against its key when
        // the handler is invoked directly with a mismatched model (upstream
        // wrapGenerateImages' synchronous throw).
        let registered = get_images_api_provider("custom-images").unwrap();
        let mut wrong = model.clone();
        wrong.api = "other-images".into();
        let error = registered(wrong, context.clone(), None).await.unwrap_err();
        assert_eq!(error, "Mismatched api: other-images expected custom-images");

        // Unregistered api: upstream "No API provider registered" throw.
        let mut unknown = model;
        unknown.api = "unregistered".into();
        let error = generate_images(unknown, context, None).await.unwrap_err();
        assert_eq!(error, "No API provider registered for api: unregistered");
    }
}
