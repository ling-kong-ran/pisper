//! 有界原生像素处理。每个排队/执行任务有独立取消令牌，dispose 等待实际处理退出。
use super::{image_protocol, media, Result, WorkflowError};
use crate::workflow_engine::RunCancellation;
use image::{ImageFormat, RgbaImage};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Cursor,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tokio::sync::{oneshot, Mutex, Notify};
#[path = "manual_edits.rs"]
mod manual_edits;
const LIMIT: u64 = 16_000_000;
pub(crate) struct ProcessingFrame {
    pub(crate) wire: Value,
    pub(crate) buffer: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct RasterFrame {
    pub(crate) wire: Value,
    pub(crate) pixels: RgbaImage,
}
pub(crate) struct ProcessingRequest {
    pub(crate) operation: String,
    pub(crate) frames: Vec<ProcessingFrame>,
    pub(crate) settings: Value,
    pub(crate) edits: Option<Value>,
}
pub(crate) struct ProcessingResult {
    pub(crate) frames: Vec<ProcessingFrame>,
    pub(crate) atlas: Option<ProcessingFrame>,
    pub(crate) recommended_background: Option<String>,
}
pub(crate) trait ImageAlgorithms: Send + Sync {
    fn process(
        &self,
        operation: &str,
        frames: Vec<RasterFrame>,
        settings: &Value,
        cancellation: &RunCancellation,
    ) -> Result<Vec<RasterFrame>>;
}
struct Job {
    cancellation: Arc<RunCancellation>,
    finished: AtomicBool,
    done: Notify,
}
pub(crate) struct ImageProcessor {
    queue: Mutex<()>,
    jobs: StdMutex<HashMap<String, Arc<Job>>>,
    closed: AtomicBool,
    algorithms: Option<Arc<dyn ImageAlgorithms>>,
}
fn failure(code: &str) -> WorkflowError {
    WorkflowError::coded(code, code)
}
fn cancelled(cancellation: &RunCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(failure("workflow_image_cancelled"))
    } else {
        Ok(())
    }
}
fn bounds(width: u32, height: u32, count: usize) -> Result<()> {
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width as u64 * height as u64 * count as u64 > LIMIT
    {
        Err(failure("workflow_image_too_large"))
    } else {
        Ok(())
    }
}
fn num(value: &Value, key: &str) -> f64 {
    value[key].as_f64().unwrap_or(0.0)
}
fn integer(value: &Value, key: &str, minimum: f64, maximum: f64) -> Result<u32> {
    value[key]
        .as_f64()
        .filter(|n| n.fract() == 0.0 && *n >= minimum && *n <= maximum)
        .map(|n| n as u32)
        .ok_or_else(image_protocol::invalid)
}
pub(crate) fn encode_png(pixels: &RgbaImage) -> Result<Vec<u8>> {
    let mut buffer = Cursor::new(Vec::new());
    pixels
        .write_to(&mut buffer, ImageFormat::Png)
        .map_err(|_| failure("workflow_image_processing_failed"))?;
    Ok(buffer.into_inner())
}
fn decode(frame: ProcessingFrame) -> Result<RasterFrame> {
    let mime = frame.wire["media"]["mimeType"].as_str().unwrap_or("");
    media::validate_bytes(&frame.buffer, mime)?;
    let (width, height, actual) =
        media::raster_dimensions(&frame.buffer).ok_or_else(image_protocol::invalid)?;
    if actual != mime
        || integer(&frame.wire, "width", 1.0, 4096.0)? != width
        || integer(&frame.wire, "height", 1.0, 4096.0)? != height
    {
        return Err(image_protocol::invalid());
    }
    integer(&frame.wire, "durationMs", 16.0, 10000.0)?;
    integer(&frame.wire, "columns", 1.0, 16.0)?;
    integer(&frame.wire, "rows", 1.0, 16.0)?;
    integer(&frame.wire, "frameCount", 1.0, 256.0)?;
    let format = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(image_protocol::invalid()),
    };
    let pixels = image::load_from_memory_with_format(&frame.buffer, format)
        .map_err(|_| failure("workflow_image_processing_failed"))?
        .into_rgba8();
    if pixels.width() != width || pixels.height() != height {
        return Err(image_protocol::invalid());
    }
    Ok(RasterFrame {
        wire: frame.wire,
        pixels,
    })
}
fn encode(frame: RasterFrame) -> Result<ProcessingFrame> {
    let mut wire = frame.wire;
    wire["width"] = json!(frame.pixels.width());
    wire["height"] = json!(frame.pixels.height());
    Ok(ProcessingFrame {
        wire,
        buffer: encode_png(&frame.pixels)?,
    })
}
fn crop(frame: &RasterFrame, x: u32, y: u32, width: u32, height: u32) -> RasterFrame {
    let mut wire = frame.wire.clone();
    wire["width"] = json!(width);
    wire["height"] = json!(height);
    for key in ["columns", "rows", "frameCount"] {
        wire[key] = json!(1);
    }
    RasterFrame {
        wire,
        pixels: image::imageops::crop_imm(&frame.pixels, x, y, width, height).to_image(),
    }
}
fn opaque_bounds(pixels: &RgbaImage) -> (u32, u32, u32, u32) {
    let mut left = pixels.width();
    let mut top = pixels.height();
    let mut right = 0;
    let mut bottom = 0;
    let mut any = false;
    for (x, y, pixel) in pixels.enumerate_pixels() {
        if pixel[3] > 0 {
            any = true;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
    }
    if any {
        (left, top, right - left + 1, bottom - top + 1)
    } else {
        (0, 0, 1, 1)
    }
}
fn palette(pixels: impl Iterator<Item = [u8; 4]> + Clone, maximum: usize) -> Vec<[u8; 3]> {
    let bucket = |color: [u8; 4]| {
        ((color[0] as usize >> 3) << 10)
            | ((color[1] as usize >> 3) << 5)
            | (color[2] as usize >> 3)
    };
    let mut counts = vec![0_u32; 1 << 15];
    for color in pixels.clone() {
        if color[3] >= 128 {
            counts[bucket(color)] += 1;
        }
    }
    let mut selected = counts
        .into_iter()
        .enumerate()
        .filter(|(_, count)| *count > 0)
        .collect::<Vec<_>>();
    selected.sort_by(|(index, count), (other, other_count)| {
        other_count.cmp(count).then(index.cmp(other))
    });
    selected.truncate(maximum);
    let mut exact: Vec<HashMap<u32, (u32, usize)>> =
        selected.iter().map(|_| HashMap::new()).collect();
    let positions: HashMap<_, _> = selected
        .iter()
        .enumerate()
        .map(|(position, (index, _))| (*index, position))
        .collect();
    for (order, color) in pixels.enumerate() {
        if color[3] < 128 {
            continue;
        }
        if let Some(position) = positions.get(&bucket(color)) {
            let key = ((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32;
            let count = exact[*position].entry(key).or_insert((0, order));
            count.0 += 1;
        }
    }
    exact
        .into_iter()
        .map(|counts| {
            let (color, _) = counts
                .into_iter()
                .max_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
                .unwrap_or((0, (0, 0)));
            [(color >> 16) as u8, (color >> 8) as u8, color as u8]
        })
        .collect()
}
fn rgb(text: &str) -> [u8; 3] {
    [
        u8::from_str_radix(&text[1..3], 16).unwrap_or(0),
        u8::from_str_radix(&text[3..5], 16).unwrap_or(0),
        u8::from_str_radix(&text[5..7], 16).unwrap_or(0),
    ]
}
fn color_background(
    mut frame: RasterFrame,
    settings: &Value,
    cancellation: &RunCancellation,
) -> Result<RasterFrame> {
    let width = frame.pixels.width();
    let height = frame.pixels.height();
    let colors = settings["colors"]
        .as_array()
        .ok_or_else(image_protocol::invalid)?;
    let targets = if colors.is_empty() {
        let border = (0..width)
            .flat_map(|x| {
                [
                    *frame.pixels.get_pixel(x, 0),
                    *frame.pixels.get_pixel(x, height - 1),
                ]
            })
            .chain((0..height).flat_map(|y| {
                [
                    *frame.pixels.get_pixel(0, y),
                    *frame.pixels.get_pixel(width - 1, y),
                ]
            }));
        palette(border.map(|p| p.0), 1)
    } else {
        colors
            .iter()
            .map(|color| rgb(color.as_str().unwrap_or("#000000")))
            .collect()
    };
    let original = frame.pixels.clone();
    let tolerance = num(settings, "tolerance").round();
    let softness = num(settings, "softness").round();
    for (x, y, pixel) in frame.pixels.enumerate_pixels_mut() {
        if x == 0 {
            cancelled(cancellation)?;
        }
        let distance = targets
            .iter()
            .map(|target| {
                (0..3)
                    .map(|c| (pixel[c] as i16 - target[c] as i16).unsigned_abs() as f64)
                    .fold(0.0, f64::max)
            })
            .fold(255.0, f64::min);
        if distance <= tolerance {
            pixel[3] = 0;
        } else if softness > 0.0 && distance < tolerance + softness {
            pixel[3] = (pixel[3] as f64 * (distance - tolerance) / softness).round() as u8;
        }
        let _ = y;
    }
    if settings["edgeConnected"] != false {
        let mut connected = vec![false; (width * height) as usize];
        let mut queue = VecDeque::new();
        let visit =
            |x: u32, y: u32, connected: &mut Vec<bool>, queue: &mut VecDeque<(u32, u32)>| {
                let index = (y * width + x) as usize;
                if !connected[index]
                    && (original.get_pixel(x, y)[3] == 0
                        || frame.pixels.get_pixel(x, y)[3] != original.get_pixel(x, y)[3])
                {
                    connected[index] = true;
                    queue.push_back((x, y));
                }
            };
        for x in 0..width {
            visit(x, 0, &mut connected, &mut queue);
            visit(x, height - 1, &mut connected, &mut queue);
        }
        for y in 0..height {
            visit(0, y, &mut connected, &mut queue);
            visit(width - 1, y, &mut connected, &mut queue);
        }
        let mut count = 0;
        while let Some((x, y)) = queue.pop_front() {
            count += 1;
            if count % 4096 == 0 {
                cancelled(cancellation)?;
            }
            if x > 0 {
                visit(x - 1, y, &mut connected, &mut queue);
            }
            if x + 1 < width {
                visit(x + 1, y, &mut connected, &mut queue);
            }
            if y > 0 {
                visit(x, y - 1, &mut connected, &mut queue);
            }
            if y + 1 < height {
                visit(x, y + 1, &mut connected, &mut queue);
            }
        }
        for (x, y, pixel) in frame.pixels.enumerate_pixels_mut() {
            if !connected[(y * width + x) as usize] {
                pixel[3] = original.get_pixel(x, y)[3];
            }
        }
    }
    Ok(frame)
}
fn split(
    frames: Vec<RasterFrame>,
    settings: &Value,
    cancellation: &RunCancellation,
) -> Result<Vec<RasterFrame>> {
    let mut output = Vec::new();
    for frame in frames {
        cancelled(cancellation)?;
        let generated = num(&frame.wire, "columns") > 1.0 || num(&frame.wire, "rows") > 1.0;
        let columns = num(if generated { &frame.wire } else { settings }, "columns") as u32;
        let rows = num(if generated { &frame.wire } else { settings }, "rows") as u32;
        let count = if generated {
            num(&frame.wire, "frameCount") as u32
        } else {
            (num(settings, "frameCount") as u32).min(columns * rows)
        };
        if frame.pixels.width() < columns
            || frame.pixels.height() < rows
            || count > columns * rows
            || output.len() + count as usize > 512
        {
            return Err(failure("workflow_image_invalid_grid"));
        }
        for index in 0..count {
            let column = index % columns;
            let row = index / columns;
            let x = column * frame.pixels.width() / columns;
            let y = row * frame.pixels.height() / rows;
            let mut item = crop(
                &frame,
                x,
                y,
                (column + 1) * frame.pixels.width() / columns - x,
                (row + 1) * frame.pixels.height() / rows - y,
            );
            if !generated {
                item.wire["durationMs"] = settings["durationMs"].clone();
            }
            output.push(item);
        }
    }
    Ok(output)
}
struct Transform {
    source: RasterFrame,
    area: (u32, u32, u32, u32),
    x: f64,
    y: f64,
    rotation: f64,
    scale: f64,
    opacity: f64,
    duration: f64,
    bounds: [f64; 4],
}
fn transform(
    frames: Vec<RasterFrame>,
    settings: &Value,
    cancellation: &RunCancellation,
) -> Result<Vec<RasterFrame>> {
    let order = settings["frameOrder"]
        .as_array()
        .ok_or_else(image_protocol::invalid)?;
    let edits = settings["transforms"]
        .as_array()
        .ok_or_else(image_protocol::invalid)?;
    if order
        .iter()
        .any(|value| value.as_u64().unwrap_or(u64::MAX) >= frames.len() as u64)
        || edits
            .iter()
            .any(|edit| num(edit, "index") as usize >= frames.len())
    {
        return Err(image_protocol::invalid());
    }
    let mut indices = order
        .iter()
        .map(|value| value.as_u64().unwrap_or(0) as usize)
        .collect::<Vec<_>>();
    for index in 0..frames.len() {
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
    let mut items = Vec::new();
    let align = settings["align"].as_str().unwrap_or("bottom-center");
    for index in indices {
        cancelled(cancellation)?;
        let frame = &frames[index];
        let edit = edits
            .iter()
            .find(|edit| num(edit, "index") as usize == index);
        if edit.is_some_and(|edit| edit["enabled"] == false) {
            continue;
        }
        let rect = if settings["trim"] != false {
            opaque_bounds(&frame.pixels)
        } else {
            (0, 0, frame.pixels.width(), frame.pixels.height())
        };
        let source = if align == "none" {
            frame.clone()
        } else {
            crop(frame, rect.0, rect.1, rect.2, rect.3)
        };
        let area = if align == "none" {
            rect
        } else {
            (0, 0, source.pixels.width(), source.pixels.height())
        };
        let get = |key: &str, default: f64| edit.map(|edit| num(edit, key)).unwrap_or(default);
        let scale = get("scale", 1.0);
        let x = get("x", 0.0);
        let y = get("y", 0.0)
            - if align == "bottom-center" {
                source.pixels.height() as f64 * scale / 2.0
            } else {
                0.0
            };
        let rotation = get("rotation", 0.0).to_radians();
        let (cos, sin) = (rotation.cos(), rotation.sin());
        let mut bound = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for (cx, cy) in [
            (area.0, area.1),
            (area.0 + area.2, area.1),
            (area.0 + area.2, area.1 + area.3),
            (area.0, area.1 + area.3),
        ] {
            let cx = cx as f64 - source.pixels.width() as f64 / 2.0;
            let cy = cy as f64 - source.pixels.height() as f64 / 2.0;
            let px = x + (cx * cos - cy * sin) * scale;
            let py = y + (cx * sin + cy * cos) * scale;
            bound[0] = bound[0].min(px);
            bound[1] = bound[1].min(py);
            bound[2] = bound[2].max(px);
            bound[3] = bound[3].max(py);
        }
        let duration = get("durationMs", num(&source.wire, "durationMs"));
        items.push(Transform {
            source,
            area,
            x,
            y,
            rotation,
            scale,
            opacity: get("opacity", 1.0),
            duration,
            bounds: bound,
        });
    }
    if items.is_empty() {
        return Err(failure("workflow_image_empty"));
    }
    let padding = num(settings, "padding");
    let left = (items
        .iter()
        .map(|item| item.bounds[0])
        .fold(f64::INFINITY, f64::min)
        + 1e-9)
        .floor()
        - padding;
    let top = (items
        .iter()
        .map(|item| item.bounds[1])
        .fold(f64::INFINITY, f64::min)
        + 1e-9)
        .floor()
        - padding;
    let natural_width = (items
        .iter()
        .map(|item| item.bounds[2])
        .fold(f64::NEG_INFINITY, f64::max)
        - 1e-9)
        .ceil()
        + padding
        - left;
    let natural_height = (items
        .iter()
        .map(|item| item.bounds[3])
        .fold(f64::NEG_INFINITY, f64::max)
        - 1e-9)
        .ceil()
        + padding
        - top;
    let maximum = num(settings, "maxFrameSize");
    let raster_scale = (maximum / natural_width.max(natural_height)).min(1.0);
    let width = (natural_width * raster_scale).ceil().clamp(1.0, maximum) as u32;
    let height = (natural_height * raster_scale).ceil().clamp(1.0, maximum) as u32;
    bounds(width, height, items.len())?;
    items
        .into_iter()
        .map(|item| {
            let mut pixels = RgbaImage::new(width, height);
            let (cos, sin) = (item.rotation.cos(), item.rotation.sin());
            for y in 0..height {
                cancelled(cancellation)?;
                for x in 0..width {
                    let tx = left + (x as f64 + 0.5) / raster_scale - item.x;
                    let ty = top + (y as f64 + 0.5) / raster_scale - item.y;
                    let sx = ((tx * cos + ty * sin) / item.scale
                        + item.source.pixels.width() as f64 / 2.0)
                        .floor();
                    let sy = ((-tx * sin + ty * cos) / item.scale
                        + item.source.pixels.height() as f64 / 2.0)
                        .floor();
                    if sx < item.area.0 as f64
                        || sy < item.area.1 as f64
                        || sx >= (item.area.0 + item.area.2) as f64
                        || sy >= (item.area.1 + item.area.3) as f64
                    {
                        continue;
                    }
                    let mut pixel = *item.source.pixels.get_pixel(sx as u32, sy as u32);
                    pixel[3] = (pixel[3] as f64 * item.opacity).round() as u8;
                    pixels.put_pixel(x, y, pixel);
                }
            }
            let mut wire = item.source.wire;
            wire["durationMs"] = json!(item.duration);
            for key in ["columns", "rows", "frameCount"] {
                wire[key] = json!(1);
            }
            Ok(RasterFrame { wire, pixels })
        })
        .collect()
}
fn atlas(
    frames: &[RasterFrame],
    settings: &Value,
    cancellation: &RunCancellation,
) -> Result<ProcessingFrame> {
    let padding = num(settings, "padding") as u32;
    let cell_width = frames
        .iter()
        .map(|frame| frame.pixels.width())
        .max()
        .ok_or_else(image_protocol::invalid)?
        + padding * 2;
    let cell_height = frames
        .iter()
        .map(|frame| frame.pixels.height())
        .max()
        .ok_or_else(image_protocol::invalid)?
        + padding * 2;
    if cell_width > 4096 || cell_height > 4096 {
        return Err(failure("workflow_image_too_large"));
    }
    let count = frames.len() as u32;
    let minimum = count.div_ceil(4096 / cell_height).max(1);
    let maximum = count.min(4096 / cell_width);
    if minimum > maximum {
        return Err(failure("workflow_image_too_large"));
    }
    let columns = (minimum..=maximum)
        .min_by_key(|columns| (columns * cell_width).max(count.div_ceil(*columns) * cell_height))
        .ok_or_else(image_protocol::invalid)?;
    let width = columns * cell_width;
    let height = count.div_ceil(columns) * cell_height;
    bounds(width, height, 1)?;
    let mut pixels = RgbaImage::new(width, height);
    let mut metadata = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        cancelled(cancellation)?;
        let x = (index as u32 % columns) * cell_width
            + padding
            + (cell_width - padding * 2 - frame.pixels.width()) / 2;
        let y = (index as u32 / columns) * cell_height
            + padding
            + (cell_height - padding * 2 - frame.pixels.height()) / 2;
        for (sx, sy, pixel) in frame.pixels.enumerate_pixels() {
            pixels.put_pixel(x + sx, y + sy, *pixel);
        }
        metadata.push(json!({"x":x,"y":y,"width":frame.pixels.width(),"height":frame.pixels.height(),"durationMs":frame.wire["durationMs"],"action":frame.wire["action"],"direction":frame.wire["direction"]}));
    }
    Ok(ProcessingFrame {
        wire: json!({"width":width,"height":height,"frames":metadata}),
        buffer: encode_png(&pixels)?,
    })
}
fn run(
    request: ProcessingRequest,
    algorithms: Option<Arc<dyn ImageAlgorithms>>,
    cancellation: &RunCancellation,
) -> Result<ProcessingResult> {
    cancelled(cancellation)?;
    if request.frames.is_empty() || request.frames.len() > 512 {
        return Err(image_protocol::invalid());
    }
    let mut pixels = 0_u64;
    let mut bytes = 0_usize;
    for frame in &request.frames {
        pixels += num(&frame.wire, "width") as u64 * num(&frame.wire, "height") as u64;
        bytes += frame.buffer.len();
        if pixels > LIMIT || bytes > 64 * 1024 * 1024 {
            return Err(failure("workflow_image_too_large"));
        }
    }
    let settings = image_protocol::settings(Some(&request.settings))?;
    let edits = if request.operation == "edit" {
        let edits = image_protocol::frame_edits(
            request
                .edits
                .as_ref()
                .ok_or_else(image_protocol::invalid_edits)?,
        )?;
        if edits
            .iter()
            .any(|edit| edit.source_index >= request.frames.len())
        {
            return Err(image_protocol::invalid_edits());
        }
        // 先计入每份复制的输出像素，避免分配之后才发现越界。
        let output_pixels = edits.iter().try_fold(0_u64, |total, edit| {
            let frame = &request.frames[edit.source_index];
            let width = integer(&frame.wire, "width", 1.0, 4096.0)?;
            let height = integer(&frame.wire, "height", 1.0, 4096.0)?;
            Ok::<_, WorkflowError>(total + width as u64 * height as u64)
        })?;
        if output_pixels > LIMIT {
            return Err(failure("workflow_image_too_large"));
        }
        Some(edits)
    } else {
        None
    };
    let mut frames = Vec::new();
    for frame in request.frames {
        cancelled(cancellation)?;
        frames.push(decode(frame)?);
    }
    if request.operation == "palette" {
        let colors = palette(frames[0].pixels.pixels().map(|pixel| pixel.0), 8);
        let presets = ["#FF00FF", "#00FF00", "#00FFFF", "#FFFFFF", "#000000"];
        let mut best = (presets[0], -1_i16);
        for preset in presets {
            let target = rgb(preset);
            let distance = colors
                .iter()
                .map(|color| {
                    (0..3)
                        .map(|c| (color[c] as i16 - target[c] as i16).abs())
                        .max()
                        .unwrap_or(0)
                })
                .min()
                .unwrap_or(0);
            if distance > best.1 {
                best = (preset, distance);
            }
        }
        return Ok(ProcessingResult {
            frames: vec![],
            atlas: None,
            recommended_background: Some(best.0.to_string()),
        });
    }
    frames = match request.operation.as_str() {
        "background" if settings["method"] == "color" => frames
            .into_iter()
            .map(|frame| color_background(frame, &settings, cancellation))
            .collect::<Result<_>>()?,
        "background" | "inpaint" => algorithms
            .ok_or_else(|| failure("workflow_image_engine_missing"))?
            .process(&request.operation, frames, &settings, cancellation)?,
        "frames" => split(frames, &settings, cancellation)?,
        "transform" => transform(frames, &settings, cancellation)?,
        "edit" => manual_edits::edit_frames(
            &frames,
            edits.as_deref().ok_or_else(image_protocol::invalid_edits)?,
            cancellation,
        )?,
        "export" => frames,
        _ => return Err(image_protocol::invalid()),
    };
    let pixels = frames
        .iter()
        .map(|frame| frame.pixels.width() as u64 * frame.pixels.height() as u64)
        .sum::<u64>();
    if frames.len() > 512 || pixels > LIMIT {
        return Err(failure("workflow_image_too_large"));
    }
    cancelled(cancellation)?;
    let atlas = if request.operation == "export" {
        Some(atlas(&frames, &settings, cancellation)?)
    } else {
        None
    };
    let mut encoded = Vec::new();
    for frame in frames {
        cancelled(cancellation)?;
        encoded.push(encode(frame)?);
    }
    Ok(ProcessingResult {
        frames: encoded,
        atlas,
        recommended_background: None,
    })
}
impl ImageProcessor {
    pub(crate) fn new(algorithms: Option<Arc<dyn ImageAlgorithms>>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(()),
            jobs: StdMutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            algorithms,
        })
    }
    pub(crate) async fn process(
        self: &Arc<Self>,
        request: ProcessingRequest,
        cancellation: Arc<RunCancellation>,
    ) -> Result<ProcessingResult> {
        if self.closed.load(Ordering::Acquire) {
            return Err(failure("workflow_image_closed"));
        }
        cancelled(&cancellation)?;
        let token = super::id()?;
        let job = Arc::new(Job {
            cancellation: Arc::new(RunCancellation::default()),
            finished: AtomicBool::new(false),
            done: Notify::new(),
        });
        {
            let mut jobs = self.jobs.lock().map_err(WorkflowError::io)?;
            if self.closed.load(Ordering::Acquire) {
                return Err(failure("workflow_image_closed"));
            }
            jobs.insert(token.clone(), job.clone());
        }
        let (service, owned) = (self.clone(), job.clone());
        let (sender, mut receiver) = oneshot::channel();
        tokio::spawn(async move {
            let result=async{let _guard=tokio::select!{_=owned.cancellation.cancelled()=>return Err(failure("workflow_image_cancelled")),guard=service.queue.lock()=>guard};cancelled(&owned.cancellation)?;let(algorithms,child)=(service.algorithms.clone(),owned.cancellation.clone());let mut task=tokio::task::spawn_blocking(move||run(request,algorithms,&child));let result=tokio::select!{result=&mut task=>result.map_err(|_|failure("workflow_image_processing_failed"))?,_=tokio::time::sleep(Duration::from_secs(120))=>{owned.cancellation.cancel();let _=task.await;return Err(failure("workflow_image_timeout"));}};result}.await;
            if let Ok(mut jobs) = service.jobs.lock() {
                jobs.remove(&token);
            }
            owned.finished.store(true, Ordering::Release);
            owned.done.notify_waiters();
            let _ = sender.send(result);
        });
        // 请求取消后继续等实际 CPU 任务退出，编辑结果和队列名额不会提前关闭。
        tokio::select! {_=cancellation.cancelled()=>{job.cancellation.cancel();let _=receiver.await;Err(failure("workflow_image_cancelled"))},result=&mut receiver=>result.map_err(|_|failure("workflow_image_processing_failed"))?}
    }
    pub(crate) async fn dispose(&self) {
        self.closed.store(true, Ordering::Release);
        let jobs = self
            .jobs
            .lock()
            .map(|jobs| jobs.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for job in &jobs {
            job.cancellation.cancel();
        }
        for job in jobs {
            loop {
                let notified = job.done.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if job.finished.load(Ordering::Acquire) {
                    break;
                }
                notified.await;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(pixels: RgbaImage) -> RasterFrame {
        let wire = json!({"media":{"id":"fixture","name":"fixture.png","mimeType":"image/png","size":1},"width":pixels.width(),"height":pixels.height(),"durationMs":125,"action":"idle","direction":"S","columns":1,"rows":1,"frameCount":1});
        RasterFrame { wire, pixels }
    }
    fn edit_request(source: &RasterFrame, edits: Value) -> ProcessingRequest {
        ProcessingRequest {
            operation: "edit".into(),
            frames: vec![ProcessingFrame {
                wire: source.wire.clone(),
                buffer: encode_png(&source.pixels).unwrap(),
            }],
            settings: json!({"trim":true,"padding":64,"maxFrameSize":16}),
            edits: Some(edits),
        }
    }
    #[tokio::test]
    async fn manual_edit_validation_and_copy_budget_leave_processor_reusable() {
        let source = frame(RgbaImage::from_pixel(
            32,
            20,
            image::Rgba([20, 40, 80, 123]),
        ));
        let processor = ImageProcessor::new(None);
        for edits in [
            json!({"frames":[{"sourceIndex":1}]}),
            json!({"frames":[{"sourceIndex":0,"opacity":null}]}),
        ] {
            let failure = processor
                .process(
                    edit_request(&source, edits),
                    Arc::new(RunCancellation::default()),
                )
                .await
                .err()
                .unwrap();
            assert_eq!(failure.code, "workflow_image_invalid_edits");
        }
        let large = frame(RgbaImage::from_pixel(
            256,
            256,
            image::Rgba([20, 40, 80, 123]),
        ));
        assert_eq!(
            processor
                .process(
                    edit_request(&large, json!({"frames":vec![json!({"sourceIndex":0});256]})),
                    Arc::new(RunCancellation::default())
                )
                .await
                .err()
                .unwrap()
                .code,
            "workflow_image_too_large"
        );
        let result = processor
            .process(
                edit_request(&source, json!({"frames":[{"sourceIndex":0}]})),
                Arc::new(RunCancellation::default()),
            )
            .await
            .unwrap();
        assert_eq!(
            (
                result.frames[0].wire["width"].as_u64(),
                result.frames[0].wire["height"].as_u64()
            ),
            (Some(32), Some(20))
        );
        assert_eq!(
            image::load_from_memory(&result.frames[0].buffer)
                .unwrap()
                .into_rgba8(),
            source.pixels
        );
        assert!(processor.jobs.lock().unwrap().is_empty());
        processor.dispose().await;
    }
    #[tokio::test]
    async fn manual_queued_cancellation_drains_owned_job_and_preserves_source() {
        let source = frame(RgbaImage::from_pixel(3, 3, image::Rgba([30, 70, 100, 200])));
        let original = source.pixels.clone();
        let processor = ImageProcessor::new(None);
        let queue = processor.queue.lock().await;
        let cancellation = Arc::new(RunCancellation::default());
        let request = edit_request(&source, json!({"frames":[{"sourceIndex":0}]}));
        let (service, token) = (processor.clone(), cancellation.clone());
        let task = tokio::spawn(async move { service.process(request, token).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while processor.jobs.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .err()
                .unwrap()
                .code,
            "workflow_image_cancelled"
        );
        assert!(processor.jobs.lock().unwrap().is_empty());
        assert_eq!(source.pixels, original);
        drop(queue);
        let output = processor
            .process(
                edit_request(&source, json!({"frames":[{"sourceIndex":0,"opacity":0.5}]})),
                Arc::new(RunCancellation::default()),
            )
            .await
            .unwrap();
        assert_eq!(
            image::load_from_memory(&output.frames[0].buffer)
                .unwrap()
                .into_rgba8()
                .get_pixel(1, 1)
                .0,
            [30, 70, 100, 100]
        );
        processor.dispose().await;
    }
    #[test]
    fn edge_connected_color_key_keeps_enclosed_matching_pixels_and_soft_alpha() {
        let mut pixels = RgbaImage::from_pixel(5, 5, image::Rgba([0, 0, 0, 255]));
        for x in 0..5 {
            pixels.put_pixel(x, 0, image::Rgba([255, 0, 255, 255]));
        }
        pixels.put_pixel(2, 2, image::Rgba([255, 0, 255, 255]));
        pixels.put_pixel(0, 1, image::Rgba([250, 0, 255, 200]));
        let settings = image_protocol::settings(Some(
            &json!({"colors":["#ff00ff"],"tolerance":0,"softness":10}),
        ))
        .unwrap();
        let result =
            color_background(frame(pixels), &settings, &RunCancellation::default()).unwrap();
        assert_eq!(result.pixels.get_pixel(2, 0)[3], 0);
        assert_eq!(result.pixels.get_pixel(2, 2)[3], 255);
        assert_eq!(result.pixels.get_pixel(0, 1)[3], 100);
    }
    #[test]
    fn uneven_grid_preserves_all_pixels_and_generated_timing() {
        let mut pixels = RgbaImage::new(5, 3);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8, y as u8, 0, 255]);
        }
        let mut source = frame(pixels);
        source.wire["columns"] = json!(2);
        source.wire["rows"] = json!(2);
        source.wire["frameCount"] = json!(4);
        source.wire["durationMs"] = json!(300);
        let output = split(
            vec![source],
            &image_protocol::settings(None).unwrap(),
            &RunCancellation::default(),
        )
        .unwrap();
        assert_eq!(
            output
                .iter()
                .map(|f| (f.pixels.width(), f.pixels.height()))
                .collect::<Vec<_>>(),
            vec![(2, 1), (3, 1), (2, 2), (3, 2)]
        );
        assert!(output.iter().all(|f| f.wire["durationMs"] == 300));
        assert_eq!(output[3].pixels.get_pixel(2, 1)[0], 4);
    }
    #[test]
    fn rotation_alignment_and_atlas_use_actual_shared_canvas() {
        let mut pixels = RgbaImage::new(3, 2);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        pixels.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        let settings=image_protocol::settings(Some(&json!({"padding":0,"align":"center","transforms":[{"index":0,"rotation":90}],"maxFrameSize":16}))).unwrap();
        let output =
            transform(vec![frame(pixels)], &settings, &RunCancellation::default()).unwrap();
        assert_eq!(
            (output[0].pixels.width(), output[0].pixels.height()),
            (2, 2)
        );
        assert_eq!(output[0].pixels.pixels().filter(|p| p[3] > 0).count(), 2);
        let packed = atlas(&output, &settings, &RunCancellation::default()).unwrap();
        assert_eq!(packed.wire["frames"][0]["width"], 2);
        assert_eq!(image::load_from_memory(&packed.buffer).unwrap().height(), 2);
    }
    #[tokio::test]
    async fn native_png_decode_process_and_dispose_have_real_outputs() {
        let pixels = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 255, 255]));
        let source = frame(pixels);
        let request = ProcessingRequest {
            operation: "background".into(),
            frames: vec![ProcessingFrame {
                buffer: encode_png(&source.pixels).unwrap(),
                wire: source.wire,
            }],
            settings: image_protocol::settings(Some(&json!({"colors":["#ff00ff"]}))).unwrap(),
            edits: None,
        };
        let processor = ImageProcessor::new(None);
        let output = processor
            .process(request, Arc::new(RunCancellation::default()))
            .await
            .unwrap();
        let pixels = image::load_from_memory(&output.frames[0].buffer)
            .unwrap()
            .into_rgba8();
        assert!(pixels.pixels().all(|pixel| pixel[3] == 0));
        processor.dispose().await;
        assert!(processor.jobs.lock().unwrap().is_empty());
    }
}
