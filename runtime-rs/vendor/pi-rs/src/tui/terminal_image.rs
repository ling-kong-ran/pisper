//! Full port of upstream `packages/tui/src/terminal-image.ts` (696 lines,
//! sha256 f29572354977fd4ef4cc76878d31cce3cb5c45cebdfc9b5c49b50ad9cfb3171f):
//! terminal capability detection with the full environment tree and tmux
//! probe, cell dimension state, OSC 8 hyperlinks, image-line detection, the
//! Kitty transmission encoder + bounded metadata/placement/crop/deletion
//! registry (in [`kitty`]), the iTerm2 encoder, cell-size math, header-only
//! PNG/JPEG/GIF/WebP dimension probes, `renderImage` and the text fallback.
//!
//! Structure map (upstream -> here):
//! - `detectCapabilities`/`detectCapabilitiesFromEnvironment`/`probeTmuxHyperlinks`
//!   -> [`detect_capabilities`]/[`detect_with`]/[`probe_tmux_hyperlinks`]
//! - `getCapabilities`/`setCapabilityOverrides`/`setCapabilities`/
//!   `resetCapabilitiesCache` -> same-named snake_case fns
//! - `getCellDimensions`/`setCellDimensions` -> [`get_cell_dimensions`]/[`set_cell_dimensions`]
//! - `encodeKitty`/`registerKittyImageMetadata`/`getKittyImageMetadata`/
//!   `getKittyImagePlacement`/`cropKittyImageLine`/`deleteKittyImage`/
//!   `deleteAllKittyImages`/`deleteAllKittyPlacements` -> [`kitty`] submodule
//! - `encodeITerm2` -> [`encode_iterm2`]
//! - `calculateImageCellSize`/`calculateImageRows` -> [`calculate_image_cell_size`]/[`calculate_image_rows`]
//! - `getPngDimensions`/`getJpegDimensions`/`getGifDimensions`/`getWebpDimensions`/
//!   `getImageDimensions` -> [`get_png_dimensions`] family
//! - `renderImage` -> [`render_image`]
//! - `hyperlink`/`imageFallback`/`shortenImagePath` -> [`hyperlink`]/[`image_fallback`]/[`shorten_image_path`]
//! - `isImageLine`/`allocateImageId` -> [`is_image_line`]/[`allocate_image_id`]
//!
//! Byte-for-byte oracle evidence: `tests/fixtures/terminal_image_oracle/` runs the
//! real upstream module under node; `oracle_consts.rs` (mounted by
//! [`tests`]) embeds every captured scenario.
//!
//! Disclosed divergences, all unreachable through the image pipeline:
//! - Upstream slices JS strings (UTF-16 code units); byte-oriented APIs here
//!   assert ASCII base64 input, which is the only data callers produce.
//! - `detect_from_environment` takes the env lookup, tmux probe and
//!   `is_windows_console` as parameters; production wiring uses
//!   `std::env`, the real 250ms tmux spawn and `cfg!(windows)` — the
//!   injection exists so tests avoid mutating process-global state.
//! - `image_fallback`'s `pathToFileURL` is the `url` crate; rootless
//!   absolute paths (e.g. `\foo` on Windows, which node resolves against the
//!   current drive) produce a plain display path instead of a hyperlink.
//! - `calculate_image_cell_size` mirrors `Math.min/max` NaN propagation, but
//!   a resulting NaN/Infinity clamps via Rust's saturating float->int cast
//!   instead of staying NaN (unreachable with validated cell sizes).
//! - `images` capability stays `Option<&'static str>` ("kitty"/"iterm2") for
//!   source compatibility with existing consumers of this module.
//! - Upstream's `Image` component tests (components/image.ts) are a
//!   separate slice; no Rust `Image` component exists yet.

use std::sync::RwLock;
use url::Url;

pub mod base64;
pub mod kitty;

#[cfg(test)]
mod oracle_consts;
#[cfg(test)]
mod tests;

pub use crate::tui::colors::TerminalColorMode;
pub use kitty::{
    crop_kitty_image_line, delete_all_kitty_images, delete_all_kitty_placements,
    delete_kitty_image, encode_kitty, get_kitty_image_metadata, get_kitty_image_placement,
    get_kitty_image_placement_rows, register_kitty_image_metadata, KittyImageMetadata,
    KittyImagePlacement, KittyImageRegistry, KittyOptions,
};

/// Upstream `TerminalCapabilities`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCapabilities {
    /// OSC 8 hyperlink support.
    pub hyperlinks: bool,
    /// Inline image protocol (`Some("kitty" | "iterm2")` or `None`).
    pub images: Option<&'static str>,
    /// 24-bit color support.
    pub true_color: bool,
}

/// Upstream `Partial<TerminalCapabilities>`: `None` fields leave the
/// detected value untouched (the double option models TS `undefined` vs
/// explicit `null` for `images`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilityOverrides {
    /// `Some(None)` forces images off, `Some(Some("kitty"))` forces a protocol.
    pub images: Option<Option<&'static str>>,
    /// `Some(..)` forces truecolor on/off.
    pub true_color: Option<bool>,
    /// `Some(..)` forces hyperlinks on/off.
    pub hyperlinks: Option<bool>,
}

/// Upstream `CellDimensions` — pixel size of one terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellDimensions {
    /// Horizontal pixels per cell.
    pub width_px: usize,
    /// Vertical pixels per cell.
    pub height_px: usize,
}

impl Default for CellDimensions {
    fn default() -> Self {
        Self {
            width_px: 9,
            height_px: 18,
        }
    }
}

/// Upstream `ImageDimensions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDimensions {
    /// Image width in pixels.
    pub width_px: usize,
    /// Image height in pixels.
    pub height_px: usize,
}

/// Upstream `ImageCellSize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageCellSize {
    /// Placement width in cells.
    pub columns: usize,
    /// Placement height in cells.
    pub rows: usize,
}

/// Upstream `ImageRenderOptions`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageRenderOptions {
    /// Maximum rendered width in cells (default 80).
    pub max_width_cells: Option<f64>,
    /// Maximum rendered height in cells.
    pub max_height_cells: Option<f64>,
    /// iTerm2 aspect handling (default true).
    pub preserve_aspect_ratio: Option<bool>,
    /// Kitty image ID; reuses/replaces the existing image with this ID.
    pub image_id: Option<u64>,
    /// Whether Kitty should apply its default cursor movement after placement.
    pub move_cursor: Option<bool>,
}

/// Upstream `renderImage` return shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedImage {
    /// Control sequence to write to the terminal.
    pub sequence: String,
    /// Placement width in cells.
    pub columns: usize,
    /// Placement height in cells.
    pub rows: usize,
    /// Kitty image ID when one was requested.
    pub image_id: Option<u64>,
}

/// Upstream `encodeITerm2` options; `width`/`height` render like JS numbers
/// or strings (`"auto"`).
#[derive(Clone, Debug, Default)]
pub struct Iterm2Options<'a> {
    /// `width=` value (cell columns or a terminal-dependent string).
    pub width: Option<&'a str>,
    /// `height=` value (commonly `"auto"`).
    pub height: Option<&'a str>,
    /// File name, base64-encoded into `name=`.
    pub name: Option<&'a str>,
    /// `Some(false)` emits `preserveAspectRatio=0`.
    pub preserve_aspect_ratio: Option<bool>,
    /// Defaults to inline (1) like upstream.
    pub inline: Option<bool>,
}

static CACHED: RwLock<Option<TerminalCapabilities>> = RwLock::new(None);
static OVERRIDES: RwLock<Option<CapabilityOverrides>> = RwLock::new(None);
static CELL_DIMENSIONS: RwLock<CellDimensions> = RwLock::new(CellDimensions {
    width_px: 9,
    height_px: 18,
});

/// Upstream `getCellDimensions` — cell size, updated by the TUI once the
/// terminal answers the cell-size query.
pub fn get_cell_dimensions() -> CellDimensions {
    *CELL_DIMENSIONS.read().unwrap()
}

/// Upstream `setCellDimensions`.
pub fn set_cell_dimensions(dims: CellDimensions) {
    *CELL_DIMENSIONS.write().unwrap() = dims;
}

/// Upstream `setCapabilities` (test hook): pin the cached capabilities.
pub fn set_capabilities(caps: TerminalCapabilities) {
    *CACHED.write().unwrap() = Some(caps);
}

/// Upstream `resetCapabilitiesCache`.
pub fn reset_capabilities_cache() {
    *CACHED.write().unwrap() = None;
}

/// Upstream `setCapabilityOverrides`: overrides replace the cached
/// capabilities wholesale; the equality short-circuit keeps the existing
/// cache when nothing changed.
pub fn set_capability_overrides(overrides: CapabilityOverrides) {
    if let Some(current) = *OVERRIDES.read().unwrap() {
        if current == overrides {
            return;
        }
    }
    *OVERRIDES.write().unwrap() = Some(overrides);
    *CACHED.write().unwrap() = None;
}

/// Upstream `getCapabilities`: detect once (honoring
/// [`CapabilityOverrides::hyperlinks`] as the tmux probe answer), merge the
/// overrides and cache the result.
pub fn get_capabilities() -> TerminalCapabilities {
    let overrides = *OVERRIDES.read().unwrap();
    let mut cached = CACHED.write().unwrap();
    if let Some(caps) = *cached {
        return caps;
    }
    let mut caps = match overrides {
        Some(o) if o.hyperlinks.is_some() => {
            let hyperlinks = o.hyperlinks.unwrap();
            detect_capabilities_with_probe(move || hyperlinks)
        }
        _ => detect_capabilities_with_probe(probe_tmux_hyperlinks),
    };
    if let Some(o) = overrides {
        if let Some(images) = o.images {
            caps.images = images;
        }
        if let Some(true_color) = o.true_color {
            caps.true_color = true_color;
        }
        if let Some(hyperlinks) = o.hyperlinks {
            caps.hyperlinks = hyperlinks;
        }
    }
    *cached = Some(caps);
    caps
}

/// Upstream `detectCapabilities` against the real environment, shelling out
/// to tmux only when a multiplexer is detected and no override pinned the
/// hyperlink answer.
pub fn detect_capabilities() -> TerminalCapabilities {
    detect_capabilities_with_probe(probe_tmux_hyperlinks)
}

/// Upstream `detectCapabilities(tmuxForwardsHyperlink)` with an injected
/// probe.
pub fn detect_capabilities_with_probe(
    tmux_forwards_hyperlink: impl Fn() -> bool,
) -> TerminalCapabilities {
    detect_with(
        &|name| std::env::var(name).ok(),
        &tmux_forwards_hyperlink,
        cfg!(windows),
    )
}

/// Upstream `parseBooleanCapabilityOverride`.
fn parse_boolean_capability_override(value: Option<String>) -> Option<bool> {
    match value.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// Upstream `detectCapabilities` with injectable environment/probe/platform.
pub(crate) fn detect_with(
    lookup: &dyn Fn(&str) -> Option<String>,
    tmux_forwards_hyperlink: &dyn Fn() -> bool,
    is_windows_console: bool,
) -> TerminalCapabilities {
    let hyperlinks = parse_boolean_capability_override(lookup("PI_HYPERLINKS"));
    let detected = match hyperlinks {
        // Upstream bypasses the tmux probe entirely when PI_HYPERLINKS wins.
        Some(forced) => detect_from_environment(lookup, &|| forced, is_windows_console),
        None => detect_from_environment(lookup, tmux_forwards_hyperlink, is_windows_console),
    };
    let image_protocol = lookup("PI_IMAGE_PROTOCOL").map(|v| v.to_lowercase());
    let images = match image_protocol.as_deref() {
        Some("kitty") => Some(Some("kitty" as &str)),
        Some("iterm2") => Some(Some("iterm2" as &str)),
        Some("none") | Some("0") => Some(None),
        _ => None,
    };
    let true_color = parse_boolean_capability_override(lookup("PI_TRUE_COLOR"));
    TerminalCapabilities {
        images: images.unwrap_or(detected.images),
        true_color: true_color.unwrap_or(detected.true_color),
        hyperlinks: hyperlinks.unwrap_or(detected.hyperlinks),
    }
}

/// Upstream `detectCapabilitiesFromEnvironment` — the terminal family tree.
fn detect_from_environment(
    lookup: &dyn Fn(&str) -> Option<String>,
    tmux_forwards_hyperlink: &dyn Fn() -> bool,
    is_windows_console: bool,
) -> TerminalCapabilities {
    let term_program = lookup("TERM_PROGRAM")
        .map(|v| v.to_lowercase())
        .unwrap_or_default();
    let terminal_emulator = lookup("TERMINAL_EMULATOR")
        .map(|v| v.to_lowercase())
        .unwrap_or_default();
    let term = lookup("TERM").map(|v| v.to_lowercase()).unwrap_or_default();
    let color_term = lookup("COLORTERM")
        .map(|v| v.to_lowercase())
        .unwrap_or_default();
    let has_true_color_hint =
        color_term == "truecolor" || color_term == "24bit" || term.ends_with("-direct");

    // Emit OSC 8 hyperlinks only when tmux confirms it forwards. Image
    // protocols are unreliable under tmux, so leave `images: None`.
    if lookup("TMUX").is_some() || term.starts_with("tmux") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: tmux_forwards_hyperlink(),
        };
    }

    // screen does not forward OSC 8 hyperlinks, so keep them off there.
    if term.starts_with("screen") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: false,
        };
    }

    if lookup("KITTY_WINDOW_ID").is_some() || term_program == "kitty" {
        return kitty_caps();
    }

    if term_program == "ghostty"
        || term.contains("ghostty")
        || lookup("GHOSTTY_RESOURCES_DIR").is_some()
    {
        return kitty_caps();
    }

    if lookup("WEZTERM_PANE").is_some() || term_program == "wezterm" {
        return kitty_caps();
    }

    // Warp supports the Kitty graphics protocol and OSC 8 hyperlinks.
    if term_program == "warpterminal"
        || lookup("WARP_SESSION_ID").is_some()
        || lookup("WARP_TERMINAL_SESSION_UUID").is_some()
    {
        return kitty_caps();
    }

    if lookup("ITERM_SESSION_ID").is_some() || term_program == "iterm.app" {
        return TerminalCapabilities {
            images: Some("iterm2"),
            true_color: true,
            hyperlinks: true,
        };
    }

    if lookup("WT_SESSION").is_some() {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "alacritty" || term_program == "vscode" || term_program == "zed" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        };
    }

    if terminal_emulator == "jetbrains-jediterm" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: false,
        };
    }

    // Windows Terminal does not always set WT_SESSION, for example when it
    // hosts a cmd.exe launched directly from Win+R. Modern Windows consoles
    // support truecolor; keep hyperlinks off unless positively detected.
    if is_windows_console {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: false,
        };
    }

    // Unknown terminal: be conservative. OSC 8 is rendered invisibly as "just
    // text" on terminals that swallow it, which means the URL disappears from
    // the rendered output.
    TerminalCapabilities {
        images: None,
        true_color: has_true_color_hint,
        hyperlinks: false,
    }
}

fn kitty_caps() -> TerminalCapabilities {
    TerminalCapabilities {
        images: Some("kitty"),
        true_color: true,
        hyperlinks: true,
    }
}

/// Upstream `probeTmuxHyperlinks`: ask the attached tmux client whether it
/// forwards OSC 8 (`client_termfeatures` lists `hyperlinks`); any failure —
/// including the 250ms timeout — falls back to false.
fn probe_tmux_hyperlinks() -> bool {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Ok(mut child) = Command::new("tmux")
        .args(["display-message", "-p", "#{client_termfeatures}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(250);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break false,
        }
    };
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    let mut features = String::new();
    if stdout.read_to_string(&mut features).is_err() {
        return false;
    }
    features
        .split(',')
        .any(|feature| feature.trim() == "hyperlinks")
}

const KITTY_PREFIX: &str = "\x1b_G";
const ITERM2_PREFIX: &str = "\x1b]1337;File=";

/// Upstream `isImageLine`.
pub fn is_image_line(line: &str) -> bool {
    line.contains(KITTY_PREFIX) || line.contains(ITERM2_PREFIX)
}

/// Upstream `allocateImageId`: random ID in `[1, 0xffffffff]` to avoid
/// collisions between module instances. (Upstream uses `Math.random`; the
/// distribution comes from `rand`, only the range is contractual.)
pub fn allocate_image_id() -> u64 {
    use rand::RngExt;
    rand::rng().random_range(1..=0xffff_ffffu64)
}

/// Upstream `encodeITerm2`: OSC 1337 `File=` with the DECODED payload size in
/// `size=` (node `Buffer.byteLength(data, "base64")`).
pub fn encode_iterm2(base64_data: &str, options: &Iterm2Options) -> String {
    let mut params = vec![
        format!(
            "inline={}",
            if options.inline != Some(false) { 1 } else { 0 }
        ),
        format!("size={}", base64::byte_length(base64_data)),
    ];
    if let Some(width) = options.width {
        params.push(format!("width={width}"));
    }
    if let Some(height) = options.height {
        params.push(format!("height={height}"));
    }
    if let Some(name) = options.name {
        params.push(format!("name={}", base64::encode(name.as_bytes())));
    }
    if options.preserve_aspect_ratio == Some(false) {
        params.push("preserveAspectRatio=0".to_owned());
    }
    format!("\x1b]1337;File={}:{base64_data}\x07", params.join(";"))
}

/// `Math.max` with JS NaN propagation.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `Math.min` with JS NaN propagation.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// Upstream `calculateImageCellSize` — fit the image into the cell budget,
/// preserving aspect ratio unless a height cap forces a narrower scale.
/// `optimize_aspect_ratio` reduces the cell-aligned distortion by choosing the
/// lower cell count when it distorts less (upstream passes Kitty only).
pub fn calculate_image_cell_size(
    image_dimensions: ImageDimensions,
    max_width_cells: f64,
    max_height_cells: Option<f64>,
    cell_dimensions: CellDimensions,
    optimize_aspect_ratio: bool,
) -> ImageCellSize {
    let max_width = js_max(1.0, max_width_cells.floor());
    let max_height = max_height_cells.map(|h| js_max(1.0, h.floor()));
    let image_width = js_max(1.0, image_dimensions.width_px as f64);
    let image_height = js_max(1.0, image_dimensions.height_px as f64);

    let width_scale = max_width * cell_dimensions.width_px as f64 / image_width;
    let height_scale = match max_height {
        Some(max_height) => max_height * cell_dimensions.height_px as f64 / image_height,
        None => width_scale,
    };
    let scale = js_min(width_scale, height_scale);

    let scaled_width_px = image_width * scale;
    let scaled_height_px = image_height * scale;
    let mut columns = js_max(
        1.0,
        js_min(
            max_width,
            (scaled_width_px / cell_dimensions.width_px as f64).ceil(),
        ),
    );
    let height_rows = scaled_height_px / cell_dimensions.height_px as f64;
    let mut rows = js_max(1.0, height_rows.ceil());
    if let Some(max_height) = max_height {
        rows = js_min(max_height, rows);
    }

    if !optimize_aspect_ratio {
        return ImageCellSize {
            columns: columns as usize,
            rows: rows as usize,
        };
    }

    if width_scale <= height_scale {
        let ideal_rows = (columns * cell_dimensions.width_px as f64 * image_height)
            / (image_width * cell_dimensions.height_px as f64);
        rows = choose_less_distorted_cell_count(rows, ideal_rows);
    } else {
        let ideal_columns = (rows * cell_dimensions.height_px as f64 * image_width)
            / (image_height * cell_dimensions.width_px as f64);
        columns = choose_less_distorted_cell_count(columns, ideal_columns);
    }

    ImageCellSize {
        columns: columns as usize,
        rows: rows as usize,
    }
}

/// Upstream `chooseLessDistortedCellCount`: prefer `upperCount - 1` when it is
/// the strictly less distorted cell count.
fn choose_less_distorted_cell_count(upper_count: f64, ideal_count: f64) -> f64 {
    if upper_count <= 1.0 {
        return upper_count;
    }

    let lower_count = upper_count - 1.0;
    let upper_distortion = js_max(upper_count / ideal_count, ideal_count / upper_count);
    let lower_distortion = js_max(lower_count / ideal_count, ideal_count / lower_count);
    if lower_distortion < upper_distortion {
        lower_count
    } else {
        upper_count
    }
}

/// Upstream `calculateImageRows` with the `{9,18}` default cell size (the
/// upstream default parameter is a literal, not the mutable global).
pub fn calculate_image_rows(image_dimensions: ImageDimensions, target_width_cells: f64) -> usize {
    calculate_image_cell_size(
        image_dimensions,
        target_width_cells,
        None,
        CellDimensions {
            width_px: 9,
            height_px: 18,
        },
        false,
    )
    .rows
}

/// Upstream `getTerminalColorMode`.
pub fn get_terminal_color_mode(capabilities: Option<TerminalCapabilities>) -> TerminalColorMode {
    let capabilities = capabilities.unwrap_or_else(get_capabilities);
    if capabilities.true_color {
        TerminalColorMode::Truecolor
    } else {
        TerminalColorMode::Color256
    }
}

/// Upstream `getPngDimensions`: IHDR width/height from the first 24 decoded
/// bytes.
pub fn get_png_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64::decode(base64_data);
    if buffer.len() < 24 {
        return None;
    }
    if buffer[0] != 0x89 || buffer[1] != b'P' || buffer[2] != b'N' || buffer[3] != b'G' {
        return None;
    }
    let width = u32::from_be_bytes([buffer[16], buffer[17], buffer[18], buffer[19]]);
    let height = u32::from_be_bytes([buffer[20], buffer[21], buffer[22], buffer[23]]);
    Some(ImageDimensions {
        width_px: width as usize,
        height_px: height as usize,
    })
}

/// Upstream `getJpegDimensions`: scan markers until SOF0-2 and read the
/// big-endian height/width.
pub fn get_jpeg_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64::decode(base64_data);
    if buffer.len() < 2 || buffer[0] != 0xff || buffer[1] != 0xd8 {
        return None;
    }
    let mut offset = 2usize;
    while offset < buffer.len().saturating_sub(9) {
        if buffer[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = buffer[offset + 1];
        if (0xc0..=0xc2).contains(&marker) {
            let height = u16::from_be_bytes([buffer[offset + 5], buffer[offset + 6]]);
            let width = u16::from_be_bytes([buffer[offset + 7], buffer[offset + 8]]);
            return Some(ImageDimensions {
                width_px: width as usize,
                height_px: height as usize,
            });
        }
        if offset + 3 >= buffer.len() {
            return None;
        }
        let length = u16::from_be_bytes([buffer[offset + 2], buffer[offset + 3]]) as usize;
        if length < 2 {
            return None;
        }
        offset += 2 + length;
    }
    None
}

/// Upstream `getGifDimensions`: `GIF87a`/`GIF89a` signature then the
/// little-endian logical screen size.
pub fn get_gif_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64::decode(base64_data);
    if buffer.len() < 10 {
        return None;
    }
    let signature = &buffer[0..6];
    if signature != b"GIF87a" && signature != b"GIF89a" {
        return None;
    }
    let width = u16::from_le_bytes([buffer[6], buffer[7]]);
    let height = u16::from_le_bytes([buffer[8], buffer[9]]);
    Some(ImageDimensions {
        width_px: width as usize,
        height_px: height as usize,
    })
}

/// Upstream `getWebpDimensions`: RIFF/WEBP container plus the lossy, lossless
/// or extended chunk header.
pub fn get_webp_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64::decode(base64_data);
    if buffer.len() < 30 {
        return None;
    }
    if buffer[0..4] != *b"RIFF" || buffer[8..12] != *b"WEBP" {
        return None;
    }
    let (width_px, height_px) = match &buffer[12..16] {
        b"VP8 " => {
            if buffer.len() < 30 {
                return None;
            }
            let width = u16::from_le_bytes([buffer[26], buffer[27]]) & 0x3fff;
            let height = u16::from_le_bytes([buffer[28], buffer[29]]) & 0x3fff;
            (width as usize, height as usize)
        }
        b"VP8L" => {
            if buffer.len() < 25 {
                return None;
            }
            let bits = u32::from_le_bytes([buffer[21], buffer[22], buffer[23], buffer[24]]);
            let width = (bits & 0x3fff) + 1;
            let height = ((bits >> 14) & 0x3fff) + 1;
            (width as usize, height as usize)
        }
        b"VP8X" => {
            if buffer.len() < 30 {
                return None;
            }
            let width =
                u32::from(buffer[24]) | u32::from(buffer[25]) << 8 | u32::from(buffer[26]) << 16;
            let height =
                u32::from(buffer[27]) | u32::from(buffer[28]) << 8 | u32::from(buffer[29]) << 16;
            (width as usize + 1, height as usize + 1)
        }
        _ => return None,
    };
    Some(ImageDimensions {
        width_px,
        height_px,
    })
}

/// Upstream `getImageDimensions`: dispatch on the mime type.
pub fn get_image_dimensions(base64_data: &str, mime_type: &str) -> Option<ImageDimensions> {
    match mime_type {
        "image/png" => get_png_dimensions(base64_data),
        "image/jpeg" => get_jpeg_dimensions(base64_data),
        "image/gif" => get_gif_dimensions(base64_data),
        "image/webp" => get_webp_dimensions(base64_data),
        _ => None,
    }
}

/// Upstream `renderImage`: size the placement and emit the sequence for the
/// detected (or overridden) image protocol.
pub fn render_image(
    base64_data: &str,
    image_dimensions: ImageDimensions,
    options: ImageRenderOptions,
) -> Option<RenderedImage> {
    let caps = get_capabilities();
    let images = caps.images?;

    let max_width = options.max_width_cells.unwrap_or(80.0);
    // Reduce Kitty's cell-aligned distortion without shrinking iTerm2
    // reservations.
    let size = calculate_image_cell_size(
        image_dimensions,
        max_width,
        options.max_height_cells,
        get_cell_dimensions(),
        images == "kitty",
    );

    if images == "kitty" {
        if let Some(image_id) = options.image_id {
            register_kitty_image_metadata(KittyImageMetadata {
                image_id,
                columns: size.columns,
                rows: size.rows,
                width_px: image_dimensions.width_px,
                height_px: image_dimensions.height_px,
            });
        }
        let sequence = encode_kitty(
            base64_data,
            KittyOptions {
                columns: Some(size.columns),
                rows: Some(size.rows),
                image_id: options.image_id,
                move_cursor: options.move_cursor,
            },
        );
        return Some(RenderedImage {
            sequence,
            columns: size.columns,
            rows: size.rows,
            image_id: options.image_id,
        });
    }

    if images == "iterm2" {
        let sequence = encode_iterm2(
            base64_data,
            &Iterm2Options {
                width: Some(&size.columns.to_string()),
                height: Some("auto"),
                preserve_aspect_ratio: Some(options.preserve_aspect_ratio.unwrap_or(true)),
                ..Iterm2Options::default()
            },
        );
        return Some(RenderedImage {
            sequence,
            columns: size.columns,
            rows: size.rows,
            image_id: None,
        });
    }

    None
}

/// Upstream `hyperlink`: wrap text in an OSC 8 hyperlink sequence.
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// node `path.isAbsolute` for the current platform (win32 accepts drive
/// letters and both separators; posix only `/`).
fn is_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if cfg!(windows) {
        first == b'/'
            || first == b'\\'
            || (bytes.len() > 2
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'/' || bytes[2] == b'\\'))
    } else {
        first == b'/'
    }
}

/// Upstream `shortenImagePath` with an injectable home directory.
fn shorten_image_path_with_home(filename: &str, home: Option<&str>) -> String {
    if let Some(home) = home.filter(|home| !home.is_empty()) {
        if filename == home
            || filename.starts_with(&format!("{home}/"))
            || filename.starts_with(&format!("{home}\\"))
        {
            return format!("~{}", &filename[home.len()..]);
        }
    }
    filename.to_owned()
}

/// Upstream `shortenImagePath`: shorten home-prefixed absolute paths to
/// `~/...` for compact display.
pub fn shorten_image_path(filename: &str) -> String {
    let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());
    shorten_image_path_with_home(filename, home.as_deref())
}

/// Upstream `imageFallback`: text fallback when the terminal cannot render
/// inline images. Absolute paths are shown shortened and, when OSC 8
/// hyperlinks are available, linked to `file://`.
pub fn image_fallback(
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    filename: Option<&str>,
) -> String {
    let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned());
    image_fallback_with_home(mime_type, dimensions, filename, home.as_deref())
}

/// [`image_fallback`] with an injectable home directory (node `os.homedir`).
fn image_fallback_with_home(
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    filename: Option<&str>,
    home: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(filename) = filename {
        let display = shorten_image_path_with_home(filename, home);
        if get_capabilities().hyperlinks && is_absolute_path(filename) {
            match Url::from_file_path(filename) {
                Ok(url) => parts.push(hyperlink(&display, url.as_str())),
                Err(_) => parts.push(display),
            }
        } else {
            parts.push(display);
        }
    }
    parts.push(format!("[{mime_type}]"));
    if let Some(dimensions) = dimensions {
        parts.push(format!("{}x{}", dimensions.width_px, dimensions.height_px));
    }
    format!("[Image: {}]", parts.join(" "))
}
