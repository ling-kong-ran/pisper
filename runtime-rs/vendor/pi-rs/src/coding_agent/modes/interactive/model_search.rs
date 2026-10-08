//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/model-search.ts`
//! (21 lines, sha256 `9be620174c0f25516b0c537dcd4ef9be7c9c9a145e6489e186cc6c4787b25412`).
//!
//! Two pure search-text builders feeding fuzzy matching for the /model
//! selector. Byte-exact oracle: `tests/fixtures/interactive_r16_oracle/model_search_oracle.json`.

/// Upstream `ModelSearchItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSearchItem {
    pub id: String,
    pub provider: String,
    pub name: Option<String>,
}

/// Upstream `getModelSearchText`.
pub fn get_model_search_text(item: &ModelSearchItem) -> String {
    let ModelSearchItem { id, provider, name } = item;
    let name = name
        .as_deref()
        .map(|name| format!(" {name}"))
        .unwrap_or_default();
    format!("{id} {provider} {provider}/{id} {provider} {id}{name}")
}

/// Upstream `getModelSelectorSearchText`.
///
/// The /model selector search should rank exact provider-prefixed queries
/// before proxy-provider IDs like openrouter/openai/gpt-5, so keep the bare
/// model ID out of the leading position.
pub fn get_model_selector_search_text(item: &ModelSearchItem) -> String {
    let ModelSearchItem { id, provider, name } = item;
    let name = name
        .as_deref()
        .map(|name| format!(" {name}"))
        .unwrap_or_default();
    format!("{provider} {provider}/{id} {provider} {id}{name}")
}
