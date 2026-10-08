use crate::native_workflow::{image_protocol, inputs, Result, WorkflowError};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::collections::HashSet;

const PROJECT_FIELDS: &[&str] = &[
    "id",
    "name",
    "prompt",
    "reference",
    "originalReference",
    "frameCount",
    "directions",
    "model",
    "actions",
];
pub(crate) fn error(code: &str, status: StatusCode) -> WorkflowError {
    let mut error = WorkflowError::coded(code, code);
    error.status = status;
    error
}
pub(crate) fn invalid() -> WorkflowError {
    error("game_assets_invalid", StatusCode::BAD_REQUEST)
}
pub(crate) fn record<'a>(
    value: &'a Value,
    fields: &[&str],
) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .filter(|object| object.keys().all(|key| fields.contains(&key.as_str())))
        .ok_or_else(invalid)
}
pub(crate) fn id(value: &str) -> Result<&str> {
    let bytes = value.as_bytes();
    if bytes.len() != 36
        || ![8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
        || !(b'1'..=b'8').contains(&bytes[14])
        || ![b'8', b'9', b'a', b'b'].contains(&bytes[19].to_ascii_lowercase())
    {
        return Err(invalid());
    }
    Ok(value)
}
fn identifier(value: &Value) -> Result<Value> {
    Ok(json!(id(value.as_str().ok_or_else(invalid)?)?))
}
fn text(value: &Value, max: usize, required: bool) -> Result<Value> {
    let text = value
        .as_str()
        .filter(|text| text.encode_utf16().count() <= max && (!required || !text.trim().is_empty()))
        .ok_or_else(invalid)?;
    Ok(json!(text.trim()))
}
fn optional_text(input: &Value, key: &str, max: usize) -> Result<Value> {
    text(
        input
            .get(key)
            .filter(|value| !value.is_null())
            .unwrap_or(&json!("")),
        max,
        false,
    )
}
fn integer(value: &Value, min: u64, max: u64) -> Result<u64> {
    value
        .as_f64()
        .filter(|value| {
            value.is_finite()
                && value.fract() == 0.0
                && *value >= min as f64
                && *value <= max as f64
        })
        .map(|value| value as u64)
        .ok_or_else(invalid)
}
fn image(value: Option<&Value>) -> Result<Value> {
    match value.filter(|value| !value.is_null()) {
        None => Ok(Value::Null),
        Some(value) => {
            let media = inputs::media(value).map_err(|_| invalid())?;
            if !media["mimeType"]
                .as_str()
                .is_some_and(|mime| mime.starts_with("image/"))
            {
                return Err(invalid());
            }
            Ok(media)
        }
    }
}
pub(crate) fn project_input(value: &Value) -> Result<Value> {
    record(value, PROJECT_FIELDS)?;
    let default_directions = json!(image_protocol::DIRECTIONS);
    let directions = value
        .get("directions")
        .filter(|value| !value.is_null())
        .unwrap_or(&default_directions)
        .as_array()
        .filter(|array| !array.is_empty() && array.len() <= 8)
        .ok_or_else(invalid)?;
    let mut seen = HashSet::new();
    for direction in directions {
        let direction = direction
            .as_str()
            .filter(|direction| image_protocol::DIRECTIONS.contains(direction))
            .ok_or_else(invalid)?;
        if !seen.insert(direction) {
            return Err(invalid());
        }
    }
    let frame_count = integer(value.get("frameCount").unwrap_or(&json!(4)), 1, 16)?;
    let empty = json!([]);
    let actions = value
        .get("actions")
        .filter(|value| !value.is_null())
        .unwrap_or(&empty)
        .as_array()
        .filter(|array| array.len() <= 8)
        .ok_or_else(invalid)?;
    let mut ids = HashSet::new();
    let mut parsed_actions = Vec::new();
    let mut enabled = 0;
    for action in actions {
        record(action, &["id", "name", "prompt", "enabled"])?;
        let action_id = action["id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 40
                    && id
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_alphanumeric())
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            })
            .ok_or_else(invalid)?;
        if !ids.insert(action_id) {
            return Err(invalid());
        }
        let is_enabled = action["enabled"].as_bool().ok_or_else(invalid)?;
        enabled += usize::from(is_enabled);
        parsed_actions.push(json!({"id":action_id,"name":text(&action["name"],100,true)?,"prompt":optional_text(action,"prompt",2000)?,"enabled":is_enabled}));
    }
    if enabled * directions.len() * frame_count as usize > 512 {
        return Err(invalid());
    }
    let model = match value.get("model").filter(|value| !value.is_null()) {
        None => Value::Null,
        Some(model) => {
            record(model, &["provider", "model"])?;
            json!({"provider":text(&model["provider"],200,true)?,"model":text(&model["model"],300,true)?})
        }
    };
    let mut parsed = json!({"name":text(&value["name"],100,true)?,"prompt":optional_text(value,"prompt",8000)?,
        "reference":image(value.get("reference"))?,"originalReference":image(value.get("originalReference"))?,
        "frameCount":frame_count,"directions":directions,"model":model,"actions":parsed_actions});
    if let Some(input_id) = value.get("id") {
        parsed["id"] = identifier(input_id)?;
    }
    Ok(parsed)
}
fn date(value: &Value) -> Result<Value> {
    let text = value.as_str().ok_or_else(invalid)?;
    let date = chrono::DateTime::parse_from_rfc3339(text).map_err(|_| invalid())?;
    if date
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        != text
    {
        return Err(invalid());
    }
    Ok(value.clone())
}
pub(crate) fn project(value: &Value) -> Result<Value> {
    let mut fields = PROJECT_FIELDS.to_vec();
    fields.extend(["createdAt", "updatedAt"]);
    record(value, &fields)?;
    let mut input = value.clone();
    input.as_object_mut().unwrap().remove("createdAt");
    input.as_object_mut().unwrap().remove("updatedAt");
    let mut parsed = project_input(&input)?;
    parsed["id"] = identifier(&value["id"])?;
    parsed["createdAt"] = date(&value["createdAt"])?;
    parsed["updatedAt"] = date(&value["updatedAt"])?;
    Ok(parsed)
}
pub(crate) fn edits(value: &Value) -> Result<Value> {
    let frames = image_protocol::frame_edits(value)?;
    Ok(
        json!({"frames":frames.into_iter().map(|frame|json!({"sourceIndex":frame.source_index,"x":frame.x,"y":frame.y,
        "rotation":frame.rotation,"scale":frame.scale,"opacity":frame.opacity,"durationMs":frame.duration_ms,
        "eraseStrokes":frame.erase_strokes.into_iter().map(|stroke|json!({"radius":stroke.radius,"restore":stroke.restore,
            "points":stroke.points.into_iter().map(|point|json!({"x":point.x,"y":point.y})).collect::<Vec<_>>()})).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
    )
}
pub(crate) fn job(value: &Value) -> Result<Value> {
    record(
        value,
        &[
            "id",
            "projectId",
            "status",
            "startedAt",
            "finishedAt",
            "completed",
            "total",
            "error",
            "output",
            "originalOutput",
            "edits",
            "revision",
        ],
    )?;
    let status = value["status"]
        .as_str()
        .filter(|status| {
            ["running", "completed", "failed", "cancelled", "interrupted"].contains(status)
        })
        .ok_or_else(invalid)?;
    let error_value = value.get("error").ok_or_else(invalid)?;
    if !error_value.is_null() {
        let code = error_value.as_str().ok_or_else(invalid)?;
        let suffix = [
            "game_assets_",
            "workflow_image_",
            "workflow_media_",
            "sprite_engine_",
        ]
        .iter()
        .find_map(|prefix| code.strip_prefix(prefix))
        .ok_or_else(invalid)?;
        if suffix.is_empty()
            || suffix.len() > 60
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            return Err(invalid());
        }
    }
    let total = integer(&value["total"], 1, 8)?;
    let completed = integer(&value["completed"], 0, total)?;
    let finished = value.get("finishedAt").ok_or_else(invalid)?;
    if (status == "running") != finished.is_null() || (status == "completed" && completed != total)
    {
        return Err(invalid());
    }
    let output = image_protocol::output(&value["output"]).map_err(|_| invalid())?;
    let original = image_protocol::output(&value["originalOutput"]).map_err(|_| invalid())?;
    let mut parsed = json!({"id":identifier(&value["id"])?,"projectId":identifier(&value["projectId"])?,"status":status,
        "startedAt":date(&value["startedAt"])?,"finishedAt":if finished.is_null(){Value::Null}else{date(finished)?},
        "completed":completed,"total":total,"error":error_value,"output":output,"originalOutput":original,
        "revision":integer(&value["revision"],0,9_007_199_254_740_991)?});
    if let Some(input) = value.get("edits") {
        let edits = edits(input).map_err(|_| invalid())?;
        let frames = edits["frames"].as_array().ok_or_else(invalid)?;
        if frames.len() != parsed["output"]["frames"].as_array().unwrap().len()
            || frames.iter().any(|frame| {
                frame["sourceIndex"].as_u64().unwrap() as usize
                    >= parsed["originalOutput"]["frames"].as_array().unwrap().len()
            })
        {
            return Err(invalid());
        }
        parsed["edits"] = edits;
    }
    Ok(parsed)
}
pub(crate) fn catalog(value: &Value) -> Result<Value> {
    record(value, &["projects", "jobs"])?;
    let projects = value["projects"]
        .as_array()
        .filter(|array| array.len() <= 100)
        .ok_or_else(invalid)?
        .iter()
        .map(project)
        .collect::<Result<Vec<_>>>()?;
    let jobs = value["jobs"]
        .as_array()
        .filter(|array| array.len() <= 100)
        .ok_or_else(invalid)?
        .iter()
        .map(job)
        .collect::<Result<Vec<_>>>()?;
    let mut project_ids = HashSet::new();
    let mut job_ids = HashSet::new();
    if projects
        .iter()
        .any(|project| !project_ids.insert(project["id"].as_str().unwrap()))
        || jobs.iter().any(|job| {
            !job_ids.insert(job["id"].as_str().unwrap())
                || !project_ids.contains(job["projectId"].as_str().unwrap())
        })
    {
        return Err(invalid());
    }
    Ok(json!({"projects":projects,"jobs":jobs}))
}
pub(crate) fn empty_output() -> Value {
    json!({"type":"workflow-images","version":1,"frames":[]})
}
pub(crate) fn safe_code(error: &WorkflowError) -> &str {
    if [
        "game_assets_cancelled",
        "game_assets_storage_failed",
        "game_assets_media_invalid",
        "workflow_image_invalid",
        "workflow_image_source_required",
        "workflow_image_too_large",
        "workflow_image_cancelled",
        "workflow_image_closed",
        "workflow_image_generation_failed",
        "workflow_image_processing_failed",
        "workflow_image_timeout",
        "workflow_media_invalid",
        "workflow_media_missing",
        "workflow_media_too_large",
        "sprite_engine_missing",
        "sprite_engine_invalid",
        "workflow_image_invalid_edits",
    ]
    .contains(&error.code.as_str())
    {
        &error.code
    } else {
        "game_assets_processing_failed"
    }
}
