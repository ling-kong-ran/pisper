//! OpenCV 5.0.0 Telea 三通道 radius 3 的 Rust 移植，保留 release 的像素排序与原始 alpha。
//! 上游： https://github.com/opencv/opencv/blob/5.0.0/modules/photo/src/inpaint.cpp
/*
Intel License Agreement For Open Source Computer Vision Library
Copyright (C) 2000, Intel Corporation, all rights reserved.
Third party copyrights are property of their respective owners.
Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:
* Redistributions of source code must retain the above copyright notice,
  this list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.
* The name of Intel Corporation may not be used to endorse or promote products
  derived from this software without specific prior written permission.
This software is provided by the copyright holders and contributors "as is" and
any express or implied warranties, including, but not limited to, the implied
warranties of merchantability and fitness for a particular purpose are disclaimed.
In no event shall the Intel Corporation or contributors be liable for any direct,
indirect, incidental, special, exemplary, or consequential damages
(including, but not limited to, procurement of substitute goods or services;
loss of use, data, or profits; or business interruption) however caused
and on any theory of liability, whether in contract, strict liability,
or tort (including negligence or otherwise) arising in any way out of
the use of this software, even if advised of the possibility of such damage.
*/
use crate::{
    native_workflow::{Result, WorkflowError},
    workflow_engine::RunCancellation,
};
use image::RgbaImage;
use serde_json::Value;
use std::{cmp::Ordering, collections::BinaryHeap};
const KNOWN: u8 = 0;
const BAND: u8 = 1;
const INSIDE: u8 = 2;
const CHANGE: u8 = 3;
#[derive(Clone, Copy)]
struct Entry {
    distance: f32,
    row: usize,
    col: usize,
    order: usize,
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.order == other.order
    }
}
impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.order.cmp(&self.order))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
#[derive(Default)]
struct Queue {
    entries: BinaryHeap<Entry>,
    next: usize,
}
impl Queue {
    fn push(&mut self, row: usize, col: usize, distance: f32) {
        self.entries.push(Entry {
            distance,
            row,
            col,
            order: self.next,
        });
        self.next += 1;
    }
    fn pop(&mut self) -> Option<Entry> {
        self.entries.pop()
    }
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
fn solve(a: usize, b: usize, flags: &[u8], time: &[f32]) -> f32 {
    let left = f64::from(time[a]);
    let right = f64::from(time[b]);
    let minimum = left.min(right);
    (if flags[a] != INSIDE {
        if flags[b] != INSIDE {
            if (left - right).abs() >= 1.0 {
                1.0 + minimum
            } else {
                (left + right + (2.0 - (left - right) * (left - right)).sqrt()) * 0.5
            }
        } else {
            1.0 + left
        }
    } else if flags[b] != INSIDE {
        1.0 + right
    } else {
        1.0 + minimum
    }) as f32
}
fn distance(row: usize, col: usize, width: usize, flags: &[u8], time: &[f32]) -> f32 {
    let at = |row, col| row * width + col;
    solve(at(row - 1, col), at(row, col - 1), flags, time)
        .min(solve(at(row + 1, col), at(row, col - 1), flags, time))
        .min(solve(at(row - 1, col), at(row, col + 1), flags, time))
        .min(solve(at(row + 1, col), at(row, col + 1), flags, time))
}
fn neighbors(row: usize, col: usize) -> [(usize, usize); 4] {
    [
        (row - 1, col),
        (row, col - 1),
        (row + 1, col),
        (row, col + 1),
    ]
}
fn outward(
    flags: &mut [u8],
    time: &mut [f32],
    width: usize,
    height: usize,
    queue: &mut Queue,
    cancellation: &RunCancellation,
) -> Result<()> {
    while let Some(entry) = queue.pop() {
        active(cancellation)?;
        flags[entry.row * width + entry.col] = CHANGE;
        for (row, col) in neighbors(entry.row, entry.col) {
            if row == 0 || col == 0 || row >= height - 1 || col >= width - 1 {
                continue;
            }
            let index = row * width + col;
            if flags[index] == INSIDE {
                let value = distance(row, col, width, flags, time);
                time[index] = value;
                flags[index] = BAND;
                queue.push(row, col, value);
            }
        }
    }
    for (index, flag) in flags.iter_mut().enumerate() {
        if *flag == CHANGE {
            *flag = KNOWN;
            time[index] = -time[index];
        }
    }
    Ok(())
}
fn gradient(row: usize, col: usize, width: usize, flags: &[u8], time: &[f32]) -> (f32, f32) {
    let at = |row, col| row * width + col;
    let current = time[at(row, col)];
    let x = if flags[at(row, col + 1)] != INSIDE {
        if flags[at(row, col - 1)] != INSIDE {
            (time[at(row, col + 1)] - time[at(row, col - 1)]) * 0.5
        } else {
            time[at(row, col + 1)] - current
        }
    } else if flags[at(row, col - 1)] != INSIDE {
        current - time[at(row, col - 1)]
    } else {
        0.0
    };
    let y = if flags[at(row + 1, col)] != INSIDE {
        if flags[at(row - 1, col)] != INSIDE {
            (time[at(row + 1, col)] - time[at(row - 1, col)]) * 0.5
        } else {
            time[at(row + 1, col)] - current
        }
    } else if flags[at(row - 1, col)] != INSIDE {
        current - time[at(row - 1, col)]
    } else {
        0.0
    };
    (x, y)
}
pub(super) fn inpaint(
    mut pixels: RgbaImage,
    region: &Value,
    cancellation: &RunCancellation,
) -> Result<RgbaImage> {
    active(cancellation)?;
    let image_width = pixels.width() as usize;
    let image_height = pixels.height() as usize;
    if image_width == 0 || image_height == 0 {
        return Err(WorkflowError::coded(
            "workflow_image_invalid",
            "图片尺寸无效。",
        ));
    }
    let number = |field: &str| {
        region[field]
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 100.0)
            .ok_or_else(|| WorkflowError::coded("workflow_image_invalid", "修复区域无效。"))
    };
    let x = ((number("x")? * image_width as f64 / 100.0).floor() as usize).min(image_width - 1);
    let y = ((number("y")? * image_height as f64 / 100.0).floor() as usize).min(image_height - 1);
    let right = (x
        + (number("width")? * image_width as f64 / 100.0)
            .round()
            .max(1.0) as usize)
        .min(image_width);
    let bottom = (y
        + (number("height")? * image_height as f64 / 100.0)
            .round()
            .max(1.0) as usize)
        .min(image_height);
    let width = image_width + 2;
    let height = image_height + 2;
    let mut mask = vec![KNOWN; width * height];
    let mut time = vec![1e6_f32; width * height];
    for row in y..bottom {
        for col in x..right {
            mask[(row + 1) * width + col + 1] = INSIDE;
        }
    }
    let mut band = vec![false; width * height];
    let mut queue = Queue::default();
    for row in 1..height - 1 {
        active(cancellation)?;
        for col in 1..width - 1 {
            if mask[row * width + col] == KNOWN
                && neighbors(row, col)
                    .iter()
                    .any(|&(r, c)| mask[r * width + c] == INSIDE)
            {
                band[row * width + col] = true;
                time[row * width + col] = 0.0;
                queue.push(row, col, 0.0);
            }
        }
    }
    if queue.entries.is_empty() {
        return Ok(pixels);
    }
    let mut outer = vec![KNOWN; width * height];
    let mut outer_queue = Queue::default();
    for row in 1..height - 1 {
        active(cancellation)?;
        for col in 1..width - 1 {
            let index = row * width + col;
            if band[index] {
                outer_queue.push(row, col, 0.0);
            } else if mask[index] == KNOWN {
                let close = (row.saturating_sub(3)..=(row + 3).min(height - 1)).any(|r| {
                    (col.saturating_sub(3)..=(col + 3).min(width - 1))
                        .any(|c| mask[r * width + c] == INSIDE)
                });
                if close {
                    outer[index] = INSIDE;
                }
            }
        }
    }
    outward(
        &mut outer,
        &mut time,
        width,
        height,
        &mut outer_queue,
        cancellation,
    )?;
    // 上游计算完成 RGB 后才向外复制掩码区域；alpha 始终取原像素。
    while let Some(entry) = queue.pop() {
        active(cancellation)?;
        mask[entry.row * width + entry.col] = KNOWN;
        for (row, col) in neighbors(entry.row, entry.col) {
            if row == 0 || col == 0 || row >= height - 1 || col >= width - 1 {
                continue;
            }
            let index = row * width + col;
            if mask[index] != INSIDE {
                continue;
            }
            let dist = distance(row, col, width, &mask, &time);
            time[index] = dist;
            let (grad_x, grad_y) = gradient(row, col, width, &mask, &time);
            let mut intensity = [0_f32; 3];
            let mut jx = [0_f32; 3];
            let mut jy = [0_f32; 3];
            let mut weights = [1e-20_f32; 3];
            let pixel = |r: isize, c: isize, channel: usize| {
                f32::from(
                    pixels.get_pixel(
                        c.clamp(0, image_width as isize - 1) as u32,
                        r.clamp(0, image_height as isize - 1) as u32,
                    )[channel],
                )
            };
            for k in row.saturating_sub(3)..=(row + 3).min(height - 1) {
                for l in col.saturating_sub(3)..=(col + 3).min(width - 1) {
                    if k == 0
                        || l == 0
                        || k >= height - 1
                        || l >= width - 1
                        || mask[k * width + l] == INSIDE
                    {
                        continue;
                    }
                    let ry = row as f32 - k as f32;
                    let rx = col as f32 - l as f32;
                    let square = rx * rx + ry * ry;
                    if square == 0.0 || square > 9.0 {
                        continue;
                    }
                    let spatial = (1.0 / (f64::from(square) * f64::from(square).sqrt())) as f32;
                    let level =
                        (1.0 / (1.0 + f64::from((time[k * width + l] - time[index]).abs()))) as f32;
                    let mut direction = rx * grad_x + ry * grad_y;
                    if direction.abs() <= 0.01 {
                        direction = 1e-6;
                    }
                    let weight = (spatial * level * direction).abs();
                    let km = k as isize - 1 + isize::from(k == 1);
                    let kp = k as isize - 1 - isize::from(k == height - 2);
                    let lm = l as isize - 1 + isize::from(l == 1);
                    let lp = l as isize - 1 - isize::from(l == width - 2);
                    for channel in 0..3 {
                        let gx = if mask[k * width + l + 1] != INSIDE {
                            if mask[k * width + l - 1] != INSIDE {
                                (pixel(km, lp + 1, channel) - pixel(km, lm - 1, channel)) * 2.0
                            } else {
                                pixel(km, lp + 1, channel) - pixel(km, lm, channel)
                            }
                        } else if mask[k * width + l - 1] != INSIDE {
                            pixel(km, lp, channel) - pixel(km, lm - 1, channel)
                        } else {
                            0.0
                        };
                        let gy = if mask[(k + 1) * width + l] != INSIDE {
                            if mask[(k - 1) * width + l] != INSIDE {
                                (pixel(kp + 1, lm, channel) - pixel(km - 1, lm, channel)) * 2.0
                            } else {
                                pixel(kp + 1, lm, channel) - pixel(km, lm, channel)
                            }
                        } else if mask[(k - 1) * width + l] != INSIDE {
                            pixel(kp, lm, channel) - pixel(km - 1, lm, channel)
                        } else {
                            0.0
                        };
                        intensity[channel] +=
                            weight * pixel(k as isize - 1, l as isize - 1, channel);
                        jx[channel] -= weight * (gx * rx);
                        jy[channel] -= weight * (gy * ry);
                        weights[channel] += weight;
                    }
                }
            }
            for channel in 0..3 {
                let value = (f64::from(intensity[channel] / weights[channel])
                    + f64::from(jx[channel] + jy[channel])
                        / (f64::from(jx[channel] * jx[channel] + jy[channel] * jy[channel]).sqrt()
                            + f64::from(1e-20_f32))) as f32;
                pixels.get_pixel_mut((col - 1) as u32, (row - 1) as u32)[channel] =
                    (f64::from(value) + 0.5).round_ties_even().clamp(0.0, 255.0) as u8;
            }
            mask[index] = BAND;
            queue.push(row, col, dist);
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixels_match_fixed_release_opencv5_oracle_for_interior_and_border_masks() {
        // 来自 release 固定 SHA b873c8… OpenCV.js 的一次有界 oracle；测试运行无需 JS/网络。
        for (width, height, x, y, w, h, expected) in [
            (
                9_u32,
                8_u32,
                3_u32,
                2_u32,
                3_u32,
                4_u32,
                vec![
                    55, 38, 81, 76, 39, 122, 99, 45, 157, 67, 61, 89, 93, 70, 131, 111, 68, 163,
                    83, 92, 97, 104, 90, 137, 125, 99, 168, 97, 116, 106, 116, 116, 143, 137, 123,
                    173,
                ],
            ),
            (
                7,
                7,
                0,
                0,
                3,
                3,
                vec![
                    66, 57, 89, 67, 58, 89, 71, 47, 103, 68, 57, 90, 68, 58, 89, 67, 40, 102, 68,
                    63, 86, 63, 67, 73, 75, 63, 100,
                ],
            ),
        ] {
            let pixels = RgbaImage::from_fn(width, height, |col, row| {
                image::Rgba([
                    if col >= x && col < x + w && row >= y && row < y + h {
                        0
                    } else {
                        ((col * 17 + row * 11) % 256) as u8
                    },
                    if col >= x && col < x + w && row >= y && row < y + h {
                        0
                    } else {
                        ((col * 3 + row * 23) % 256) as u8
                    },
                    if col >= x && col < x + w && row >= y && row < y + h {
                        0
                    } else {
                        ((col * 31 + row * 5) % 256) as u8
                    },
                    ((col + row) * 11) as u8,
                ])
            });
            let region = serde_json::json!({"x":(x as f64+0.01)*100.0/width as f64,"y":(y as f64+0.01)*100.0/height as f64,"width":w as f64*100.0/width as f64,"height":h as f64*100.0/height as f64});
            let actual = inpaint(pixels.clone(), &region, &RunCancellation::default()).unwrap();
            let mut found = Vec::new();
            for row in y..y + h {
                for col in x..x + w {
                    found.extend_from_slice(&actual.get_pixel(col, row).0[..3]);
                }
            }
            assert_eq!(found, expected);
            for (col, row, before) in pixels.enumerate_pixels() {
                assert_eq!(actual.get_pixel(col, row)[3], before[3]);
                if !(col >= x && col < x + w && row >= y && row < y + h) {
                    assert_eq!(actual.get_pixel(col, row), before);
                }
            }
        }
    }
    #[test]
    fn telea_repairs_mask_preserves_every_alpha_and_outside_rgb() {
        let pixels = RgbaImage::from_fn(9, 9, |x, y| {
            image::Rgba([
                if x == 4 && y == 4 { 0 } else { 100 },
                if x == 4 && y == 4 { 0 } else { 120 },
                if x == 4 && y == 4 { 0 } else { 140 },
                (x + y) as u8 * 11,
            ])
        });
        let result = inpaint(
            pixels.clone(),
            &serde_json::json!({"x":44.5,"y":44.5,"width":1,"height":1}),
            &RunCancellation::default(),
        )
        .unwrap();
        assert_eq!(result.get_pixel(4, 4).0[..3], [100, 120, 140]);
        for (x, y, pixel) in pixels.enumerate_pixels() {
            assert_eq!(result.get_pixel(x, y)[3], pixel[3]);
            if x != 4 || y != 4 {
                assert_eq!(result.get_pixel(x, y), pixel);
            }
        }
    }
    #[test]
    fn whole_mask_and_cancelled_processing_are_bounded() {
        let pixels = RgbaImage::from_pixel(2, 2, image::Rgba([12, 22, 32, 42]));
        assert_eq!(
            inpaint(
                pixels.clone(),
                &serde_json::json!({"x":0,"y":0,"width":100,"height":100}),
                &RunCancellation::default()
            )
            .unwrap(),
            pixels
        );
        let cancellation = RunCancellation::default();
        cancellation.cancel();
        assert_eq!(
            inpaint(
                pixels,
                &serde_json::json!({"x":0,"y":0,"width":1,"height":1}),
                &cancellation
            )
            .unwrap_err()
            .code,
            "workflow_image_cancelled"
        );
    }
}
