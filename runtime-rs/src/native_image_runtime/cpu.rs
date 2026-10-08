//! 固定 U²-Net 模型使用原生 CPU 推理；预处理、量化遮罩和双线性采样与 release 相同。
use crate::{
    native_workflow::{
        engine_cache::EngineCache,
        image_processing::{ImageAlgorithms, RasterFrame},
        Result, WorkflowError,
    },
    workflow_engine::RunCancellation,
};
use image::RgbaImage;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tract::prelude::*;

pub(crate) struct CpuImageAlgorithms {
    engines: Arc<EngineCache>,
    model: Mutex<Option<(String, Arc<Runnable>)>>,
}
fn failed() -> WorkflowError {
    WorkflowError::coded("workflow_image_processing_failed", "本地图片计算失败。")
}
fn active(cancellation: &RunCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        Err(WorkflowError::coded(
            "workflow_image_cancelled",
            "图片处理已取消。",
        ))
    } else {
        Ok(())
    }
}
impl CpuImageAlgorithms {
    pub(crate) fn new(engines: Arc<EngineCache>) -> Arc<Self> {
        Arc::new(Self {
            engines,
            model: Mutex::new(None),
        })
    }
    fn u2net(&self, cancellation: &RunCancellation) -> Result<Arc<Runnable>> {
        active(cancellation)?;
        let files = self.engines.execution_files_blocking("background")?;
        let bytes = files.get("u2netp.onnx").ok_or_else(|| {
            WorkflowError::coded("workflow_image_engine_missing", "请先下载本地抠图模型。")
        })?;
        let digest = crate::native_workflow::media::digest(bytes);
        let mut cached = self.model.lock().map_err(|_| failed())?;
        if let Some((before, model)) = &*cached {
            if before == &digest {
                return Ok(model.clone());
            }
        }
        let model = load_model(bytes)?;
        active(cancellation)?;
        let model = Arc::new(model);
        *cached = Some((digest, model.clone()));
        Ok(model)
    }
}
fn load_model(bytes: &[u8]) -> Result<Runnable> {
    let mut inference = tract::onnx()
        .map_err(|_| failed())?
        .load_buffer(bytes)
        .map_err(|_| failed())?;
    if inference.input_count().map_err(|_| failed())? != 1
        || inference.output_count().map_err(|_| failed())? == 0
    {
        return Err(failed());
    }
    inference
        .set_input_fact(0, "1,3,320,320,f32")
        .map_err(|_| failed())?;
    inference
        .into_model()
        .map_err(|_| failed())?
        .into_runnable()
        .map_err(|_| failed())
}
fn bilinear(
    width: usize,
    height: usize,
    x: f64,
    y: f64,
    sample: impl Fn(usize, usize) -> f64,
) -> f64 {
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let dx = x - x0 as f64;
    let dy = y - y0 as f64;
    (sample(x0, y0) * (1.0 - dx) + sample(x1, y0) * dx) * (1.0 - dy)
        + (sample(x0, y1) * (1.0 - dx) + sample(x1, y1) * dx) * dy
}
fn input(pixels: &RgbaImage, cancellation: &RunCancellation) -> Result<Vec<f32>> {
    let plane = 320 * 320;
    let mut input = vec![0_f32; plane * 3];
    let mut maximum = 0_f64;
    for y in 0..320 {
        active(cancellation)?;
        for x in 0..320 {
            for channel in 0..3 {
                let value = bilinear(
                    pixels.width() as usize,
                    pixels.height() as usize,
                    (x as f64 + 0.5) * pixels.width() as f64 / 320.0 - 0.5,
                    (y as f64 + 0.5) * pixels.height() as f64 / 320.0 - 0.5,
                    |x, y| f64::from(pixels.get_pixel(x as u32, y as u32)[channel]),
                );
                input[channel * plane + y * 320 + x] = value as f32;
                maximum = maximum.max(value);
            }
        }
    }
    let maximum = if maximum == 0.0 { 255.0 } else { maximum };
    for (channel, (mean, deviation)) in [(0.485, 0.229), (0.456, 0.224), (0.406, 0.225)]
        .into_iter()
        .enumerate()
    {
        for value in &mut input[channel * plane..(channel + 1) * plane] {
            *value = ((f64::from(*value) / maximum - mean) / deviation) as f32;
        }
    }
    Ok(input)
}
fn apply_mask(
    mut pixels: RgbaImage,
    prediction: &[f32],
    cancellation: &RunCancellation,
) -> Result<RgbaImage> {
    if prediction.len() != 320 * 320 || prediction.iter().any(|value| !value.is_finite()) {
        return Err(failed());
    }
    let minimum = prediction.iter().copied().fold(f32::INFINITY, f32::min) as f64;
    let maximum = prediction.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    // Uint8ClampedArray 使用最近偶数，不能在输入遮罩阶段提前普通四舍五入。
    let mask = prediction
        .iter()
        .map(|value| {
            if maximum > minimum {
                ((f64::from(*value) - minimum) / (maximum - minimum) * 255.0)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let width = pixels.width();
    let height = pixels.height();
    for y in 0..height {
        active(cancellation)?;
        for x in 0..width {
            let alpha = bilinear(
                320,
                320,
                (x as f64 + 0.5) * 320.0 / width as f64 - 0.5,
                (y as f64 + 0.5) * 320.0 / height as f64 - 0.5,
                |x, y| f64::from(mask[y * 320 + x]),
            );
            let pixel = pixels.get_pixel_mut(x, y);
            pixel[3] = (f64::from(pixel[3]) * alpha / 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    Ok(pixels)
}
impl ImageAlgorithms for CpuImageAlgorithms {
    fn process(
        &self,
        operation: &str,
        frames: Vec<RasterFrame>,
        settings: &Value,
        cancellation: &RunCancellation,
    ) -> Result<Vec<RasterFrame>> {
        active(cancellation)?;
        match operation {
            "background" => {
                let model = self.u2net(cancellation)?;
                let mut output = Vec::with_capacity(frames.len());
                for mut frame in frames {
                    active(cancellation)?;
                    let input = input(&frame.pixels, cancellation)?;
                    let tensor =
                        Tensor::from_slice(&[1, 3, 320, 320], &input).map_err(|_| failed())?;
                    let result = model.run([tensor]).map_err(|_| failed())?;
                    let prediction = result
                        .first()
                        .ok_or_else(failed)?
                        .as_slice::<f32>()
                        .map_err(|_| failed())?;
                    frame.pixels = apply_mask(frame.pixels, prediction, cancellation)?;
                    output.push(frame);
                }
                Ok(output)
            }
            "inpaint" => {
                self.engines.execution_files_blocking("inpaint")?;
                frames
                    .into_iter()
                    .map(|mut frame| {
                        active(cancellation)?;
                        frame.pixels =
                            super::telea::inpaint(frame.pixels, &settings["region"], cancellation)?;
                        Ok(frame)
                    })
                    .collect()
            }
            _ => Err(failed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_preprocessing_and_constant_mask_keep_rgb_and_original_alpha() {
        let pixels = RgbaImage::from_pixel(2, 2, image::Rgba([255, 128, 0, 129]));
        let input = input(&pixels, &RunCancellation::default()).unwrap();
        assert!((input[0] - (1.0 - 0.485) / 0.229).abs() < 1e-5);
        assert!((input[320 * 320] - (128.0 / 255.0 - 0.456) / 0.224).abs() < 1e-5);
        let result = apply_mask(
            pixels.clone(),
            &vec![1.0; 320 * 320],
            &RunCancellation::default(),
        )
        .unwrap();
        assert_eq!(result.get_pixel(0, 0).0, [255, 128, 0, 0]);
        let mut prediction = vec![0.0; 320 * 320];
        for y in 0..320 {
            for x in 160..320 {
                prediction[y * 320 + x] = 1.0;
            }
        }
        let result = apply_mask(pixels, &prediction, &RunCancellation::default()).unwrap();
        assert_eq!(result.get_pixel(0, 0)[3], 0);
        assert_eq!(result.get_pixel(1, 0)[3], 129);
    }
    #[test]
    fn cancellation_and_nonfinite_model_output_are_real_failures() {
        let cancellation = RunCancellation::default();
        cancellation.cancel();
        assert_eq!(
            input(&RgbaImage::new(2, 2), &cancellation)
                .unwrap_err()
                .code,
            "workflow_image_cancelled"
        );
        assert!(apply_mask(
            RgbaImage::new(2, 2),
            &vec![f32::NAN; 320 * 320],
            &RunCancellation::default()
        )
        .is_err());
        assert!(load_model(b"invalid protobuf").is_err());
    }
    #[test]
    #[ignore = "需要单独提供经过 release 固定摘要核验的公开模型 fixture；普通测试不访问网络"]
    fn fixed_release_u2net_executes_on_native_cpu() {
        let path = std::env::var("PISPER_TEST_U2NET_MODEL")
            .expect("PISPER_TEST_U2NET_MODEL fixed public fixture");
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 4_574_861);
        assert_eq!(
            crate::native_workflow::media::digest(&bytes),
            "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8"
        );
        let model = load_model(&bytes).unwrap();
        let pixels = RgbaImage::from_fn(32, 32, |x, y| {
            if (8..24).contains(&x) && (8..24).contains(&y) {
                image::Rgba([220, 30, 40, 190])
            } else {
                image::Rgba([245, 245, 245, 255])
            }
        });
        let tensor = Tensor::from_slice(
            &[1, 3, 320, 320],
            &input(&pixels, &RunCancellation::default()).unwrap(),
        )
        .unwrap();
        let result = model.run([tensor]).unwrap();
        let prediction = result[0].as_slice::<f32>().unwrap();
        assert_eq!(prediction.len(), 320 * 320);
        let output = apply_mask(pixels.clone(), prediction, &RunCancellation::default()).unwrap();
        assert!(output
            .pixels()
            .zip(pixels.pixels())
            .any(|(after, before)| after[3] < before[3]));
        assert!(output
            .pixels()
            .zip(pixels.pixels())
            .all(|(after, before)| after.0[..3] == before.0[..3] && after[3] <= before[3]));
    }
}
