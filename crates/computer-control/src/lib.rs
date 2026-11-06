//! Computer Use 的控制层：截屏、键鼠、判断目标是不是 Pisper 自己。
//! 不碰窗口界面（遮罩、小窗在 src-tauri 里），也不知道会话和审批。
//!
//! 坐标有两套：模型看到的是缩放后的截图像素，系统输入用的是屏幕坐标
//! （Windows、X11 是物理像素，macOS 是点）。转换都走 [`ScreenMap`]。
mod capture;
mod input;
mod own_window;
mod permissions;

pub use capture::{capture_screen, encode_jpeg, CapturedScreen, Frame};
pub use input::{parse_chord, Button, Chord, Controller, KeyName};
pub use own_window::{foreground_is_own, point_hits_own_window};
pub use permissions::{check_support, request_permissions, Support};

/// 把模型看到的截图坐标换算成屏幕坐标。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMap {
    pub image_width: u32,
    pub image_height: u32,
    pub screen_width: f64,
    pub screen_height: f64,
}

impl ScreenMap {
    /// 超出截图范围的坐标说明模型算错了，点到屏幕边上只会更糟，直接拒绝。
    pub fn to_screen(&self, x: f64, y: f64) -> Result<(i32, i32), String> {
        if !x.is_finite() || !y.is_finite() {
            return Err("The coordinates are not numbers.".into());
        }
        if x < 0.0
            || y < 0.0
            || x >= f64::from(self.image_width)
            || y >= f64::from(self.image_height)
        {
            return Err(format!(
                "({x}, {y}) is outside the {}x{} screenshot.",
                self.image_width, self.image_height
            ));
        }
        Ok((
            (x * self.screen_width / f64::from(self.image_width)).round() as i32,
            (y * self.screen_height / f64::from(self.image_height)).round() as i32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_coordinates_scale_to_the_screen() {
        // 4K 屏截图缩到 1440 宽。
        let map = ScreenMap {
            image_width: 1440,
            image_height: 810,
            screen_width: 3840.0,
            screen_height: 2160.0,
        };
        assert_eq!(map.to_screen(720.0, 405.0), Ok((1920, 1080)));
        assert_eq!(map.to_screen(0.0, 0.0), Ok((0, 0)));
        assert!(map.to_screen(1440.0, 10.0).is_err());
        assert!(map.to_screen(-1.0, 10.0).is_err());
        assert!(map.to_screen(f64::NAN, 10.0).is_err());
    }
}
