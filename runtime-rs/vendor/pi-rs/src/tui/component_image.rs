//! Full port of upstream `packages/tui/src/components/image.ts` (127 lines,
//! sha256 cdef2f757008ce2fecfde6c0108366ebdc160bac70355db855fd264aec8d53f8):
//! the inline-image `Component` on top of the already-ported
//! [`crate::tui::terminal_image`] layer. It renders through the Kitty
//! graphics protocol, OSC 1337 (iTerm2) or an ANSI-colored text fallback,
//! pads the returned line vector to the placement height so TUI cursor
//! accounting stays inside the scroll area, caches per render width and
//! reallocates/reuses Kitty image ids exactly like the source.
//!
//! Structure map (upstream -> here):
//! - `ImageTheme` -> [`ImageTheme`] (callback boxed in an `Arc`, like the
//!   other component ports)
//! - `ImageOptions` -> [`ImageOptions`] (`maxWidthCells`/`maxHeightCells`
//!   stay JS numbers as `f64`)
//! - `Image`/`getImageId`/`invalidate`/`render` -> [`Image`] +
//!   [`Image::get_image_id`] + [`Component::invalidate`] +
//!   [`Component::render`]
//!
//! Behavior notes pinned to the source:
//! - `dimensions || getImageDimensions(..) || {800,600}`: an explicitly
//!   passed dimension object always wins, then the mime-dispatched header
//!   probe, then the 800x600 default.
//! - `maxWidth = Math.max(1, Math.min(width - 2, options.maxWidthCells ?? 60))`
//!   and `defaultMaxHeight = Math.max(1, Math.ceil(maxWidth * cellW / cellH))`
//!   use JS `Math.min`/`Math.max` NaN semantics (`width - 2` may go negative
//!   for width < 2; the `f64` math absorbs that without usize underflow).
//! - Kitty allocates an id only when `imageId === undefined`; the render
//!   result's id is adopted only when truthy (`if (result.imageId)`), so an
//!   explicit id of 0 survives untouched (upstream `encodeKitty` itself treats
//!   0 as falsy and omits `i=`, byte-verified by the terminal-image oracle).
//! - Kitty emits `[sequence, "", "", ...]` (C=1 keeps the cursor put); other
//!   protocols emit `(rows-1)` empty lines and then `\x1b[<rows-1>A` +
//!   sequence, moving the cursor back up before drawing so the TUI cursor
//!   accounting stays inside the scroll area.
//! - Rendered lines are cached and reused only while the width is unchanged;
//!   [`Component::invalidate`] drops the cache (the TUI calls it on every
//!   component on resize, which is the resize re-render path).
//!
//! Byte-for-byte oracle evidence: `tests/fixtures/component_image_oracle/oracle.mjs`
//! runs the real upstream component (with the real `terminal-image.ts` and
//! `utils.ts`) under node and captured 19 scenarios / 27 render probes;
//! `tests/component_image.rs` replays every probe byte-for-byte. The only
//! masked bytes are randomly allocated Kitty ids (`Math.random` upstream):
//! those rows are compared with `,i=<ID>` masking plus the contractual
//! integer range [1, 0xffffffff].
//!
//! Disclosed substitutions:
//! - JS classes implement `Component` structurally; here [`Image`] implements
//!   the [`Component`] trait (`render(&mut self)` because rendering mutates
//!   the id/cache state).
//! - The render width is the trait's `usize` (upstream accepts any JS number);
//!   the `- 2` negotiation runs in `f64` so widths below 2 still clamp to a
//!   1-cell minimum instead of underflowing.
//! - `ImageTheme.fallbackColor` is `Arc<dyn Fn(&str) -> String + Send + Sync>`
//!   instead of a JS closure property.
//! - Upstream holds `options`/`theme` by reference and reads `options.filename`
//!   lazily at render time; the port owns them, so runtime mutation of the
//!   options object is not observable.

use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::tui::component::Component;
use crate::tui::terminal_image::{
    allocate_image_id, get_capabilities, get_cell_dimensions, get_image_dimensions,
    get_png_dimensions, image_fallback, render_image, ImageDimensions, ImageRenderOptions,
};
use crate::tui::utils::truncate_to_width;

/// Upstream `ImageTranscoder` (v1.0.0): converts base64 image data to base64
/// PNG data, or returns `None` if it cannot. Called synchronously during
/// rendering.
pub type ImageTranscoder = Arc<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;

static IMAGE_TRANSCODER: RwLock<Option<ImageTranscoder>> = RwLock::new(None);
/// Upstream module-level `pngCache`: backstop for callers that recreate
/// `Image` instances, keyed by source data, least recently used first.
/// One pngCache slot: source base64 -> converted PNG (None when the
/// transcoder failed).
type PngCacheEntry = (String, Option<String>);
static PNG_CACHE: OnceLock<Mutex<Vec<PngCacheEntry>>> = OnceLock::new();

const PNG_CACHE_LIMIT: usize = 32;

/// Upstream `setImageTranscoder` (v1.0.0): register the converter used for
/// non-PNG images on Kitty-protocol terminals, which only accept PNG.
/// Without one, such images render as text fallbacks.
pub fn set_image_transcoder(transcoder: Option<ImageTranscoder>) {
    *IMAGE_TRANSCODER
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = transcoder;
    if let Some(cache) = PNG_CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// Upstream `toPng`: the transcoder call memoized in the LRU cache.
fn to_png(base64_data: &str, mime_type: &str) -> Option<String> {
    let transcoder = IMAGE_TRANSCODER
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let cache = PNG_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cached = cache
        .iter()
        .find(|(key, _)| key == base64_data)
        .map(|(_, value)| value.clone());
    let png = match cached {
        Some(png) => png,
        // The upstream reads `cached === undefined ? imageTranscoder(..) : cached`,
        // so a cached `null` short-circuits and a miss converts.
        None => transcoder(base64_data, mime_type),
    };
    // Re-insert as most recently used.
    cache.retain(|(key, _)| key != base64_data);
    cache.push((base64_data.to_owned(), png.clone()));
    if cache.len() > PNG_CACHE_LIMIT {
        cache.remove(0);
    }
    png
}

/// Upstream `ImageTheme`.
#[derive(Clone)]
pub struct ImageTheme {
    /// Upstream `fallbackColor`: paints the plain-text fallback line.
    pub fallback_color: FallbackColorFn,
}

/// Upstream `fallbackColor: (str: string) => string`.
pub type FallbackColorFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

impl std::fmt::Debug for ImageTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageTheme")
            .field("fallback_color", &"Arc<dyn Fn(&str) -> String>")
            .finish()
    }
}

/// Upstream `ImageOptions`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageOptions {
    /// Maximum rendered width in cells (upstream default 60).
    pub max_width_cells: Option<f64>,
    /// Maximum rendered height in cells; defaults to the square-pixel box.
    pub max_height_cells: Option<f64>,
    /// File name shown (shortened/hyperlinked) by the text fallback.
    pub filename: Option<String>,
    /// Kitty image ID. If provided, reuses this ID (for animations/updates).
    pub image_id: Option<u64>,
}

/// Upstream `Image`: inline image component with per-width line caching.
#[derive(Clone)]
pub struct Image {
    base64_data: String,
    mime_type: String,
    theme: ImageTheme,
    options: ImageOptions,
    dimensions: ImageDimensions,
    image_id: Option<u64>,
    /// Converted PNG data for Kitty (v1.0.0). Failures are not stored so a
    /// later transcoder can retry.
    png_data: Option<String>,

    cached_lines: Option<Vec<String>>,
    cached_width: Option<usize>,
}

impl Image {
    /// Upstream `new Image(base64Data, mimeType, theme)` with default
    /// options and no explicit dimensions.
    pub fn new(base64_data: &str, mime_type: &str, theme: ImageTheme) -> Self {
        Self::with_options(base64_data, mime_type, theme, ImageOptions::default(), None)
    }

    /// Upstream `new Image(base64Data, mimeType, theme, options, dimensions?)`:
    /// `dimensions` wins over the header probe, which wins over 800x600.
    pub fn with_options(
        base64_data: &str,
        mime_type: &str,
        theme: ImageTheme,
        options: ImageOptions,
        dimensions: Option<ImageDimensions>,
    ) -> Self {
        let dimensions = dimensions
            .or_else(|| get_image_dimensions(base64_data, mime_type))
            .unwrap_or(ImageDimensions {
                width_px: 800,
                height_px: 600,
            });
        let image_id = options.image_id;
        Self {
            base64_data: base64_data.to_owned(),
            mime_type: mime_type.to_owned(),
            theme,
            options,
            dimensions,
            image_id,
            png_data: None,
            cached_lines: None,
            cached_width: None,
        }
    }

    /// Upstream `getImageId`: the Kitty image ID used by this image (if any).
    pub fn get_image_id(&self) -> Option<u64> {
        self.image_id
    }

    /// Upstream `render(width)` body, exposed for callers holding `&mut dyn
    /// Component`; [`Component::render`] forwards here.
    fn render_lines(&mut self, width: usize) -> Vec<String> {
        if self.cached_width == Some(width) {
            if let Some(cached) = &self.cached_lines {
                return cached.clone();
            }
        }

        let width_number = width as f64;
        let max_width = js_max(
            1.0,
            js_min(
                width_number - 2.0,
                self.options.max_width_cells.unwrap_or(60.0),
            ),
        );
        let cell_dimensions = get_cell_dimensions();
        let default_max_height = js_max(
            1.0,
            (max_width * cell_dimensions.width_px as f64 / cell_dimensions.height_px as f64).ceil(),
        );
        let max_height = self.options.max_height_cells.unwrap_or(default_max_height);

        let caps = get_capabilities();
        // v1.0.0: Kitty accepts only PNG, so a non-PNG source is converted
        // through the registered transcoder; conversion may apply EXIF
        // rotation, so the PNG's own dimensions are preferred. Without data
        // (no transcoder, or it failed) the image renders as a text fallback.
        let mut data: Option<String> = Some(self.base64_data.clone());
        let mut dimensions = self.dimensions;
        if caps.images == Some("kitty") && self.mime_type != "image/png" {
            if self.png_data.is_none() {
                self.png_data = to_png(&self.base64_data, &self.mime_type);
            }
            data = self.png_data.clone();
            if let Some(png) = &data {
                if let Some(png_dimensions) = get_png_dimensions(png) {
                    dimensions = png_dimensions;
                }
            }
        }
        let lines: Vec<String> = match caps.images.filter(|_| data.is_some()) {
            Some(protocol) => {
                let data = data.as_deref().expect("checked non-empty above");
                if protocol == "kitty" && self.image_id.is_none() {
                    self.image_id = Some(allocate_image_id());
                }
                let rendered = render_image(
                    data,
                    dimensions,
                    ImageRenderOptions {
                        max_width_cells: Some(max_width),
                        max_height_cells: Some(max_height),
                        image_id: self.image_id,
                        move_cursor: Some(false),
                        ..ImageRenderOptions::default()
                    },
                );
                match rendered {
                    Some(result) => {
                        // Store the image ID for later cleanup; the truthy
                        // check keeps an explicit id of 0 untouched.
                        if let Some(id) = result.image_id {
                            if id != 0 {
                                self.image_id = Some(id);
                            }
                        }
                        if protocol == "kitty" {
                            // For Kitty: C=1 prevents cursor movement.
                            // Don't need the cursor movement.
                            let mut lines = Vec::with_capacity(result.rows);
                            lines.push(result.sequence);
                            // Return `rows` lines so TUI accounts for image height.
                            // saturating_sub mirrors the JS loop under the
                            // disclosed NaN clamping (rows can only be 0 for
                            // degenerate non-finite negotiations; upstream's
                            // `i < NaN - 1` also adds zero padding rows).
                            lines.extend(vec![String::new(); result.rows.saturating_sub(1)]);
                            lines
                        } else {
                            // Return `rows` lines so TUI accounts for image height.
                            // First (rows-1) lines are empty and cleared before
                            // the image is drawn. Last line: move cursor back
                            // up, draw the image, then move back down so TUI
                            // cursor accounting stays inside the scroll area.
                            let row_offset = result.rows.saturating_sub(1);
                            let move_up = if row_offset > 0 {
                                format!("\x1b[{row_offset}A")
                            } else {
                                String::new()
                            };
                            let mut lines = vec![String::new(); row_offset];
                            lines.push(format!("{move_up}{}", result.sequence));
                            lines
                        }
                    }
                    None => self.fallback_lines(width),
                }
            }
            None => self.fallback_lines(width),
        };

        self.cached_lines = Some(lines.clone());
        self.cached_width = Some(width);

        lines
    }

    /// `imageFallback(...)` colored and truncated to the render width.
    fn fallback_lines(&self, width: usize) -> Vec<String> {
        let fallback = image_fallback(
            &self.mime_type,
            Some(self.dimensions),
            self.options.filename.as_deref(),
        );
        vec![truncate_to_width(
            &(self.theme.fallback_color)(&fallback),
            width,
            "...",
            false,
        )]
    }
}

/// `Math.max` with JS NaN propagation (same helper as terminal_image).
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min` with JS NaN propagation (same helper as terminal_image).
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

impl Component for Image {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_lines(width)
    }

    fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_width = None;
    }
}

#[cfg(test)]
#[path = "tests/component_image.rs"]
mod tests;
