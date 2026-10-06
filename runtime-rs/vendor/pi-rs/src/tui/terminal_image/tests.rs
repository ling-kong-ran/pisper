//! Upstream test port plus oracle replay for `terminal-image.ts`.
//!
//! Two evidence sources:
//! - `tests/fixtures/terminal_image_oracle/oracle.mjs` ran the REAL upstream module
//!   under node and captured exact outputs per scenario; every
//!   `oracle_*` test replays a section of that capture byte-for-byte.
//! - The upstream `test/terminal-image.test.ts` and
//!   `test/bug-regression-isimageline-startswith-bug.test.ts` cases are
//!   ported 1:1 where they exercise this module (the `Image` component cases
//!   belong to components/image.ts, a separate slice with no Rust port yet).
//!
//! Platform note: the detect-oracle was captured with
//! `process.platform === "win32"`; the replay passes `cfg!(windows)`, so the
//! `empty` / `colorterm 24bit alone` rows are only meaningful on Windows (the
//! gate environment). Tests that touch the process-global capability cache,
//! cell dimensions or the global Kitty registry serialize on `STATE_LOCK`.

use super::oracle_consts::ORACLE_JSON;
use super::{
    allocate_image_id, base64, calculate_image_cell_size, calculate_image_rows,
    delete_all_kitty_images, delete_all_kitty_placements, delete_kitty_image, detect_with,
    encode_iterm2, get_cell_dimensions, get_gif_dimensions, get_image_dimensions,
    get_jpeg_dimensions, get_kitty_image_metadata, get_png_dimensions, get_webp_dimensions,
    hyperlink, image_fallback_with_home, is_image_line, probe_tmux_hyperlinks, render_image,
    reset_capabilities_cache, set_capabilities, set_capability_overrides, set_cell_dimensions,
    CapabilityOverrides, CellDimensions, ImageDimensions, ImageRenderOptions, Iterm2Options,
    KittyImageMetadata, KittyImageRegistry, KittyOptions, TerminalCapabilities, ITERM2_PREFIX,
    KITTY_PREFIX,
};
use serde_json::Value;
use std::sync::Mutex;
use std::sync::OnceLock;

fn state_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn oracle() -> &'static Value {
    static ORACLE: OnceLock<Value> = OnceLock::new();
    ORACLE.get_or_init(|| serde_json::from_str(ORACLE_JSON).expect("oracle JSON parses"))
}

fn rows<'a>(oracle: &'a Value, section: &str) -> &'a Vec<Value> {
    oracle[section]
        .as_array()
        .unwrap_or_else(|| panic!("oracle section {section}"))
}

fn oracle_sha_matches_upstream(oracle: &Value) {
    // The first SHA is the pre-delta capture (upstream 590144609); the second
    // is the v0.99.1 terminal-image.ts. The delta only added the optional
    // aspect-ratio pass to calculateImageCellSize (called with
    // optimizeAspectRatio=false here; true is pinned by the tui_delta_oracle
    // fixtures), a `-direct` TERM suffix hint, and getTerminalColorMode, so
    // the earlier capture remains valid for these sections.
    let sha = oracle["meta"]["upstreamSha256"].as_str().unwrap();
    assert!(
        sha == "f29572354977fd4ef4cc76878d31cce3cb5c45cebdfc9b5c49b50ad9cfb3171f"
            || sha == "b901b8df0ad84e04ac0ee612c419b45d960430f63d2bdaa974c8dd9d89f3e6e5",
        "oracle was captured against a different upstream terminal-image.ts",
    );
    assert_eq!(oracle["meta"]["platform"].as_str().unwrap(), "win32");
}

// ---------------------------------------------------------------- base64
#[test]
fn oracle_base64_decode_and_byte_length_match_node() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for row in rows(oracle, "base64Decode") {
        let input = row["input"].as_str().unwrap();
        let decoded = base64::decode(input);
        assert_eq!(
            decoded.len(),
            row["len"].as_u64().unwrap() as usize,
            "decode length for {input:?}",
        );
        let hex: String = decoded.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            row["hex"].as_str().unwrap(),
            "decode hex for {input:?}"
        );
    }
    for row in rows(oracle, "base64ByteLength") {
        let input = row["input"].as_str().unwrap();
        assert_eq!(
            base64::byte_length(input),
            row["n"].as_u64().unwrap() as usize,
            "byteLength for {input:?}",
        );
    }
}

#[test]
#[allow(non_snake_case)]
fn base64_encode_round_trips_buffer_toString_output() {
    // node: Buffer.from([0,16,131]).toString("base64") === "ABCD", canonical
    // padding, and Buffer.from("照片.png").toString("base64") from the
    // iTerm2 name oracle row.
    assert_eq!(base64::encode(&[0x00, 0x10, 0x83]), "ABCD");
    assert_eq!(base64::encode(&[0x00]), "AA==");
    assert_eq!(base64::encode(&[0x00, 0x10]), "ABA=");
    assert_eq!(base64::encode(&[]), "");
    assert_eq!(base64::encode("照片.png".as_bytes()), "54Wn54mHLnBuZw==");
}

// ---------------------------------------------------------------- detect
fn env_lookup(env: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| env[name].as_str().map(str::to_owned)
}

#[test]
fn oracle_detect_capabilities_matrix() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for (i, row) in rows(oracle, "detect").iter().enumerate() {
        let lookup = env_lookup(&row["env"]);
        let probe_calls = std::cell::Cell::new(0usize);
        let probe_answer = row["probe"].as_bool().unwrap();
        let caps = detect_with(
            &lookup,
            &|| {
                probe_calls.set(probe_calls.get() + 1);
                probe_answer
            },
            // environment-anchored: the matrix was captured upstream on win32,
            // where an empty environment means a Windows console; replay that
            // console kind on every platform (env and probe are synthetic).
            true,
        );
        let expected_images = row["images"].as_str();
        assert_eq!(
            caps.images,
            expected_images.map(|s| match s {
                "kitty" => "kitty",
                "iterm2" => "iterm2",
                other => panic!("unexpected protocol {other}"),
            }),
            "detect row {i} ({}) images",
            row["name"].as_str().unwrap(),
        );
        assert_eq!(
            caps.true_color,
            row["trueColor"].as_bool().unwrap(),
            "detect row {i} ({}) trueColor",
            row["name"].as_str().unwrap(),
        );
        assert_eq!(
            caps.hyperlinks,
            row["hyperlinks"].as_bool().unwrap(),
            "detect row {i} ({}) hyperlinks",
            row["name"].as_str().unwrap(),
        );
        assert_eq!(
            probe_calls.get() > 0,
            row["probeCalled"].as_bool().unwrap(),
            "detect row {i} ({}) probe usage",
            row["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn capability_overrides_and_cache_flow_match_upstream() {
    let _guard = state_lock().lock().unwrap();
    let oracle = oracle();
    let flow = rows(oracle, "overrideFlow");
    let all_off = CapabilityOverrides {
        images: Some(None),
        true_color: Some(false),
        hyperlinks: Some(false),
    };
    let kitty = CapabilityOverrides {
        images: Some(Some("kitty")),
        true_color: Some(true),
        hyperlinks: Some(true),
    };

    // step 1: override everything off (oracle detects kitty through PI_*)
    set_capability_overrides(all_off);
    let caps = super::get_capabilities();
    assert_eq!(caps.images, flow[0]["caps"]["images"].as_str());
    assert!(!caps.true_color);
    assert!(!caps.hyperlinks);

    // step 2: an EQUAL override must not clear the cache, so the pinned
    // capabilities set underneath survive
    set_capability_overrides(all_off);
    set_capabilities(TerminalCapabilities {
        images: Some("iterm2"),
        true_color: true,
        hyperlinks: true,
    });
    let caps = super::get_capabilities();
    assert_eq!(caps.images, Some("iterm2"), "equal override kept the cache");
    assert!(caps.true_color && caps.hyperlinks);

    // step 3: a DIFFERENT override drops the cache and re-detects (oracle:
    // PI_* envs force kitty)
    set_capability_overrides(kitty);
    let caps = super::get_capabilities();
    assert_eq!(flow[2]["caps"]["images"].as_str(), Some("kitty"));
    assert_eq!(caps.images, Some("kitty"));
    assert!(caps.true_color && caps.hyperlinks);

    // step 4: clearing overrides falls back to plain detection; oracle
    // captures the same shape via PI_* env, here compared against the
    // detection computed from the live environment.
    set_capability_overrides(CapabilityOverrides::default());
    reset_capabilities_cache();
    let caps = super::get_capabilities();
    let expected = detect_with(&|name| std::env::var(name).ok(), &|| false, cfg!(windows));
    assert_eq!(caps, expected, "cleared overrides fall back to detection");

    set_capability_overrides(CapabilityOverrides::default());
    reset_capabilities_cache();
}

// ---------------------------------------------------------------- kitty
#[test]
fn oracle_encode_kitty_chunk_boundaries() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for (i, case) in rows(oracle, "encodeKitty").iter().enumerate() {
        let options = &case["options"];
        let actual = super::encode_kitty(
            case["data"].as_str().unwrap(),
            KittyOptions {
                columns: options["columns"].as_u64().map(|n| n as usize),
                rows: options["rows"].as_u64().map(|n| n as usize),
                image_id: options["imageId"].as_u64(),
                move_cursor: options["moveCursor"].as_bool(),
            },
        );
        assert_eq!(
            actual,
            case["sequence"].as_str().unwrap(),
            "encodeKitty case {i}"
        );
    }
}

#[test]
fn oracle_delete_commands_are_byte_exact() {
    let oracle = oracle();
    assert_eq!(
        delete_kitty_image(42),
        oracle["deleteCommands"]["delete42"].as_str().unwrap()
    );
    assert_eq!(
        delete_kitty_image(1),
        oracle["deleteCommands"]["delete1"].as_str().unwrap()
    );
    assert_eq!(
        delete_kitty_image(0),
        oracle["deleteCommands"]["delete0"].as_str().unwrap(),
        "id 0 still formats into the delete command",
    );
    assert_eq!(
        delete_kitty_image(4294967295),
        oracle["deleteCommands"]["deleteBig"].as_str().unwrap()
    );
    assert_eq!(delete_all_kitty_images(), "\x1b_Ga=d,d=A,q=2\x1b\\");
    assert_eq!(delete_all_kitty_placements(), "\x1b_Ga=d,d=a,q=2\x1b\\");
}

#[test]
fn oracle_registry_flow_replays_register_metadata_placement_crop() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    let mut registry = KittyImageRegistry::default();
    for (i, op) in rows(oracle, "registryFlow").iter().enumerate() {
        match op["op"].as_str().unwrap() {
            "register" => {
                let m = &op["metadata"];
                registry.register(KittyImageMetadata {
                    image_id: m["imageId"].as_u64().unwrap(),
                    columns: m["columns"].as_u64().unwrap() as usize,
                    rows: m["rows"].as_u64().unwrap() as usize,
                    width_px: m["widthPx"].as_u64().unwrap() as usize,
                    height_px: m["heightPx"].as_u64().unwrap() as usize,
                });
            }
            "metadata" => {
                let line = op["line"].as_str().unwrap();
                let expected = &op["metadata"];
                let actual = registry.get(line);
                if expected.is_null() {
                    assert!(actual.is_none(), "flow op {i}: expected no metadata");
                } else {
                    let m = actual.unwrap_or_else(|| panic!("flow op {i}: expected metadata"));
                    assert_eq!(m.image_id, expected["imageId"].as_u64().unwrap(), "op {i}");
                    assert_eq!(
                        m.columns as u64,
                        expected["columns"].as_u64().unwrap(),
                        "op {i}"
                    );
                    assert_eq!(m.rows as u64, expected["rows"].as_u64().unwrap(), "op {i}");
                    assert_eq!(
                        m.width_px as u64,
                        expected["widthPx"].as_u64().unwrap(),
                        "op {i}"
                    );
                    assert_eq!(
                        m.height_px as u64,
                        expected["heightPx"].as_u64().unwrap(),
                        "op {i}"
                    );
                }
            }
            "placement" => {
                let line = op["line"].as_str().unwrap();
                let expected = &op["placement"];
                let actual = registry.placement(line);
                if expected.is_null() {
                    assert!(actual.is_none(), "flow op {i}: expected no placement");
                } else {
                    let p = actual.unwrap_or_else(|| panic!("flow op {i}: expected placement"));
                    assert_eq!(p.image_id, expected["imageId"].as_u64().unwrap(), "op {i}");
                    assert_eq!(
                        p.transmission_generation,
                        expected["transmissionGeneration"].as_u64().unwrap(),
                        "op {i} generation",
                    );
                    assert_eq!(
                        p.transmission_bytes,
                        expected["transmissionBytes"].as_u64().unwrap() as usize,
                        "op {i} transmission bytes",
                    );
                    assert_eq!(
                        p.estimated_decoded_bytes,
                        expected["estimatedDecodedBytes"].as_u64().unwrap(),
                        "op {i} estimated decoded bytes",
                    );
                    assert_eq!(
                        p.sequence,
                        expected["sequence"].as_str().unwrap(),
                        "op {i} sequence"
                    );
                    assert_eq!(
                        p.replacement_line,
                        expected["replacementLine"].as_str().unwrap(),
                        "op {i} replacement line",
                    );
                }
            }
            "crop" => {
                let line = op["line"].as_str().unwrap();
                let actual = registry.crop(
                    line,
                    op["hiddenRows"].as_i64().unwrap(),
                    op["visibleRows"].as_i64().unwrap(),
                );
                assert_eq!(actual, op["result"].as_str().unwrap(), "flow op {i} crop");
            }
            other => panic!("unknown registry flow op {other}"),
        }
    }
}

/// The upstream renderImage crop/metadata test through the global registry:
/// register via render, crop a partially visible placement.
#[test]
fn global_registry_crops_partially_visible_render_placement() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: Some("kitty"),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });
    let rendered = render_image(
        "AAAA",
        ImageDimensions {
            width_px: 100,
            height_px: 100,
        },
        ImageRenderOptions {
            max_width_cells: Some(3.0),
            image_id: Some(42301),
            move_cursor: Some(false),
            ..ImageRenderOptions::default()
        },
    )
    .expect("kitty render");
    let metadata =
        get_kitty_image_metadata(&rendered.sequence).expect("render registered metadata");
    assert_eq!(
        metadata,
        KittyImageMetadata {
            image_id: 42301,
            columns: 3,
            rows: 3,
            width_px: 100,
            height_px: 100,
        }
    );
    let cropped = super::crop_kitty_image_line(&rendered.sequence, 2, 1);
    assert!(cropped.contains("y=66,h=34,r=1"), "cropped: {cropped:?}");
    // placement-only command rebuilt from the cropped line through the
    // global wrapper (upstream "creates placement-only commands" shape)
    let placement = super::get_kitty_image_placement(&cropped).expect("global placement");
    assert_eq!(placement.image_id, 42301);
    assert_eq!(
        placement.sequence,
        "\x1b_Ga=p,q=2,C=1,c=3,i=42301,y=66,h=34,r=1\x1b\\"
    );
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

#[test]
fn probe_tmux_hyperlinks_smoke() {
    // Never shells out in tests with a multiplexer absent; must not panic and
    // must return a bool (the real tmux answer is environment-dependent).
    let _ = probe_tmux_hyperlinks();
}

// ---------------------------------------------------------------- iterm2
#[test]
fn oracle_encode_iterm2_includes_decoded_sizes() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for (i, case) in rows(oracle, "encodeITerm2").iter().enumerate() {
        let options = &case["options"];
        let js_string = |key: &str| -> Option<String> {
            let value = &options[key];
            if value.is_null() {
                return None;
            }
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_i64().map(|n| n.to_string()))
        };
        let actual = encode_iterm2(
            case["data"].as_str().unwrap(),
            &Iterm2Options {
                width: js_string("width").as_deref(),
                height: js_string("height").as_deref(),
                name: js_string("name").as_deref(),
                preserve_aspect_ratio: options["preserveAspectRatio"].as_bool(),
                inline: options["inline"].as_bool(),
            },
        );
        assert_eq!(
            actual,
            case["sequence"].as_str().unwrap(),
            "encodeITerm2 case {i}"
        );
    }
}

// ---------------------------------------------------------------- cell size
#[test]
fn oracle_calculate_image_cell_size_and_rows() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for (i, case) in rows(oracle, "cellSize").iter().enumerate() {
        let input = &case["input"];
        let dims = &input["dims"];
        let cell = &input["cell"];
        let size = calculate_image_cell_size(
            ImageDimensions {
                width_px: dims["widthPx"].as_f64().unwrap() as usize,
                height_px: dims["heightPx"].as_f64().unwrap() as usize,
            },
            input["maxWidth"].as_f64().unwrap(),
            input["maxHeight"].as_f64(),
            if cell.is_null() {
                CellDimensions::default()
            } else {
                CellDimensions {
                    width_px: cell["widthPx"].as_u64().unwrap() as usize,
                    height_px: cell["heightPx"].as_u64().unwrap() as usize,
                }
            },
            false,
        );
        assert_eq!(
            size.columns as u64,
            case["cellSize"]["columns"].as_u64().unwrap(),
            "cellSize case {i} columns",
        );
        assert_eq!(
            size.rows as u64,
            case["cellSize"]["rows"].as_u64().unwrap(),
            "cellSize case {i} rows",
        );
    }
    for (i, case) in rows(oracle, "calculateRows").iter().enumerate() {
        let dims = &case["dims"];
        let calculated = calculate_image_rows(
            ImageDimensions {
                width_px: dims["widthPx"].as_u64().unwrap() as usize,
                height_px: dims["heightPx"].as_u64().unwrap() as usize,
            },
            case["width"].as_f64().unwrap(),
        );
        assert_eq!(
            calculated as u64,
            case["rows"].as_u64().unwrap(),
            "calculateRows case {i}"
        );
    }
}

// ---------------------------------------------------------------- dimensions
#[test]
fn oracle_image_dimension_probes() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    for case in rows(oracle, "dimensions") {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        let expected = &case["result"];
        let actual = match name {
            "png via mime" => get_image_dimensions(input, "image/png"),
            "jpeg via mime" => get_image_dimensions(input, "image/jpeg"),
            "gif via mime" => get_image_dimensions(input, "image/gif"),
            "webp vp8 via mime" => get_image_dimensions(input, "image/webp"),
            "unknown mime" => get_image_dimensions(input, "image/x-png"),
            "jpeg data under png mime" => get_image_dimensions(input, "image/png"),
            _ if name.starts_with("png") => get_png_dimensions(input),
            _ if name.starts_with("jpeg") => get_jpeg_dimensions(input),
            _ if name.starts_with("gif") => get_gif_dimensions(input),
            _ if name.starts_with("webp") => get_webp_dimensions(input),
            other => panic!("unrouted dimension case {other}"),
        };
        if expected.is_null() {
            assert!(actual.is_none(), "dimension case {name}: expected None");
        } else {
            let dims = actual.unwrap_or_else(|| panic!("dimension case {name}: expected dims"));
            assert_eq!(
                dims.width_px as u64,
                expected["widthPx"].as_u64().unwrap(),
                "dimension case {name} width",
            );
            assert_eq!(
                dims.height_px as u64,
                expected["heightPx"].as_u64().unwrap(),
                "dimension case {name} height",
            );
        }
    }
}

// ---------------------------------------------------------------- render
fn capabilities_from_json(value: &Value) -> TerminalCapabilities {
    TerminalCapabilities {
        images: value["images"].as_str().map(|s| match s {
            "kitty" => "kitty",
            "iterm2" => "iterm2",
            other => panic!("unexpected protocol {other}"),
        }),
        true_color: value["trueColor"].as_bool().unwrap(),
        hyperlinks: value["hyperlinks"].as_bool().unwrap(),
    }
}

fn image_dimensions_from_json(value: &Value) -> ImageDimensions {
    ImageDimensions {
        width_px: value["widthPx"].as_u64().unwrap() as usize,
        height_px: value["heightPx"].as_u64().unwrap() as usize,
    }
}

#[test]
fn oracle_render_image_matrix() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    let _guard = state_lock().lock().unwrap();
    for case in rows(oracle, "renderCases") {
        set_capabilities(capabilities_from_json(&case["caps"]));
        set_cell_dimensions(CellDimensions {
            width_px: case["cell"]["widthPx"].as_u64().unwrap() as usize,
            height_px: case["cell"]["heightPx"].as_u64().unwrap() as usize,
        });
        let options = &case["options"];
        let rendered = render_image(
            case["data"].as_str().unwrap(),
            image_dimensions_from_json(&case["dims"]),
            ImageRenderOptions {
                max_width_cells: options["maxWidthCells"].as_f64(),
                max_height_cells: options["maxHeightCells"].as_f64(),
                preserve_aspect_ratio: options["preserveAspectRatio"].as_bool(),
                image_id: options["imageId"].as_u64(),
                move_cursor: options["moveCursor"].as_bool(),
            },
        );
        let expected = &case["result"];
        if expected.is_null() {
            assert!(
                rendered.is_none(),
                "render case {}: expected None",
                case["name"]
            );
            continue;
        }
        let rendered =
            rendered.unwrap_or_else(|| panic!("render case {}: expected render", case["name"]));
        assert_eq!(
            rendered.sequence,
            expected["sequence"].as_str().unwrap(),
            "sequence"
        );
        assert_eq!(
            rendered.columns as u64,
            expected["columns"].as_u64().unwrap(),
            "columns"
        );
        assert_eq!(
            rendered.rows as u64,
            expected["rows"].as_u64().unwrap(),
            "rows"
        );
        let expected_id = expected["imageId"].as_u64();
        assert_eq!(rendered.image_id, expected_id, "image id");
    }
    // renderImage with an image id registered metadata into the global
    // registry (oracle: renderMetadataAfter).
    let expected_metadata = &oracle["renderMetadataAfter"];
    let line = rows(oracle, "renderCases")
        .iter()
        .find(|c| c["name"] == "kitty with id no move")
        .unwrap()["result"]["sequence"]
        .as_str()
        .unwrap()
        .to_owned();
    let metadata = get_kitty_image_metadata(&line).expect("global registry metadata after render");
    assert_eq!(
        metadata.image_id,
        expected_metadata["imageId"].as_u64().unwrap()
    );
    assert_eq!(
        metadata.columns as u64,
        expected_metadata["columns"].as_u64().unwrap()
    );
    assert_eq!(
        metadata.rows as u64,
        expected_metadata["rows"].as_u64().unwrap()
    );
    assert_eq!(
        metadata.width_px as u64,
        expected_metadata["widthPx"].as_u64().unwrap()
    );
    assert_eq!(
        metadata.height_px as u64,
        expected_metadata["heightPx"].as_u64().unwrap()
    );
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

/// Upstream "honors maxHeightCells by reducing rendered width" and the
/// cursor-movement renderImage tests, via the public API.
#[test]
fn render_image_matches_upstream_component_flows() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: Some("kitty"),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });

    // default keeps cursor movement (no C=1)
    let rendered = render_image(
        "AAAA",
        ImageDimensions {
            width_px: 20,
            height_px: 20,
        },
        ImageRenderOptions {
            max_width_cells: Some(2.0),
            ..ImageRenderOptions::default()
        },
    )
    .unwrap();
    assert!(!rendered.sequence.contains(",C=1,"));
    assert_eq!(rendered.rows, 2);

    // explicit moveCursor: false emits C=1
    let rendered = render_image(
        "AAAA",
        ImageDimensions {
            width_px: 20,
            height_px: 20,
        },
        ImageRenderOptions {
            max_width_cells: Some(2.0),
            move_cursor: Some(false),
            ..ImageRenderOptions::default()
        },
    )
    .unwrap();
    assert!(rendered.sequence.contains(",C=1,"));
    assert_eq!(rendered.rows, 2);

    // maxHeightCells shrinks width
    let rendered = render_image(
        "AAAA",
        ImageDimensions {
            width_px: 10,
            height_px: 100,
        },
        ImageRenderOptions {
            max_width_cells: Some(10.0),
            max_height_cells: Some(5.0),
            ..ImageRenderOptions::default()
        },
    )
    .unwrap();
    assert_eq!(rendered.rows, 5);
    assert!(rendered.sequence.contains(",c=1,r=5"));

    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

// ---------------------------------------------------------------- fallback
#[test]
fn oracle_image_fallback_strings() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    let _guard = state_lock().lock().unwrap();
    let home = oracle["fallbackHome"].as_str().unwrap();
    for case in rows(oracle, "fallback") {
        // environment-anchored: this row pins win32 `pathToFileURL` semantics
        // (`file:///C:/...`); the `url` crate resolves paths per host
        // platform, so a `C:/` input has no URL form on unix. The wrap flow
        // itself is replayed with a native absolute path in
        // `image_fallback_wraps_native_posix_paths`.
        if !cfg!(windows) && case["name"] == "hyperlinks wrap shortened path" {
            continue;
        }
        set_capabilities(capabilities_from_json(&case["caps"]));
        let dims = &case["dims"];
        let actual = image_fallback_with_home(
            case["mime"].as_str().unwrap(),
            if dims.is_null() {
                None
            } else {
                Some(image_dimensions_from_json(dims))
            },
            case["filename"].as_str(),
            Some(home),
        );
        assert_eq!(
            actual,
            case["result"].as_str().unwrap(),
            "fallback case {}",
            case["name"]
        );
    }
    reset_capabilities_cache();
}

/// Unix replay of the win32-oracle wrap row ("hyperlinks wrap shortened
/// path"): a native absolute home path still wraps in an OSC 8 file URL whose
/// body is the shortened display path — the flow the url crate realizes on
/// this platform.
#[cfg(unix)]
#[test]
fn image_fallback_wraps_native_posix_paths() {
    let _guard = state_lock().lock().unwrap();
    let home = dirs::home_dir()
        .expect("home dir exists")
        .to_string_lossy()
        .into_owned();
    let abs = format!("{home}/.pi/agent/shot.png");
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: true,
    });
    let result = image_fallback_with_home(
        "image/png",
        Some(ImageDimensions {
            width_px: 10,
            height_px: 10,
        }),
        Some(&abs),
        Some(&home),
    );
    let expected = format!(
        "[Image: \u{1b}]8;;file://{abs}\u{1b}\\~/.pi/agent/shot.png\u{1b}]8;;\u{1b}\\ [image/png] 10x10]"
    );
    assert_eq!(result, expected);
    reset_capabilities_cache();
}

/// Upstream imageFallback tests that shorten against the REAL home dir.
#[test]
fn image_fallback_shortens_real_home_paths() {
    let _guard = state_lock().lock().unwrap();
    let home = dirs::home_dir()
        .expect("home dir exists")
        .to_string_lossy()
        .into_owned();
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let abs = format!("{home}{separator}.pi{separator}agent{separator}shot.png");

    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });
    let result = image_fallback_with_home(
        "image/png",
        Some(ImageDimensions {
            width_px: 1280,
            height_px: 720,
        }),
        Some(&abs),
        Some(&home),
    );
    let shortened = format!("~{separator}.pi{separator}agent{separator}shot.png");
    assert_eq!(result, format!("[Image: {shortened} [image/png] 1280x720]"));

    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: true,
    });
    let result = image_fallback_with_home(
        "image/png",
        Some(ImageDimensions {
            width_px: 10,
            height_px: 10,
        }),
        Some(&abs),
        Some(&home),
    );
    assert!(
        result.contains("\x1b]8;;file://"),
        "expected OSC 8 file link: {result:?}"
    );
    let visible = result.replace('\\', "/");
    assert!(visible.contains(shortened.replace('\\', "/").as_str()));
    reset_capabilities_cache();
}

// ---------------------------------------------------------------- hyperlink
#[test]
fn oracle_hyperlink_sequences() {
    let oracle = oracle();
    for case in rows(oracle, "hyperlink") {
        let actual = hyperlink(
            case["text"].as_str().unwrap(),
            case["url"].as_str().unwrap(),
        );
        assert_eq!(
            actual,
            case["result"].as_str().unwrap(),
            "hyperlink {:?}",
            case["text"]
        );
    }
}

#[test]
fn hyperlink_wraps_ansi_styled_and_empty_text() {
    let styled = "\x1b[4m\x1b[34mclick me\x1b[0m";
    let result = hyperlink(styled, "https://example.com");
    assert!(result.starts_with("\x1b]8;;https://example.com\x1b\\"));
    assert!(result.contains(styled));
    assert!(result.ends_with("\x1b]8;;\x1b\\"));
    assert_eq!(
        hyperlink("", "https://example.com"),
        "\x1b]8;;https://example.com\x1b\\\x1b]8;;\x1b\\"
    );
    let readme = hyperlink("README.md", "file:///home/user/README.md");
    assert!(readme.contains("file:///home/user/README.md"));
    assert!(readme.contains("README.md"));
}

// ---------------------------------------------------------------- isImageLine
/// The programmatic >256-char lines the oracle collapsed to
/// `{"long":true,"result":...}`; same construction, same order.
fn long_line(idx: usize) -> String {
    match idx {
        0 => format!(
            "Text prefix \x1b]1337;File=size=800,600;inline=1:{} suffix",
            "A".repeat(100).repeat(3000)
        ),
        1 => format!(
            "Text before \x1b_Ga=T,f=100{} text after",
            "A".repeat(300000)
        ),
        2 => format!(
            "Text before \x1b]1337;File=size=800,600;inline=1:{} text after",
            "B".repeat(300000)
        ),
        3 => format!(
            "Output: \x1b]1337;File=size=800,600;inline=1:{} end of output",
            "A".repeat(100).repeat(3040)
        ),
        4 => format!(
            "Text\x1b_Ga=T,f=100{}End",
            "A".repeat(58649 - 4 - "\x1b_Ga=T,f=100".len() - 3)
        ),
        5 => "A".repeat(100000),
        other => panic!("no long line {other}"),
    }
}

#[test]
fn oracle_is_image_line_all_positions_and_lengths() {
    let oracle = oracle();
    oracle_sha_matches_upstream(oracle);
    let mut long_idx = 0usize;
    for (i, row) in rows(oracle, "isImageLine").iter().enumerate() {
        let expected = row["result"].as_bool().unwrap();
        if row["long"].as_bool().unwrap_or(false) {
            let line = long_line(long_idx);
            long_idx += 1;
            assert_eq!(
                is_image_line(&line),
                expected,
                "long isImageLine row {i} (long #{})",
                long_idx - 1
            );
        } else {
            let line = row["line"].as_str().unwrap();
            assert_eq!(
                is_image_line(line),
                expected,
                "isImageLine row {i}: {line:?}"
            );
        }
    }
    assert_eq!(long_idx, 6, "every collapsed long row was replayed");
}

/// Port of `bug-regression-isimageline-startswith-bug.test.ts`: detection
/// works regardless of position, even on crash-log-sized lines, and plain
/// text/paths never trigger.
#[test]
fn is_image_line_regressions_from_upstream_bug_test() {
    let scenarios = [
        "At start: \x1b_Ga=T,f=100,data...\x1b\\".to_owned(),
        "Prefix \x1b_Ga=T,data...\x1b\\".to_owned(),
        "Suffix text \x1b_Ga=T,data...\x1b\\ suffix".to_owned(),
        "Middle \x1b_Ga=T,data...\x1b\\ more text".to_owned(),
        format!(
            "Text before \x1b_Ga=T,f=100{} text after",
            "A".repeat(300000)
        ),
    ];
    for line in &scenarios {
        assert!(
            is_image_line(line),
            "kitty sequence in {:?}",
            &line[..line.len().min(50)]
        );
    }
    let iterm_scenarios = [
        "At start: \x1b]1337;File=size=100,100:base64...\x07".to_owned(),
        "Prefix \x1b]1337;File=inline=1:data==\x07".to_owned(),
        "Suffix text \x1b]1337;File=inline=1:data==\x07 suffix".to_owned(),
        "Middle \x1b]1337;File=inline=1:data==\x07 more text".to_owned(),
        format!(
            "Text before \x1b]1337;File=size=800,600;inline=1:{} text after",
            "B".repeat(300000)
        ),
    ];
    for line in &iterm_scenarios {
        assert!(
            is_image_line(line),
            "iterm2 sequence in {:?}",
            &line[..line.len().min(50)]
        );
    }
    let crash_line = format!(
        "Output: \x1b]1337;File=size=800,600;inline=1:{} end of output",
        "A".repeat(100).repeat(3040)
    );
    assert!(crash_line.len() > 300000);
    assert!(is_image_line(&crash_line));
    // 58649-char line shaped like the crash log
    let line = format!(
        "Text\x1b_Ga=T,f=100{}End",
        "A".repeat(58649 - 4 - "\x1b_Ga=T,f=100".len() - 3)
    );
    assert_eq!(line.chars().count(), 58649);
    assert!(is_image_line(&line));
    assert!(!is_image_line(&"A".repeat(100000)));
    for path in [
        "/path/to/1337/image.jpg",
        "/usr/local/bin/File_converter",
        "~/Documents/1337File_backup.png",
        "./_G_test_file.txt",
    ] {
        assert!(!is_image_line(path), "false positive on {path}");
    }
}

// ---------------------------------------------------------------- allocate
#[test]
fn allocate_image_id_stays_in_contracted_range() {
    for _ in 0..1000 {
        let id = allocate_image_id();
        assert!(id >= 1, "id {id} below the [1, 0xffffffff] range");
        assert!(id <= 0xffff_ffff, "id {id} above the [1, 0xffffffff] range");
    }
    let oracle = oracle();
    let bounds = &oracle["allocateImageId"];
    assert!(bounds["allInteger"].as_bool().unwrap());
    assert!(bounds["min"].as_u64().unwrap() >= 1);
    assert!(bounds["max"].as_u64().unwrap() <= 0xffff_ffff);
}

// ---------------------------------------------------------------- cell dims
#[test]
fn cell_dimensions_default_and_round_trip() {
    let _guard = state_lock().lock().unwrap();
    assert_eq!(
        get_cell_dimensions(),
        CellDimensions {
            width_px: 9,
            height_px: 18
        }
    );
    set_cell_dimensions(CellDimensions {
        width_px: 7,
        height_px: 11,
    });
    assert_eq!(
        get_cell_dimensions(),
        CellDimensions {
            width_px: 7,
            height_px: 11
        }
    );
    set_cell_dimensions(CellDimensions::default());
    assert_eq!(
        get_cell_dimensions(),
        CellDimensions {
            width_px: 9,
            height_px: 18
        }
    );
}

// ---------------------------------------------------------------- prefixes
#[test]
fn image_line_prefixes_match_upstream_constants() {
    assert_eq!(KITTY_PREFIX, "\x1b_G");
    assert_eq!(ITERM2_PREFIX, "\x1b]1337;File=");
}
