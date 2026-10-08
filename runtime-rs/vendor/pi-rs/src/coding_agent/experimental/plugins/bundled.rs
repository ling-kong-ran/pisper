//! Port of upstream `experimental/plugins/bundled.ts`.

use serde_json::{Map, Value};

const PRESENTATION_FACET_BUNDLES_KEY: &str = "presentationFacetBundles";

/// Upstream `createPresentationFacetData`: wrap facet bundle artifacts in the
/// single-key JSON envelope the presentation services carry.
pub fn create_presentation_facet_data(artifacts: &[Value]) -> Value {
    let mut object = Map::new();
    object.insert(
        PRESENTATION_FACET_BUNDLES_KEY.to_string(),
        Value::Array(artifacts.to_vec()),
    );
    Value::Object(object)
}

/// Upstream `createPresentationFacetLoaders`: validate the envelope and hand
/// each artifact to the embedder's artifact loader (D9 seam: chord's
/// `createFacetBundleArtifactLoader` is not portable; the embedder receives
/// the validated artifacts). Exact upstream error strings are preserved.
pub fn presentation_facet_artifacts(data: &Value) -> Result<Vec<Value>, String> {
    if !data.is_object() {
        return Err("Invalid presentation plugin data".to_string());
    }
    let Some(artifacts) = data.get(PRESENTATION_FACET_BUNDLES_KEY) else {
        return Ok(Vec::new());
    };
    let Some(artifacts) = artifacts.as_array() else {
        return Err("Invalid presentation plugin bundle list".to_string());
    };
    Ok(artifacts.clone())
}

/// Upstream `createSessionPluginFacetLoader`'s empty-manifest short circuit:
/// a manifest without a `session` entry contributes nothing. The manifest
/// read itself is embedder-owned (D9).
pub fn session_facet_loader_needed(manifest_entries_session: Option<&Value>) -> bool {
    manifest_entries_session.is_some()
}
