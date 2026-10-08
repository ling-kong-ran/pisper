//! shared/image/image-operations 的严格配置和 wire 格式，媒体执行与 ZIP 共用。
use super::{inputs, Result, WorkflowError};
use serde_json::{json, Value};
use std::collections::HashSet;
pub(crate) const KINDS: &[&str] = &[
    "media-input",
    "media-background",
    "media-inpaint",
    "media-generate",
    "media-frames",
    "media-transform",
    "media-preview",
    "media-export",
];
pub(crate) const DIRECTIONS: &[&str] = &["S", "SW", "W", "NW", "N", "NE", "E", "SE"];
pub(crate) fn invalid() -> WorkflowError {
    WorkflowError::coded("workflow_image_invalid", "workflow_image_invalid")
}
pub(crate) fn invalid_edits() -> WorkflowError {
    WorkflowError::coded(
        "workflow_image_invalid_edits",
        "workflow_image_invalid_edits",
    )
}
#[derive(Clone, Debug)]
pub(crate) struct AlphaPoint {
    pub(crate) x: f64,
    pub(crate) y: f64,
}
#[derive(Clone, Debug)]
pub(crate) struct AlphaStroke {
    pub(crate) radius: f64,
    pub(crate) restore: bool,
    pub(crate) points: Vec<AlphaPoint>,
}
#[derive(Clone, Debug)]
pub(crate) struct FrameEdit {
    pub(crate) source_index: usize,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) rotation: f64,
    pub(crate) scale: f64,
    pub(crate) opacity: f64,
    pub(crate) duration_ms: u32,
    pub(crate) erase_strokes: Vec<AlphaStroke>,
}
// 与 shared/image/image-frame-edits 一致：缺省只针对缺失字段，null 不能被当作缺省。
pub(crate) fn frame_edits(value: &Value) -> Result<Vec<FrameEdit>> {
    fn finite(value: Option<&Value>, fallback: f64, min: f64, max: f64) -> Result<f64> {
        match value {
            None if fallback.is_finite() => Ok(fallback),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= min && *n <= max)
                .ok_or_else(invalid_edits),
            _ => Err(invalid_edits()),
        }
    }
    if !value.is_object() {
        return Err(invalid_edits());
    }
    let frames = value["frames"]
        .as_array()
        .filter(|frames| !frames.is_empty() && frames.len() <= 512)
        .ok_or_else(invalid_edits)?;
    let mut point_count = 0_usize;
    let mut normalized = Vec::with_capacity(frames.len());
    for frame in frames {
        if !frame.is_object() {
            return Err(invalid_edits());
        }
        let source_index = finite(frame.get("sourceIndex"), f64::NAN, 0.0, 511.0)?;
        let duration = finite(frame.get("durationMs"), 125.0, 16.0, 10000.0)?;
        if source_index.fract() != 0.0 || duration.fract() != 0.0 {
            return Err(invalid_edits());
        }
        let empty = Vec::new();
        let strokes = match frame.get("eraseStrokes") {
            None => &empty,
            Some(value) => value
                .as_array()
                .filter(|strokes| strokes.len() <= 64)
                .ok_or_else(invalid_edits)?,
        };
        let x = finite(frame.get("x"), 0.0, -4096.0, 4096.0)?;
        let y = finite(frame.get("y"), 0.0, -4096.0, 4096.0)?;
        let rotation = finite(frame.get("rotation"), 0.0, -360.0, 360.0)?;
        let scale = finite(frame.get("scale"), 1.0, 0.05, 8.0)?;
        let opacity = finite(frame.get("opacity"), 1.0, 0.0, 1.0)?;
        let mut erase_strokes = Vec::with_capacity(strokes.len());
        for stroke in strokes {
            if !stroke.is_object()
                || stroke
                    .get("restore")
                    .is_some_and(|value| !value.is_boolean())
            {
                return Err(invalid_edits());
            }
            let points = stroke["points"]
                .as_array()
                .filter(|points| !points.is_empty() && points.len() <= 512)
                .ok_or_else(invalid_edits)?;
            point_count += points.len();
            if point_count > 20_000 {
                return Err(WorkflowError::coded(
                    "workflow_image_too_large",
                    "workflow_image_too_large",
                ));
            }
            let radius = finite(stroke.get("radius"), f64::NAN, 0.001, 0.25)?;
            let mut parsed_points = Vec::with_capacity(points.len());
            for point in points {
                if !point.is_object() {
                    return Err(invalid_edits());
                }
                parsed_points.push(AlphaPoint {
                    x: finite(point.get("x"), f64::NAN, 0.0, 1.0)?,
                    y: finite(point.get("y"), f64::NAN, 0.0, 1.0)?,
                });
            }
            erase_strokes.push(AlphaStroke {
                radius,
                restore: stroke["restore"] == true,
                points: parsed_points,
            });
        }
        normalized.push(FrameEdit {
            source_index: source_index as usize,
            x,
            y,
            rotation,
            scale,
            opacity,
            duration_ms: duration as u32,
            erase_strokes,
        });
    }
    Ok(normalized)
}
fn numeric(input: &Value, key: &str, fallback: f64, minimum: f64, maximum: f64) -> Result<f64> {
    match input.get(key) {
        None => Ok(fallback),
        Some(value) => value
            .as_f64()
            .filter(|number| number.is_finite() && *number >= minimum && *number <= maximum)
            .ok_or_else(invalid),
    }
}
fn number(input: &Value, key: &str, fallback: f64, minimum: f64, maximum: f64) -> Result<Value> {
    let value = numeric(input, key, fallback, minimum, maximum)?;
    if value.fract() == 0.0 {
        Ok(json!(value as i64))
    } else {
        Ok(json!(value))
    }
}
fn rounded(input: &Value, key: &str, fallback: f64, minimum: f64, maximum: f64) -> Result<Value> {
    Ok(json!(
        (numeric(input, key, fallback, minimum, maximum)? + 0.5).floor() as i64
    ))
}
fn text(input: &Value, key: &str, fallback: &str, maximum: usize) -> Result<String> {
    match input.get(key) {
        None => Ok(fallback.into()),
        Some(value) => value
            .as_str()
            .filter(|text| text.encode_utf16().count() <= maximum)
            .map(|text| text.trim().to_string())
            .ok_or_else(invalid),
    }
}
fn array(input: &Value, key: &str, default: Vec<Value>) -> Result<Vec<Value>> {
    match input.get(key).filter(|value| !value.is_null()) {
        None => Ok(default),
        Some(value) => value.as_array().cloned().ok_or_else(invalid),
    }
}
pub(crate) fn settings(input: Option<&Value>) -> Result<Value> {
    let empty = json!({});
    let input = input.unwrap_or(&empty);
    if !input.is_object() {
        return Err(invalid());
    }
    let colors = array(input, "colors", vec![])?;
    let directions = array(input, "directions", vec![json!("S")])?;
    let order = array(input, "frameOrder", vec![])?;
    let transforms = array(input, "transforms", vec![])?;
    if colors.len() > 8
        || colors.iter().any(|color| {
            !color.as_str().is_some_and(|color| {
                color.len() == 7
                    && color.starts_with('#')
                    && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
            })
        })
        || directions.is_empty()
        || directions.len() > 8
        || directions.iter().any(|direction| {
            !direction
                .as_str()
                .is_some_and(|direction| DIRECTIONS.contains(&direction))
        })
        || directions
            .iter()
            .map(Value::to_string)
            .collect::<HashSet<_>>()
            .len()
            != directions.len()
        || order.len() > 512
        || order.iter().any(|index| {
            !index
                .as_f64()
                .is_some_and(|index| index.fract() == 0.0 && (0.0..512.0).contains(&index))
        })
        || order
            .iter()
            .map(Value::to_string)
            .collect::<HashSet<_>>()
            .len()
            != order.len()
        || transforms.len() > 512
    {
        return Err(invalid());
    }
    let method = input
        .get("method")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(json!("color"));
    let align = input
        .get("align")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(json!("bottom-center"));
    if ![json!("color"), json!("model")].contains(&method)
        || ![json!("center"), json!("bottom-center"), json!("none")].contains(&align)
    {
        return Err(invalid());
    }
    let name = text(input, "inputName", "reference", 80)?;
    if !inputs::safe_name(&name) || !name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()) {
        return Err(invalid());
    }
    let region = input
        .get("region")
        .filter(|value| !value.is_null())
        .unwrap_or(&empty);
    if !region.is_object() {
        return Err(invalid());
    }
    let mut normalized_transforms = Vec::new();
    let mut indices = HashSet::new();
    for transform in transforms {
        if !transform.is_object() {
            return Err(invalid());
        }
        let index = rounded(&transform, "index", 0.0, 0.0, 511.0)?;
        if !indices.insert(index.as_i64().ok_or_else(invalid)?) {
            return Err(invalid());
        }
        normalized_transforms.push(json!({"index":index,"x":number(&transform,"x",0.0,-4096.0,4096.0)?,"y":number(&transform,"y",0.0,-4096.0,4096.0)?,"rotation":number(&transform,"rotation",0.0,-360.0,360.0)?,"scale":number(&transform,"scale",1.0,0.05,8.0)?,"opacity":number(&transform,"opacity",1.0,0.0,1.0)?,"durationMs":rounded(&transform,"durationMs",125.0,16.0,10000.0)?,"enabled":transform["enabled"] != false}));
    }
    Ok(
        json!({"inputName":name,"method":method,"colors":colors,"tolerance":number(input,"tolerance",24.0,0.0,255.0)?,"softness":number(input,"softness",8.0,0.0,64.0)?,"edgeConnected":input["edgeConnected"] != false,"region":{"x":number(region,"x",25.0,0.0,99.0)?,"y":number(region,"y",25.0,0.0,99.0)?,"width":number(region,"width",20.0,1.0,100.0)?,"height":number(region,"height",10.0,1.0,100.0)?},"action":text(input,"action","idle",160)?,"frameCount":rounded(input,"frameCount",4.0,1.0,16.0)?,"directions":directions,"columns":rounded(input,"columns",4.0,1.0,16.0)?,"rows":rounded(input,"rows",1.0,1.0,16.0)?,"durationMs":rounded(input,"durationMs",125.0,16.0,10000.0)?,"trim":input["trim"] != false,"align":align,"padding":rounded(input,"padding",4.0,0.0,64.0)?,"maxFrameSize":rounded(input,"maxFrameSize",256.0,16.0,1024.0)?,"frameOrder":order,"transforms":normalized_transforms,"filename":text(input,"filename","animation",100)?}),
    )
}
fn frame(value: &Value) -> Result<Value> {
    if !value.is_object() {
        return Err(invalid());
    }
    let media = inputs::media(&value["media"])?;
    if !media["mimeType"]
        .as_str()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return Err(invalid());
    }
    Ok(
        json!({"media":media,"width":number(value,"width",1.0,1.0,4096.0)?,"height":number(value,"height",1.0,1.0,4096.0)?,"durationMs":number(value,"durationMs",125.0,16.0,10000.0)?,"action":text(value,"action","",160)?,"direction":text(value,"direction","",16)?,"columns":number(value,"columns",1.0,1.0,16.0)?,"rows":number(value,"rows",1.0,1.0,16.0)?,"frameCount":number(value,"frameCount",1.0,1.0,256.0)?}),
    )
}
pub(crate) fn output(value: &Value) -> Result<Value> {
    let frames = value["frames"]
        .as_array()
        .filter(|frames| frames.len() <= 512)
        .ok_or_else(invalid)?;
    if !value.is_object() || value["type"] != "workflow-images" || value["version"] != 1 {
        return Err(invalid());
    }
    let mut parsed = json!({"type":"workflow-images","version":1,"frames":frames.iter().map(frame).collect::<Result<Vec<_>>>()?});
    if let Some(atlas) = value.get("atlas") {
        let frames = atlas["frames"]
            .as_array()
            .filter(|frames| frames.len() <= 512)
            .ok_or_else(invalid)?;
        if !atlas.is_object() {
            return Err(invalid());
        }
        let frames = frames.iter().map(|value| { if !value.is_object() { return Err(invalid()); } Ok(json!({"x":number(value,"x",0.0,0.0,4096.0)?,"y":number(value,"y",0.0,0.0,4096.0)?,"width":number(value,"width",1.0,1.0,4096.0)?,"height":number(value,"height",1.0,1.0,4096.0)?,"durationMs":number(value,"durationMs",125.0,16.0,10000.0)?,"action":text(value,"action","",160)?,"direction":text(value,"direction","",16)?})) }).collect::<Result<Vec<_>>>()?;
        parsed["atlas"] = json!({"media":inputs::media(&atlas["media"])? ,"width":number(atlas,"width",1.0,1.0,4096.0)?,"height":number(atlas,"height",1.0,1.0,4096.0)?,"frames":frames});
    }
    Ok(parsed)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_edits_match_release_defaults_bounds_and_independent_copies() {
        let source = json!({"frames":[{"sourceIndex":0},{"sourceIndex":0,
            "x":4096,"y":-4096,"rotation":-360,"scale":0.05,"opacity":0,
            "durationMs":16,"eraseStrokes":[{"radius":0.001,"restore":true,
            "points":[{"x":0,"y":1}]}]}]});
        let original = source.clone();
        let parsed = frame_edits(&source).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].scale, 1.0);
        assert_eq!(parsed[0].opacity, 1.0);
        assert_eq!(parsed[0].duration_ms, 125);
        assert!(parsed[0].erase_strokes.is_empty());
        assert_eq!(parsed[1].source_index, 0);
        assert!(parsed[1].erase_strokes[0].restore);
        assert_eq!(source, original);
        assert_eq!(
            frame_edits(&json!({"frames":vec![json!({"sourceIndex":511});512]}))
                .unwrap()
                .len(),
            512
        );
    }
    #[test]
    fn manual_edits_reject_invalid_parameters_and_null_defaults() {
        for input in [
            json!(null),
            json!([]),
            json!({}),
            json!({"frames":[]}),
            json!({"frames":vec![json!({"sourceIndex":0});513]}),
        ] {
            assert_eq!(
                frame_edits(&input).unwrap_err().code,
                "workflow_image_invalid_edits"
            );
        }
        for (key, values) in [
            (
                "sourceIndex",
                vec![json!(null), json!(-1), json!(512), json!(0.5), json!("0")],
            ),
            (
                "x",
                vec![json!(null), json!(-4097), json!(4097), json!("1")],
            ),
            ("y", vec![json!(null), json!(-4097), json!(4097)]),
            ("rotation", vec![json!(null), json!(-361), json!(361)]),
            ("scale", vec![json!(null), json!(0), json!(8.01)]),
            ("opacity", vec![json!(null), json!(-0.1), json!(1.1)]),
            (
                "durationMs",
                vec![json!(null), json!(15), json!(10001), json!(120.5)],
            ),
            ("eraseStrokes", vec![json!(null), json!({}), json!(false)]),
        ] {
            for value in values {
                let mut frame = json!({"sourceIndex":0});
                frame[key] = value;
                assert_eq!(
                    frame_edits(&json!({"frames":[frame]})).unwrap_err().code,
                    "workflow_image_invalid_edits",
                    "accepted {key}"
                );
            }
        }
        for stroke in [
            json!({"points":[{"x":0,"y":0}]}),
            json!({"radius":0.3,"points":[{"x":0,"y":0}]}),
            json!({"radius":0.1,"points":[]}),
            json!({"radius":0.1,"points":[{"x":null,"y":0}]}),
            json!({"radius":0.1,"points":[{"x":1.1,"y":0}]}),
            json!({"radius":0.1,"restore":"true","points":[{"x":0,"y":0}]}),
            json!({"radius":0.1,"points":vec![json!({"x":0,"y":0});513]}),
        ] {
            assert_eq!(
                frame_edits(&json!({"frames":[{"sourceIndex":0,"eraseStrokes":[stroke]}]}))
                    .unwrap_err()
                    .code,
                "workflow_image_invalid_edits"
            );
        }
    }
    #[test]
    fn manual_edits_limit_points_across_all_frames_and_strokes() {
        let stroke = json!({"radius":0.1,"points":vec![json!({"x":0.5,"y":0.5});512]});
        assert_eq!(
            frame_edits(
                &json!({"frames":[{"sourceIndex":0,"eraseStrokes":vec![stroke.clone();65]}]})
            )
            .unwrap_err()
            .code,
            "workflow_image_invalid_edits"
        );
        let mut frames = vec![json!({"sourceIndex":0,"eraseStrokes":vec![stroke;39]})];
        frames.push(json!({"sourceIndex":0,"eraseStrokes":[{"radius":0.25,"points":vec![json!({"x":1,"y":1});32]}]}));
        assert_eq!(
            frame_edits(&json!({"frames":frames.clone()}))
                .unwrap()
                .len(),
            2
        );
        frames[1]["eraseStrokes"][0]["points"]
            .as_array_mut()
            .unwrap()
            .push(json!({"x":1,"y":1}));
        assert_eq!(
            frame_edits(&json!({"frames":frames})).unwrap_err().code,
            "workflow_image_too_large"
        );
    }
    #[test]
    fn settings_match_release_defaults_bounds_and_transform_uniqueness() {
        let defaults = settings(None).unwrap();
        assert_eq!(defaults["inputName"], "reference");
        assert_eq!(defaults["durationMs"], 125);
        assert_eq!(defaults["directions"], json!(["S"]));
        assert_eq!(defaults["region"]["height"], 10);
        assert_eq!(
            settings(Some(&json!({"frameCount":4.5}))).unwrap()["frameCount"],
            5
        );
        for invalid_value in [
            json!(null),
            json!({"inputName":"__proto__"}),
            json!({"colors":["red"]}),
            json!({"directions":["S","S"]}),
            json!({"frameOrder":[512]}),
            json!({"tolerance":"24"}),
            json!({"transforms":[{"index":2.2},{"index":2.4}]}),
        ] {
            assert!(
                settings(Some(&invalid_value)).is_err(),
                "accepted {invalid_value}"
            );
        }
    }
    #[test]
    fn image_output_only_accepts_strict_references_and_bounded_frames() {
        let media = json!({"id":"fixture","name":"source.png","mimeType":"image/png","size":24});
        let parsed =
            output(&json!({"type":"workflow-images","version":1,"frames":[{"media":media}]}))
                .unwrap();
        assert_eq!(parsed["frames"][0]["width"], 1);
        assert_eq!(parsed["frames"][0]["durationMs"], 125);
        assert!(output(&json!({"type":"workflow-images","version":1,"frames":[{"media":{"url":"https://invalid"}}]})).is_err());
        assert!(output(
            &json!({"type":"workflow-images","version":1,"frames":[{"media":media,"width":4097}]})
        )
        .is_err());
    }
}
