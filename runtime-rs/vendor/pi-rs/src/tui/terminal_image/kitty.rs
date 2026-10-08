//! Kitty graphics protocol encoding plus the bounded metadata registry,
//! placement rebuilding, row cropping and deletion commands of upstream
//! `terminal-image.ts` (kitty sections). Capability detection, iTerm2 and the
//! pixel-dimension probes live in the parent module.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

const KITTY_PREFIX: &str = "\x1b_G";
const CHUNK_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KittyOptions {
    pub columns: Option<usize>,
    pub rows: Option<usize>,
    pub image_id: Option<u64>,
    pub move_cursor: Option<bool>,
}

/// Upstream `encodeKitty`: single-shot transmission at or below 4096 base64
/// characters, otherwise `m=1`/`m=1`/`m=0` continuation chunks of exactly 4096.
/// The input is ASCII base64, not arbitrary UTF-16 text (upstream slices JS
/// string units; disclosed divergence for non-ASCII input).
pub fn encode_kitty(base64_data: &str, options: KittyOptions) -> String {
    assert!(base64_data.is_ascii(), "Kitty data must be ASCII base64");
    let mut params = vec!["a=T".to_owned(), "f=100".to_owned(), "q=2".to_owned()];
    if options.move_cursor == Some(false) {
        params.push("C=1".into());
    }
    // Upstream truthiness: zero values are omitted just like absent ones.
    for (key, value) in [
        ("c", options.columns.map(|n| n as u64)),
        ("r", options.rows.map(|n| n as u64)),
        ("i", options.image_id),
    ] {
        if let Some(value) = value.filter(|n| *n != 0) {
            params.push(format!("{key}={value}"));
        }
    }
    let params = params.join(",");
    if base64_data.len() <= CHUNK_SIZE {
        return format!("\x1b_G{params};{base64_data}\x1b\\");
    }
    let chunks = base64_data.as_bytes().chunks(CHUNK_SIZE);
    let count = chunks.len();
    let mut result = String::new();
    for (i, chunk) in chunks.enumerate() {
        let controls = if i == 0 {
            format!("{params},m=1")
        } else if i + 1 == count {
            "m=0".to_owned()
        } else {
            "m=1".to_owned()
        };
        result.push_str(&format!(
            "\x1b_G{controls};{}\x1b\\",
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    result
}

/// Upstream `deleteKittyImage`: uppercase `d=I` also frees the image data.
pub fn delete_kitty_image(image_id: u64) -> String {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
}

/// Upstream `deleteAllKittyImages`: delete every visible placement and free
/// the image data.
pub fn delete_all_kitty_images() -> String {
    "\x1b_Ga=d,d=A,q=2\x1b\\".to_owned()
}

/// Upstream `deleteAllKittyPlacements`: delete every visible placement while
/// retaining the uploaded image data.
pub fn delete_all_kitty_placements() -> String {
    "\x1b_Ga=d,d=a,q=2\x1b\\".to_owned()
}

/// Upstream `KittyImageMetadata`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KittyImageMetadata {
    pub image_id: u64,
    pub columns: usize,
    pub rows: usize,
    pub width_px: usize,
    pub height_px: usize,
}

/// Upstream `KittyImagePlacement` — a placement-only command rebuilt from a
/// transmitted (and possibly cropped) image line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KittyImagePlacement {
    pub image_id: u64,
    pub transmission_generation: u64,
    pub transmission_bytes: usize,
    pub estimated_decoded_bytes: u64,
    /// Rows covered by the placement (v1.0.0): the command's explicit `r=`,
    /// else the registered metadata's rows.
    pub rows: usize,
    pub sequence: String,
    pub replacement_line: String,
}

#[derive(Clone, Copy)]
struct Registered {
    metadata: KittyImageMetadata,
    generation: u64,
}

/// Upstream module-level `kittyImageMetadata` map plus the
/// `kittyTransmissionGeneration` counter, bounded at 1000 entries with
/// insertion-order eviction.
#[derive(Default)]
pub struct KittyImageRegistry {
    entries: VecDeque<Registered>,
    generation: u64,
}

/// Index of `\x1b_G`, index just past the `;` of that header, and the controls
/// text between them. Mirrors the regex `/\x1b_G([^;]*);/` first match: the
/// first `\x1b_G` occurrence, controls run to the next `;` (a missing `;`
/// means no match anywhere, since every later candidate is a suffix).
fn header(line: &str) -> Option<(usize, usize, &str)> {
    let start = line.find(KITTY_PREFIX)?;
    let controls_start = start + KITTY_PREFIX.len();
    let semi = line[controls_start..].find(';')?;
    let end = controls_start + semi + 1;
    Some((start, end, &line[controls_start..end - 1]))
}

/// The `(?:^|,)i=(\d+)(?:,|$)` extraction over raw controls text: `i=` as a
/// whole comma field with ASCII digits only.
fn image_id_in_controls(controls: &str) -> Option<u64> {
    controls.split(',').find_map(|field| {
        let digits = field.strip_prefix("i=")?;
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    })
}

/// `image_id_in_controls` over a full line (first header's controls).
fn image_id(line: &str) -> Option<u64> {
    let (_, _, controls) = header(line)?;
    image_id_in_controls(controls)
}

/// Whole-field membership — `(?:^|,)m=1(?:,|$)` on comma-separated controls.
fn has_field(controls: &str, field: &str) -> bool {
    controls.split(',').any(|f| f == field)
}

impl KittyImageRegistry {
    pub fn register(&mut self, metadata: KittyImageMetadata) {
        self.generation += 1;
        self.entries
            .retain(|item| item.metadata.image_id != metadata.image_id);
        self.entries.push_back(Registered {
            metadata,
            generation: self.generation,
        });
        if self.entries.len() > 1000 {
            self.entries.pop_front();
        }
    }

    fn registered(&self, line: &str) -> Option<Registered> {
        let id = image_id(line)?;
        self.entries
            .iter()
            .find(|item| item.metadata.image_id == id)
            .copied()
    }

    /// Upstream `getRegisteredKittyImageMetadataFromControls` (v1.0.0): the
    /// registered metadata for the `i=` field of `controls` alone.
    fn registered_for_controls(&self, controls: &str) -> Option<Registered> {
        let id = image_id_in_controls(controls)?;
        self.entries
            .iter()
            .find(|item| item.metadata.image_id == id)
            .copied()
    }

    /// Upstream `getKittyImageMetadata`.
    pub fn get(&self, line: &str) -> Option<KittyImageMetadata> {
        self.registered(line).map(|item| item.metadata)
    }

    /// The `transmissionGeneration` stored with the metadata for `line`.
    pub fn transmission_generation(&self, line: &str) -> Option<u64> {
        self.registered(line).map(|item| item.generation)
    }

    /// Upstream `getKittyImagePlacement`: walk the transmission chunks of
    /// `line` (first header through the chunk without `m=1`), then rebuild a
    /// placement-only command from the first header's controls.
    pub fn placement(&self, line: &str) -> Option<KittyImagePlacement> {
        let (match_start, _, controls) = header(line)?;
        let registered = self.registered_for_controls(controls)?;
        let mut command_start = match_start;
        let mut command_controls = controls.to_owned();
        let transmission_end = loop {
            let search_from = command_start + KITTY_PREFIX.len();
            let found = line
                .get(search_from..)
                .and_then(|tail| tail.find("\x1b\\"))?;
            let terminator = search_from + found;
            if !has_field(&command_controls, "m=1") {
                break terminator + 2;
            }
            command_start = terminator + 2;
            if !line
                .get(command_start..)
                .is_some_and(|tail| tail.starts_with(KITTY_PREFIX))
            {
                return None;
            }
            let after = command_start + KITTY_PREFIX.len();
            let semi = line.get(after..).and_then(|tail| tail.find(';'))?;
            command_controls = line[after..after + semi].to_owned();
        };
        const PLACEMENT_KEYS: [&str; 17] = [
            "i", "p", "x", "y", "w", "h", "X", "Y", "c", "r", "C", "U", "z", "P", "Q", "H", "V",
        ];
        let filtered: Vec<&str> = controls
            .split(',')
            .filter(|field| PLACEMENT_KEYS.contains(&field.split('=').next().unwrap_or("")))
            .collect();
        let sequence = format!("\x1b_Ga=p,q=2,{}\x1b\\", filtered.join(","));
        Some(KittyImagePlacement {
            image_id: registered.metadata.image_id,
            transmission_generation: registered.generation,
            transmission_bytes: transmission_end - match_start,
            estimated_decoded_bytes: registered.metadata.width_px as u64
                * registered.metadata.height_px as u64
                * 4,
            rows: kitty_image_rows_from_controls(controls, registered.metadata.rows),
            replacement_line: format!(
                "{}{}{}",
                &line[..match_start],
                sequence,
                &line[transmission_end..]
            ),
            sequence,
        })
    }

    /// Upstream `cropKittyImageLine`: shrink the visible rows of a placement
    /// by injecting source-pixel `y`/`h` and `r` controls. Note the filter
    /// drops any original `y=`/`h=`/`r=` field, including a previous `r=`.
    pub fn crop(&self, line: &str, hidden_rows: i64, visible_rows: i64) -> String {
        let Some(metadata) = self.get(line) else {
            return line.to_owned();
        };
        let Some((start, end, controls)) = header(line) else {
            return line.to_owned();
        };
        if hidden_rows < 0 || hidden_rows as i128 >= metadata.rows as i128 || visible_rows <= 0 {
            return line.to_owned();
        }
        let hidden = hidden_rows as usize;
        let rows = (visible_rows as usize).min(metadata.rows - hidden);
        if hidden == 0 && rows == metadata.rows {
            return line.to_owned();
        }
        let source_y =
            (metadata.height_px as f64 * hidden as f64 / metadata.rows as f64).floor() as usize;
        let source_end = (metadata.height_px as f64 * (hidden + rows) as f64 / metadata.rows as f64)
            .ceil() as usize;
        let source_height = source_end
            .min(metadata.height_px)
            .saturating_sub(source_y)
            .max(1);
        let mut kept: Vec<String> = controls
            .split(',')
            .filter(|c| !c.starts_with("y=") && !c.starts_with("h=") && !c.starts_with("r="))
            .map(str::to_owned)
            .collect();
        kept.extend([
            format!("y={source_y}"),
            format!("h={source_height}"),
            format!("r={rows}"),
        ]);
        format!(
            "{}\x1b_G{};{}",
            &line[..start],
            kept.join(","),
            &line[end..]
        )
    }
}

static REGISTRY: LazyLock<Mutex<KittyImageRegistry>> =
    LazyLock::new(|| Mutex::new(KittyImageRegistry::default()));

/// Upstream `registerKittyImageMetadata` (module-level registry).
pub fn register_kitty_image_metadata(metadata: KittyImageMetadata) {
    REGISTRY.lock().unwrap().register(metadata);
}

/// Upstream `getKittyImageMetadata` (module-level registry).
pub fn get_kitty_image_metadata(line: &str) -> Option<KittyImageMetadata> {
    REGISTRY.lock().unwrap().get(line)
}

/// Upstream `getKittyImagePlacement` (module-level registry).
pub fn get_kitty_image_placement(line: &str) -> Option<KittyImagePlacement> {
    REGISTRY.lock().unwrap().placement(line)
}

/// Upstream `cropKittyImageLine` (module-level registry).
pub fn crop_kitty_image_line(line: &str, hidden_rows: i64, visible_rows: i64) -> String {
    REGISTRY
        .lock()
        .unwrap()
        .crop(line, hidden_rows, visible_rows)
}

/// Upstream `getExplicitKittyImageRows` (v1.0.0): the `(?:^|,)r=(\d+)
/// (?:,|$)` field, kept only when positive.
fn explicit_kitty_image_rows(controls: &str) -> Option<usize> {
    let digits = controls
        .split(',')
        .find_map(|field| field.strip_prefix("r="))?;
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rows: usize = digits.parse().ok()?;
    (rows > 0).then_some(rows)
}

/// Upstream `getKittyImageRowsFromControls` (v1.0.0).
fn kitty_image_rows_from_controls(controls: &str, fallback_rows: usize) -> usize {
    explicit_kitty_image_rows(controls).unwrap_or(fallback_rows)
}

/// Upstream `getKittyImagePlacementRows` (v1.0.0): read the number of rows
/// covered by an image placement without scanning its payload — the
/// command's explicit `r=`, else the registered metadata's rows.
pub fn get_kitty_image_placement_rows(line: &str) -> Option<usize> {
    let (_, _, controls) = header(line)?;
    if let Some(rows) = explicit_kitty_image_rows(controls) {
        return Some(rows);
    }
    REGISTRY
        .lock()
        .unwrap()
        .registered_for_controls(controls)
        .map(|registered| registered.metadata.rows)
}
