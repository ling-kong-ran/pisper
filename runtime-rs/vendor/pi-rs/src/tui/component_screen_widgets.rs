//! Fullscreen flash composition and the jump-to-end indicator from
//! `tui-alt-screen.ts:1018-1024,1622-1656`. These are ordered render stages,
//! not an OS host or the complete search/overlay/selection/diff compositor.
//!
//! Geometry remains signed. A negative JS array key is exposed separately
//! from visible rows; it is not silently painted at row zero. Integer cell
//! sizes must fit usize; arbitrary JS numbers and array identity are outside
//! this API. Label callbacks run synchronously and must not reenter the owner.
use crate::tui::component::Component;
use crate::tui::components::alt_screen_flash::AltScreenFlashContainer;
use crate::tui::components::scroll_view::ScrollHandle;
use crate::tui::layout::{get_scroll_view_box_id, get_scrollbar_geometry, LayoutFrame};
use crate::tui::overlay::composite_tui_line;
use crate::tui::terminal_image::is_image_line;
use crate::tui::utils::{
    extract_segments, slice_by_column, slice_with_width, truncate_to_width, visible_width,
};
use crate::tui::viewport_mouse::SgrMouseEvent;

/// Last published hit rectangle, not a prediction from the next layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollToEndIndicatorRect {
    pub row: i128,
    pub column: i128,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndicatorOutput {
    pub screen: Vec<String>,
    /// Upstream can assign result[-1] for a manually supplied offscreen frame.
    /// Such a named array property is not part of its length or visible rows.
    pub negative_row: Option<(i128, String)>,
}
impl IndicatorOutput {
    fn unchanged(screen: &[String]) -> Self {
        Self {
            screen: screen.to_vec(),
            negative_row: None,
        }
    }
}

// Reuse the verified compositor for ordinary geometry. For a negative column,
// the source's before range is empty. Nonnegative grapheme columns let us
// normalize *extraction thresholds* while retaining signed afterStart = x+w.
// This is NOT composite_tui_line(..., max(0,x), ...), which loses that overlap.
fn composite_signed(
    base: &str,
    overlay: &str,
    column: i128,
    overlay_width: usize,
    width: usize,
) -> String {
    if column >= 0 {
        return composite_tui_line(
            base,
            overlay,
            usize::try_from(column).expect("cell column fits usize"),
            overlay_width,
            width,
        );
    }
    if is_image_line(base) {
        return base.into();
    }
    let after_start =
        usize::try_from((column + overlay_width as i128).max(0)).expect("cell column fits usize");
    let segments = extract_segments(
        base,
        0,
        after_start,
        width.saturating_sub(after_start),
        true,
    );
    let (text, text_width) = slice_with_width(overlay, 0, overlay_width, true);
    const RESET: &str = "\x1b[0m\x1b]8;;\x07";
    let after_pad = width
        .saturating_sub(overlay_width.max(text_width))
        .saturating_sub(segments.after_width);
    let result = format!(
        "{RESET}{text}{}{RESET}{}{}",
        " ".repeat(overlay_width.saturating_sub(text_width)),
        segments.after,
        " ".repeat(after_pad)
    );
    if visible_width(&result) <= width {
        result
    } else {
        slice_by_column(&result, 0, width, true)
    }
}

/// Invoke after selection painting. A zero height means JS slice(-0), i.e.
/// all flash rows, not no rows. An empty stack leaves even a short screen alone.
pub fn composite_flashes(
    screen: &[String],
    flashes: &mut AltScreenFlashContainer,
    width: usize,
    height: usize,
) -> Vec<String> {
    let lines = flashes.render(width);
    let start = if height == 0 {
        0
    } else {
        lines.len().saturating_sub(height)
    };
    let lines = &lines[start..];
    if lines.is_empty() {
        return screen.to_vec();
    }
    let mut result = screen.to_vec();
    result.resize_with(result.len().max(height), String::new);
    for (row, line) in lines.iter().enumerate() {
        let flash_width = visible_width(line);
        if flash_width == 0 {
            continue;
        }
        if row >= result.len() {
            result.resize_with(row + 1, String::new);
        }
        result[row] = composite_signed(
            &result[row],
            line,
            width as i128 - flash_width as i128,
            flash_width,
            width,
        );
    }
    result
}

#[derive(Debug, Default)]
pub struct ScrollToEndIndicator {
    rect: Option<ScrollToEndIndicatorRect>,
}
impl ScrollToEndIndicator {
    pub fn rect(&self) -> Option<ScrollToEndIndicatorRect> {
        self.rect
    }

    /// Clears the published rectangle on every draw, including empty labels,
    /// failed callbacks, missing boxes and image rows. Labels are requested only
    /// after the follow/clip/row checks. Error propagation does not fall back.
    pub fn composite<E>(
        &mut self,
        screen: &[String],
        layout: &LayoutFrame,
        implicit: &ScrollHandle,
        width: usize,
        label: Option<&mut dyn FnMut() -> Result<String, E>>,
    ) -> Result<IndicatorOutput, E> {
        self.rect = None;
        let mut output = IndicatorOutput::unchanged(screen);
        let scroll = layout.primary_scroll_view.as_ref().unwrap_or(implicit);
        let Some(label) = label else {
            return Ok(output);
        };
        if !scroll.follow_end() || scroll.snapshot().following_end {
            return Ok(output);
        }
        let Some(index) = get_scroll_view_box_id(layout, scroll) else {
            return Ok(output);
        };
        let clip = layout.boxes[index].clip;
        if clip.width == 0 || clip.height == 0 {
            return Ok(output);
        }
        let row = clip.y as i128 + clip.height as i128 - 1;
        if row >= screen.len() as i128 {
            return Ok(output);
        }
        let base = usize::try_from(row)
            .ok()
            .and_then(|r| screen.get(r))
            .map_or("", String::as_str);
        if is_image_line(base) {
            return Ok(output);
        }
        let end = get_scrollbar_geometry(layout, index, false)
            .map_or(clip.x as i128 + clip.width as i128, |g| g.column as i128);
        let available_width =
            usize::try_from((end - clip.x as i128).max(0)).expect("cell width fits usize");
        let text = truncate_to_width(&label()?, available_width, "", false);
        let text_width = visible_width(&text);
        if text_width == 0 {
            return Ok(output);
        }
        let column = clip.x as i128 + (available_width as i128 - text_width as i128).div_euclid(2);
        let line = composite_signed(base, &text, column, text_width, width);
        if row < 0 {
            output.negative_row = Some((row, line));
        } else {
            output.screen[row as usize] = line;
        }
        self.rect = Some(ScrollToEndIndicatorRect {
            row,
            column,
            width: text_width,
        });
        Ok(output)
    }

    /// Hit testing uses the last published rect, but scrollToBottom targets the
    /// CURRENT primary (or implicit) scroll. ScrollHandle's own notification
    /// runs before the explicit render request, even if that produces two calls.
    /// A handled click does not clear the rect; only a subsequent draw does.
    pub fn handle_mouse_event(
        &self,
        event: SgrMouseEvent,
        current_layout: Option<&LayoutFrame>,
        implicit: &ScrollHandle,
        request_render: impl FnOnce(),
    ) -> bool {
        let Some(rect) = self.rect else {
            return false;
        };
        if event.release
            || event.button & 32 != 0
            || event.button & 3 != 0
            || event.y as i128 != rect.row
            || (event.x as i128) < rect.column
            || event.x as i128 >= rect.column + rect.width as i128
        {
            return false;
        }
        current_layout
            .and_then(|f| f.primary_scroll_view.as_ref())
            .unwrap_or(implicit)
            .scroll_to_end();
        request_render();
        true
    }
}
