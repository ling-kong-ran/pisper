//! Upstream `packages/ai/src/api/cloudflare-workers-ai-system-one.ts`:
//! System One models on the Workers AI REST endpoint —
//! `POST /accounts/{account}/ai/run` with `{ model, input }`. The REST API
//! wraps the model output in Cloudflare's API envelope. Third-party models
//! such as `typesafe/jev` add a run record:
//! `{ success, result: { state: "Completed", result: { answers, usage } } }`.
//! https://developers.cloudflare.com/ai/models/typesafe/jev/
//! Cloudflare-hosted models such as `@cf/cloudflare/clef` return the output
//! directly: `{ success, result: { model, answers, usage } }`.
//! https://developers.cloudflare.com/workers-ai/models/clef/

use serde_json::{json, Value};

use super::system_one_shared::{
    classify_system_one, is_record, SystemOneTransport, SystemOneWireRequest,
};
use crate::ai::types::classifier::{ClassifierContext, ClassifierOptions, ClassifierResult};
use crate::ai::types::model::ClassifierModel;

const LABEL: &str = "Cloudflare Workers AI";

/// Upstream `cloudflareErrorMessage` (`cloudflare-workers-ai-system-one.ts:7-17`).
fn cloudflare_error_message(errors: &Value) -> String {
    if let Some(errors) = errors.as_array() {
        let messages: Vec<&str> = errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect();
        if !messages.is_empty() {
            return format!("{LABEL} error: {}", messages.join("; "));
        }
    }
    format!("{LABEL} request failed")
}

/// Upstream transport (`cloudflare-workers-ai-system-one.ts:27-40`): the
/// Workers AI `run` endpoint with the `{ model, input }` envelope and the
/// Cloudflare run-record unwrap.
struct CloudflareWorkersAiSystemOneTransport;

impl SystemOneTransport for CloudflareWorkersAiSystemOneTransport {
    fn api(&self) -> &'static str {
        "cloudflare-workers-ai-system-one"
    }

    fn label(&self) -> &'static str {
        LABEL
    }

    fn url(&self, model: &ClassifierModel) -> String {
        // Upstream `new URL("run", `${model.baseUrl.replace(/\/+$/u, "")}/`)`.
        format!("{}/run", model.base_url.trim_end_matches('/'))
    }

    fn payload(&self, model: &ClassifierModel, request: &SystemOneWireRequest) -> Value {
        json!({ "model": model.id, "input": { "state": request.state, "questions": request.questions } })
    }

    fn output(&self, body: &Value) -> Result<Value, String> {
        if !is_record(body) {
            return Err(format!("{LABEL} returned an unexpected response"));
        }
        if body.get("success") == Some(&json!(false)) {
            return Err(cloudflare_error_message(
                body.get("errors").unwrap_or(&Value::Null),
            ));
        }
        // v1.0.0: Cloudflare-hosted models return the output directly
        // (`result.answers`), third-party ones nest it in a run record
        // (`result.state` + `result.result`).
        let result = body.get("result").unwrap_or(&Value::Null);
        if !is_record(result) {
            return Err(format!("{LABEL} returned an unexpected response"));
        }
        if result.get("answers").is_some() {
            return Ok(result.clone());
        }
        if result.get("state").and_then(Value::as_str) != Some("Completed") {
            return Err(format!(
                "{LABEL} run did not complete (state: {})",
                result
                    .get("state")
                    .map(|state| match state {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "undefined".to_string())
            ));
        }
        let inner = result.get("result").unwrap_or(&Value::Null);
        if !is_record(inner) {
            return Err(format!("{LABEL} returned an unexpected response"));
        }
        Ok(inner.clone())
    }
}

/// Upstream `classify` (`cloudflare-workers-ai-system-one.ts:43-45`):
/// Cloudflare Workers AI System One classification with public `bool` values
/// mapped to wire-level `noul`.
pub async fn classify(
    model: &ClassifierModel,
    context: &ClassifierContext,
    options: Option<&ClassifierOptions>,
) -> ClassifierResult {
    let default_options;
    let options = match options {
        Some(options) => options,
        None => {
            default_options = ClassifierOptions::default();
            &default_options
        }
    };
    classify_system_one(
        &CloudflareWorkersAiSystemOneTransport,
        model,
        context,
        options,
    )
    .await
}

/// The [`crate::ai::models::provider::ClassifierApiImpl`] adapter the
/// `Models.classify` routing dispatches to (upstream
/// `classifiers: { "cloudflare-workers-ai-system-one":
/// cloudflareClassifier(cloudflareWorkersAISystemOneApi()) }`).
pub struct CloudflareWorkersAiSystemOneApi;

impl crate::ai::models::provider::ClassifierApiImpl for CloudflareWorkersAiSystemOneApi {
    fn classify(
        &self,
        _config: &crate::ai::ProviderConfig,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> futures::future::BoxFuture<
        'static,
        Result<ClassifierResult, crate::ai::auth::resolve::ModelsError>,
    > {
        // Upstream `cloudflareClassifier` (providers/cloudflare-stream.ts):
        // `resolveCloudflareModel(model, options?.env)` — materialize the
        // `{CLOUDFLARE_ACCOUNT_ID}`/`{CLOUDFLARE_GATEWAY_ID}` placeholders
        // from the effective provider env before the request. The port's
        // chat wrapper additionally substitutes the routed config's URL;
        // the classifier endpoints read the model's base URL, so both
        // substitutions land here (config first, env second — identical
        // values by construction, both id/placeholder materializations).
        let mut model = model.clone();
        let base_url = _config.base_url.clone();
        model.base_url = crate::ai::models::providers::cloudflare::resolve_cloudflare_base_url(
            &base_url,
            options.env.as_ref(),
        );
        let context = context.clone();
        let options = options.clone();
        Box::pin(async move { Ok(classify(&model, &context, Some(&options)).await) })
    }
}
