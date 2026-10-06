//! Upstream `packages/ai/src/api/typesafe-system-one.ts`: TypeSafe's native
//! System One protocol. OpenRouter serves the same protocol, so both
//! providers use this API with different base URLs.

use serde_json::{json, Value};

use super::system_one_shared::{
    classify_system_one, is_record, SystemOneTransport, SystemOneWireRequest,
};
use crate::ai::types::classifier::{ClassifierContext, ClassifierOptions, ClassifierResult};
use crate::ai::types::model::ClassifierModel;

/// Upstream transport (`typesafe-system-one.ts:9-18`): the `systemone`
/// endpoint on the model's base URL with the flat
/// `{ model, ...request }` envelope (`{ model, state, questions }`) and the
/// response as the System One output itself.
struct TypeSafeSystemOneTransport;

impl SystemOneTransport for TypeSafeSystemOneTransport {
    fn api(&self) -> &'static str {
        "typesafe-system-one"
    }

    fn label(&self) -> &'static str {
        "System One API"
    }

    fn url(&self, model: &ClassifierModel) -> String {
        // Upstream `new URL("systemone", `${baseUrl.replace(/\/+$/, "")}/`)`.
        format!("{}/systemone", model.base_url.trim_end_matches('/'))
    }

    fn payload(&self, model: &ClassifierModel, request: &SystemOneWireRequest) -> Value {
        // Upstream `{ model: model.id, ...request }`.
        json!({
            "model": model.id,
            "state": request.state,
            "questions": request.questions,
        })
    }

    fn output(&self, body: &Value) -> Result<Value, String> {
        if !is_record(body) {
            return Err("System One API returned an unexpected response".to_string());
        }
        Ok(body.clone())
    }
}

/// Upstream `classify` (typesafe-system-one.ts:21-23): TypeSafe System One
/// classification with public `bool` values mapped to wire-level `noul`
/// (the mapping lives in [`super::system_one_shared::wire_request`]).
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
    classify_system_one(&TypeSafeSystemOneTransport, model, context, options).await
}

/// The [`crate::ai::models::provider::ClassifierApiImpl`] adapter the
/// `Models.classify` routing dispatches to (upstream
/// `classifiers: { "typesafe-system-one": typesafeSystemOneApi() }`).
pub struct TypeSafeSystemOneApi;

impl crate::ai::models::provider::ClassifierApiImpl for TypeSafeSystemOneApi {
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
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        Box::pin(async move { Ok(classify(&model, &context, Some(&options)).await) })
    }
}
