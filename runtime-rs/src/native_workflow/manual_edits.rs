//! 人工编辑重放 shared/image 的固定画布和连续胶囊笔刷，源帧始终只读。
use super::{cancelled, failure, image_protocol, RasterFrame, Result, RunCancellation, LIMIT};
use image::RgbaImage;
use image_protocol::{AlphaStroke, FrameEdit};
use serde_json::json;

fn alpha_strokes(
    original: &RgbaImage,
    strokes: &[AlphaStroke],
    remaining: &mut u64,
    cancellation: &RunCancellation,
) -> Result<RgbaImage> {
    let mut pixels = original.clone();
    let (width, height) = (original.width() as f64, original.height() as f64);
    for stroke in strokes {
        cancelled(cancellation)?;
        let radius = stroke.radius * width.min(height);
        let radius_squared = radius * radius;
        for (index, end) in stroke.points.iter().enumerate() {
            cancelled(cancellation)?;
            let start = &stroke.points[index.saturating_sub(1)];
            let (ax, ay) = (start.x * width, start.y * height);
            let (bx, by) = (end.x * width, end.y * height);
            let (dx, dy) = (bx - ax, by - ay);
            let length_squared = dx * dx + dy * dy;
            let left = (ax.min(bx) - radius).floor().max(0.0) as u32;
            let right = (ax.max(bx) + radius).ceil().min(width - 1.0) as u32;
            let top = (ay.min(by) - radius).floor().max(0.0) as u32;
            let bottom = (ay.max(by) + radius).ceil().min(height - 1.0) as u32;
            let work = (right - left + 1) as u64 * (bottom - top + 1) as u64;
            *remaining = remaining
                .checked_sub(work)
                .ok_or_else(|| failure("workflow_image_too_large"))?;
            for y in top..=bottom {
                cancelled(cancellation)?;
                for x in left..=right {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let t = if length_squared > 0.0 {
                        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (ex, ey) = (px - ax - t * dx, py - ay - t * dy);
                    if ex * ex + ey * ey <= radius_squared {
                        pixels.get_pixel_mut(x, y)[3] = if stroke.restore {
                            original.get_pixel(x, y)[3]
                        } else {
                            0
                        };
                    }
                }
            }
        }
    }
    Ok(pixels)
}

pub(super) fn edit_frames(
    frames: &[RasterFrame],
    edits: &[FrameEdit],
    cancellation: &RunCancellation,
) -> Result<Vec<RasterFrame>> {
    cancelled(cancellation)?;
    let mut total = 0_u64;
    for edit in edits {
        let source = frames
            .get(edit.source_index)
            .ok_or_else(image_protocol::invalid_edits)?;
        total += source.pixels.width() as u64 * source.pixels.height() as u64;
        if total > LIMIT {
            return Err(failure("workflow_image_too_large"));
        }
    }
    let mut remaining = 128_000_000;
    let mut output = Vec::with_capacity(edits.len());
    for edit in edits {
        cancelled(cancellation)?;
        let original = &frames[edit.source_index];
        let source = alpha_strokes(
            &original.pixels,
            &edit.erase_strokes,
            &mut remaining,
            cancellation,
        )?;
        let (width, height) = (source.width(), source.height());
        let mut pixels = RgbaImage::new(width, height);
        // 与预览相同的计算顺序；人工画布不采用自动对齐/包围盒缩放。
        let radians = edit.rotation * std::f64::consts::PI / 180.0;
        let (cos, sin) = (radians.cos(), radians.sin());
        for y in 0..height {
            cancelled(cancellation)?;
            for x in 0..width {
                let tx = x as f64 + 0.5 - width as f64 / 2.0 - edit.x;
                let ty = y as f64 + 0.5 - height as f64 / 2.0 - edit.y;
                let sx = ((tx * cos + ty * sin) / edit.scale + width as f64 / 2.0).floor();
                let sy = ((-tx * sin + ty * cos) / edit.scale + height as f64 / 2.0).floor();
                if sx < 0.0 || sy < 0.0 || sx >= width as f64 || sy >= height as f64 {
                    continue;
                }
                let mut pixel = *source.get_pixel(sx as u32, sy as u32);
                pixel[3] = (pixel[3] as f64 * edit.opacity).round() as u8;
                pixels.put_pixel(x, y, pixel);
            }
        }
        let mut wire = original.wire.clone();
        wire["durationMs"] = json!(edit.duration_ms);
        for key in ["columns", "rows", "frameCount"] {
            wire[key] = json!(1);
        }
        output.push(RasterFrame { wire, pixels });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn frame(width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 4]) -> RasterFrame {
        RasterFrame {
            wire: json!({"width":width,"height":height,"durationMs":125,"action":"walk",
                "direction":"S","columns":3,"rows":2,"frameCount":6}),
            pixels: RgbaImage::from_fn(width, height, |x, y| Rgba(color(x, y))),
        }
    }
    fn apply(frames: &[RasterFrame], edits: serde_json::Value) -> Vec<RasterFrame> {
        edit_frames(
            frames,
            &image_protocol::frame_edits(&edits).unwrap(),
            &RunCancellation::default(),
        )
        .unwrap()
    }
    #[test]
    fn manual_alpha_matches_fixed_release_oracle_for_diagonal_edge_and_restore() {
        let source = frame(17, 11, |x, y| {
            [30, 70, 100, ((x * 31 + y * 17) % 256) as u8]
        });
        let original = source.pixels.clone();
        let output = apply(
            &[source.clone()],
            json!({"frames":[{"sourceIndex":0,"eraseStrokes":[
            {"radius":0.001,"points":[{"x":0.5/17.0,"y":0.5/11.0},{"x":16.5/17.0,"y":10.5/11.0}]},
            {"radius":0.04,"points":[{"x":0,"y":0.3},{"x":0.43,"y":0.97},{"x":1,"y":0}]},
            {"radius":0.1,"restore":true,"points":[{"x":0.4,"y":0.3},{"x":0.6,"y":0.7}]},
            {"radius":0.25,"points":[{"x":0,"y":1}]}]}]}),
        );
        // 固定预期由 release 5821602 的原始 editFrames + 共享 alpha 算法生成。
        let expected = [
            0, 31, 62, 93, 124, 155, 186, 217, 248, 23, 54, 85, 116, 147, 178, 209, 0, 17, 48, 79,
            110, 141, 172, 203, 234, 9, 40, 71, 102, 133, 164, 195, 0, 1, 34, 65, 96, 127, 158,
            189, 220, 251, 26, 57, 88, 119, 150, 181, 0, 243, 18, 0, 82, 113, 144, 175, 206, 237,
            12, 43, 74, 105, 136, 167, 0, 229, 4, 35, 68, 0, 130, 161, 192, 223, 254, 29, 60, 91,
            122, 153, 0, 0, 246, 21, 52, 85, 116, 0, 178, 209, 240, 15, 46, 77, 108, 139, 0, 0,
            232, 7, 38, 69, 102, 133, 164, 0, 226, 1, 32, 63, 94, 125, 156, 0, 218, 249, 24, 55,
            86, 119, 150, 181, 212, 0, 18, 49, 80, 111, 142, 173, 204, 235, 10, 41, 72, 103, 0,
            167, 198, 229, 4, 0, 66, 97, 128, 159, 190, 221, 252, 27, 58, 89, 120, 0, 0, 215, 246,
            21, 52, 0, 114, 0, 176, 207, 238, 13, 44, 75, 106, 137, 0, 0, 0, 7, 38, 69, 100, 0,
            162, 193, 224, 255, 30, 61, 92, 123, 0,
        ];
        assert_eq!(
            output[0].pixels.pixels().map(|p| p[3]).collect::<Vec<_>>(),
            expected
        );
        assert!(output[0].pixels.pixels().all(|p| p.0[..3] == [30, 70, 100]));
        assert_eq!(source.pixels, original);
    }
    #[test]
    fn manual_combined_transform_matches_fixed_release_rgba_without_recentering() {
        let source = frame(5, 3, |x, y| {
            [
                (x * 30 + 10) as u8,
                (y * 50 + 20) as u8,
                90,
                ((x * 31 + y * 17) % 256) as u8,
            ]
        });
        let first = apply(
            &[source],
            json!({"frames":[{"sourceIndex":0,"rotation":37,
            "scale":0.8,"opacity":0.5,"x":0.5,"y":-0.5,"durationMs":240}]}),
        );
        let first_expected = [
            0, 0, 0, 0, 10, 120, 90, 17, 40, 70, 90, 24, 70, 20, 90, 31, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 70, 120, 90, 48, 100, 70, 90, 55, 130, 20, 90, 62, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 130, 120, 90, 79, 0, 0, 0, 0,
        ];
        assert_eq!(first[0].pixels.as_raw(), &first_expected);
        let second = apply(
            &first,
            json!({"frames":[{"sourceIndex":0,"rotation":-90,
            "scale":1.5,"opacity":0.75,"x":-0.5,"y":0.25,"durationMs":300}]}),
        );
        let second_expected = [
            70, 20, 90, 23, 100, 70, 90, 41, 100, 70, 90, 41, 130, 120, 90, 59, 0, 0, 0, 0, 40, 70,
            90, 18, 70, 120, 90, 36, 70, 120, 90, 36, 0, 0, 0, 0, 0, 0, 0, 0, 40, 70, 90, 18, 70,
            120, 90, 36, 70, 120, 90, 36, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(second[0].pixels.as_raw(), &second_expected);
        assert_eq!(
            (second[0].pixels.width(), second[0].pixels.height()),
            (5, 3)
        );
        assert_eq!(
            second[0].wire,
            json!({"width":5,"height":3,"durationMs":300,
            "action":"walk","direction":"S","columns":1,"rows":1,"frameCount":1})
        );
    }
    #[test]
    fn manual_sparse_capsules_restore_original_alpha_and_preserve_transparent_rgb() {
        let source = frame(16, 16, |x, _| [30, 70, 100, if x == 8 { 0 } else { 200 }]);
        let stroke = json!({"radius":0.04,"points":[{"x":1.5/16.0,"y":8.5/16.0},
            {"x":14.5/16.0,"y":8.5/16.0}]});
        let erased = apply(
            &[source.clone()],
            json!({"frames":[{"sourceIndex":0,"eraseStrokes":[stroke.clone()]}]}),
        );
        for x in 1..15 {
            assert_eq!(erased[0].pixels.get_pixel(x, 8).0, [30, 70, 100, 0]);
        }
        assert_eq!(erased[0].pixels.get_pixel(2, 7).0, [30, 70, 100, 200]);
        let mut restore = stroke.clone();
        restore["restore"] = json!(true);
        let restored = apply(
            &[source.clone()],
            json!({"frames":[{"sourceIndex":0,"eraseStrokes":[stroke,restore]}]}),
        );
        assert_eq!(restored[0].pixels, source.pixels);
        let transparent = apply(&[source], json!({"frames":[{"sourceIndex":0,"opacity":0}]}));
        assert!(transparent[0]
            .pixels
            .pixels()
            .all(|p| p.0 == [30, 70, 100, 0]));
        let diagonal = apply(
            &[frame(3, 3, |_, _| [10, 20, 30, 255])],
            json!({"frames":[{"sourceIndex":0,
            "eraseStrokes":[{"radius":0.001,"points":[{"x":1.0/6.0,"y":1.0/6.0},
            {"x":5.0/6.0,"y":5.0/6.0}]}]}]}),
        );
        for (x, y, pixel) in diagonal[0].pixels.enumerate_pixels() {
            assert_eq!(pixel[3], if x == y { 0 } else { 255 });
        }
    }
    #[test]
    fn manual_fixed_canvas_clips_overflow_and_nearest_rotation_keeps_exact_positions() {
        let source = frame(6, 6, |x, y| {
            if y == 2 && x == 2 {
                [200, 20, 50, 255]
            } else if y == 2 && x == 3 {
                [20, 70, 200, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let shifted = apply(
            &[source.clone()],
            json!({"frames":[{"sourceIndex":0,"x":1,"y":2}]}),
        );
        assert_eq!(shifted[0].pixels.get_pixel(3, 4).0, [200, 20, 50, 255]);
        assert_eq!(shifted[0].pixels.get_pixel(2, 2)[3], 0);
        let clipped = apply(
            &[source.clone()],
            json!({"frames":[{"sourceIndex":0,"x":8}]}),
        );
        assert!(clipped[0].pixels.pixels().all(|p| p[3] == 0));
        let rotated = apply(
            &[source],
            json!({"frames":[{"sourceIndex":0,"rotation":90,"scale":2,"opacity":0.5}]}),
        );
        for x in [3, 4] {
            for y in [1, 2] {
                assert_eq!(rotated[0].pixels.get_pixel(x, y).0, [200, 20, 50, 128]);
            }
            for y in [3, 4] {
                assert_eq!(rotated[0].pixels.get_pixel(x, y).0, [20, 70, 200, 128]);
            }
        }
        assert_eq!(rotated[0].pixels.get_pixel(2, 2)[3], 0);
    }
    #[test]
    fn manual_duplicate_drop_reorder_and_varied_dimensions_remain_independent() {
        let frames = vec![
            frame(4, 2, |_, _| [200, 30, 40, 255]),
            frame(4, 2, |_, _| [0, 255, 0, 255]),
            frame(3, 5, |_, _| [20, 70, 220, 255]),
        ];
        let output = apply(
            &frames,
            json!({"frames":[{"sourceIndex":2,"durationMs":240},
            {"sourceIndex":0,"opacity":0.5,"durationMs":500},
            {"sourceIndex":2,"durationMs":60,"eraseStrokes":[{"radius":0.25,"points":[{"x":0.5,"y":0.5}]}]}]}),
        );
        assert_eq!(
            output
                .iter()
                .map(|f| (f.pixels.width(), f.pixels.height()))
                .collect::<Vec<_>>(),
            vec![(3, 5), (4, 2), (3, 5)]
        );
        assert_eq!(
            output
                .iter()
                .map(|f| f.wire["durationMs"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![240, 500, 60]
        );
        assert_ne!(output[0].pixels, output[2].pixels);
        assert_eq!(frames[0].pixels.get_pixel(0, 0).0, [200, 30, 40, 255]);
        assert!(output
            .iter()
            .flat_map(|f| f.pixels.pixels())
            .all(|p| p.0 != [0, 255, 0, 255]));
    }
    #[test]
    fn manual_budget_and_cancel_reject_without_mutating_source_pixels() {
        let source = frame(3, 3, |_, _| [30, 70, 100, 200]);
        let edits =
            image_protocol::frame_edits(&json!({"frames":[{"sourceIndex":0,"eraseStrokes":[
            {"radius":0.001,"points":[{"x":0.5,"y":0.5}]},
            {"radius":0.001,"points":[{"x":0.5,"y":0.5}]}]}]}))
            .unwrap();
        let mut work = 4;
        assert_eq!(
            alpha_strokes(
                &source.pixels,
                &edits[0].erase_strokes,
                &mut work,
                &RunCancellation::default()
            )
            .unwrap_err()
            .code,
            "workflow_image_too_large"
        );
        assert!(source.pixels.pixels().all(|p| p.0 == [30, 70, 100, 200]));
        let cancelled = RunCancellation::default();
        cancelled.cancel();
        assert_eq!(
            edit_frames(&[source], &edits, &cancelled)
                .err()
                .unwrap()
                .code,
            "workflow_image_cancelled"
        );
        let large = frame(256, 256, |_, _| [10, 20, 30, 255]);
        let edits =
            image_protocol::frame_edits(&json!({"frames":vec![json!({"sourceIndex":0});256]}))
                .unwrap();
        assert_eq!(
            edit_frames(&[large], &edits, &RunCancellation::default())
                .err()
                .unwrap()
                .code,
            "workflow_image_too_large"
        );
    }
}
