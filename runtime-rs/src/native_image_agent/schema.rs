use super::{invalid, Result};
use crate::native_workflow::{image_protocol, inputs};
use serde_json::{json, Value};
use std::sync::OnceLock;

pub(crate) fn schema() -> Result<Value> {
    serde_json::from_str(include_str!("tool-schema.json")).map_err(|_| invalid())
}
pub(crate) struct Arguments {
    pub(crate) operation: String,
    pub(crate) source_image: Option<String>,
    pub(crate) source: Option<Value>,
    pub(crate) images: Vec<Value>,
    pub(crate) settings: Value,
    pub(crate) prompt: String,
    pub(crate) model: Value,
    pub(crate) edits: Option<Value>,
}
pub(crate) fn parse(value: &Value) -> Result<Arguments> {
    static VALIDATOR: OnceLock<std::result::Result<jsonschema::Validator, String>> =
        OnceLock::new();
    let validator = VALIDATOR.get_or_init(|| {
        schema().map_err(|error| error.code).and_then(|schema| {
            jsonschema::validator_for(&schema).map_err(|error| error.to_string())
        })
    });
    if !validator.as_ref().map_err(|_| invalid())?.is_valid(value)
        || (value.get("sourceImage").is_some() && value.get("source").is_some())
        || (value.get("edits").is_some() && value["operation"] != "edit")
    {
        return Err(invalid());
    }
    let requested = value["model"].as_str().unwrap_or("").trim();
    let model = if requested.is_empty() {
        Value::Null
    } else {
        let (provider, model) = requested.split_once('/').ok_or_else(invalid)?;
        if provider.is_empty() || model.is_empty() || requested.contains(['\r', '\n']) {
            return Err(invalid());
        }
        json!({"provider":provider,"model":model})
    };
    let settings = image_protocol::settings(value.get("settings"))?;
    let edits = if value["operation"] == "edit" {
        image_protocol::frame_edits(&value["edits"])?;
        Some(value["edits"].clone())
    } else {
        None
    };
    let images = match value.get("images") {
        Some(images) => {
            image_protocol::output(&json!({"type":"workflow-images","version":1,"frames":images}))?
                ["frames"]
                .as_array()
                .cloned()
                .ok_or_else(invalid)?
        }
        None => Vec::new(),
    };
    let source = value.get("source").map(inputs::media).transpose()?;
    Ok(Arguments {
        operation: value["operation"].as_str().ok_or_else(invalid)?.into(),
        source_image: value["sourceImage"].as_str().map(str::to_owned),
        source,
        images,
        settings,
        prompt: value["prompt"].as_str().unwrap_or("").into(),
        model,
        edits,
    })
}
