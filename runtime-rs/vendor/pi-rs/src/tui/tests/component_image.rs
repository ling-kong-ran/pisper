//! Test port for `components/image.ts` (upstream
//! `test/terminal-image.test.ts` Image-component cases plus the
//! component-level essence of the registered
//! `tui-alt-screen.test.ts` "does not emit Kitty graphics commands or OSC 133
//! zones in iTerm2" case) and a byte-for-byte oracle replay.
//!
//! Evidence: `tests/fixtures/component_image_oracle/oracle.mjs` ran the REAL
//! upstream component (with the real `terminal-image.ts` and `utils.ts`)
//! under node v25.8.2 and captured 20 scenarios / 28 render probes into the
//! embedded JSON below (canonical form, sha256
//! f1df0abfe8902c104e2c46ec3183d57068abf31c3428c00538236f43bfffc916). The
//! only masked bytes are randomly allocated Kitty ids (upstream
//! `Math.random`): masked rows compare with `,i=<ID>` and the contractual
//! integer range [1, 0xffffffff]. The single home-shortening scenario is
//! captured with node's `USERPROFILE` pinned to a fake home and replayed by
//! recomposing theme+truncation from the captured raw fallback string:
//! `dirs` 7 on Windows resolves the profile through
//! `SHGetKnownFolderPath(FOLDERID_Profile)` and ignores the environment, so
//! the fake home cannot be injected into the component under test (the same
//! home-injection substitution as the terminal-image slice); the component's
//! real-home fallback behavior is covered structurally by the upstream test
//! port below, exactly as the upstream suite asserts it.
//!
//! Every test in this module serializes on [`STATE_LOCK`]; the project gates
//! run the suite with `--test-threads=1`.

use super::{Component, Image, ImageOptions, ImageTheme};
use crate::tui::terminal_image::{
    reset_capabilities_cache, set_capabilities, set_cell_dimensions, CellDimensions,
    TerminalCapabilities,
};
use crate::tui::utils::{truncate_to_width, visible_width};
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

fn state_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn oracle() -> &'static Value {
    static ORACLE: OnceLock<Value> = OnceLock::new();
    ORACLE.get_or_init(|| serde_json::from_str(ORACLE_JSON).expect("oracle JSON parses"))
}

fn identity_theme() -> ImageTheme {
    ImageTheme {
        fallback_color: Arc::new(|value| value.to_owned()),
    }
}

fn yellow_theme() -> ImageTheme {
    ImageTheme {
        fallback_color: Arc::new(|value| format!("\x1b[33m{value}\x1b[0m")),
    }
}

fn theme(kind: &str) -> ImageTheme {
    match kind {
        "identity" => identity_theme(),
        "yellow" => yellow_theme(),
        other => panic!("unknown oracle theme {other}"),
    }
}

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

fn cell_from_json(value: &Value) -> CellDimensions {
    CellDimensions {
        width_px: value["widthPx"].as_u64().unwrap() as usize,
        height_px: value["heightPx"].as_u64().unwrap() as usize,
    }
}

/// Mirrors the oracle's `line.replace(/,i=\d+/, ",i=<ID>")`: rewrite the
/// first `,i=<digits>` occurrence so randomly allocated ids compare equal.
fn mask_allocated_id(line: &str) -> String {
    let Some(start) = line.find(",i=") else {
        return line.to_owned();
    };
    let bytes = line.as_bytes();
    let digits_start = start + ",i=".len();
    let mut end = digits_start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end > digits_start {
        format!("{}<ID>{}", &line[..digits_start], &line[end..])
    } else {
        line.to_owned()
    }
}

// ---------------------------------------------------------------- oracle

#[test]
fn oracle_component_scenarios_byte_exact() {
    let _guard = state_lock().lock().unwrap();
    let oracle = oracle();
    assert_eq!(
        oracle["meta"]["upstreamImageSha256"].as_str().unwrap(),
        "e92c3f66f36eb579ff1eabd9dc3b9f34bd8a07997bd62ffe3fb63a21f97a7462",
        "oracle was captured against a different upstream components/image.ts",
    );
    assert_eq!(
        oracle["meta"]["upstreamTerminalImageSha256"]
            .as_str()
            .unwrap(),
        "26beb4d4154a6a7fc800d40af39cde8984749d14096756182faba4af0380093d",
        "oracle was captured against a different upstream terminal-image.ts",
    );
    assert_eq!(
        oracle["meta"]["upstreamUtilsSha256"].as_str().unwrap(),
        "258ff73a0ff4d2b05f8a60515a9cc37eb96c9fc03a4072ac6db2906cad1862d9",
        "oracle was captured against a different upstream utils.ts",
    );
    assert!(oracle["allocatedIdFacts"]["allInteger"].as_bool().unwrap());
    assert!(oracle["allocatedIdFacts"]["inRange"].as_bool().unwrap());

    let mut masked_probes = 0usize;
    let mut render_probes = 0usize;
    for (i, scenario) in oracle["scenarios"].as_array().unwrap().iter().enumerate() {
        // environment-anchored: the two wrap scenarios pin win32
        // `pathToFileURL` semantics (`file:///C:/...`); the `url` crate
        // resolves paths per host platform, so a `C:/` input has no URL form
        // on unix and the fallback renders unwrapped there. The wrap flow on
        // unix is covered by terminal_image's
        // `image_fallback_wraps_native_posix_paths`.
        if !cfg!(windows)
            && matches!(
                scenario["name"].as_str(),
                Some("hyperlinks wrap absolute path") | Some("home path shortened and hyperlinked")
            )
        {
            continue;
        }
        set_capabilities(capabilities_from_json(&scenario["caps"]));
        set_cell_dimensions(cell_from_json(&scenario["cell"]));
        let ctor = &scenario["ctorOptions"];
        let options = ImageOptions {
            max_width_cells: ctor["maxWidthCells"].as_f64(),
            max_height_cells: ctor["maxHeightCells"].as_f64(),
            filename: ctor["filename"].as_str().map(str::to_owned),
            image_id: ctor["imageId"].as_u64(),
        };
        let dimensions =
            scenario["ctorDimensions"]
                .as_object()
                .map(|dims| super::ImageDimensions {
                    width_px: dims["widthPx"].as_u64().unwrap() as usize,
                    height_px: dims["heightPx"].as_u64().unwrap() as usize,
                });
        let mut image = Image::with_options(
            scenario["base64"].as_str().unwrap(),
            scenario["mimeType"].as_str().unwrap(),
            theme(scenario["theme"].as_str().unwrap()),
            options,
            dimensions,
        );

        for (j, probe) in scenario["renders"].as_array().unwrap().iter().enumerate() {
            let label = || format!("scenario {i} ({}) probe {j}", scenario["name"]);
            match probe["op"].as_str().unwrap() {
                "invalidate" => Component::invalidate(&mut image),
                "render" => {
                    render_probes += 1;
                    let width = probe["width"].as_u64().unwrap() as usize;
                    let expected: Vec<String> = probe["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|line| line.as_str().unwrap().to_owned())
                        .collect();
                    // The home-shortening scenario was captured with node's
                    // homedir pinned; dirs 7 (SHGetKnownFolderPath) cannot be
                    // redirected via the environment, so it replays through
                    // the captured raw fallback string instead of the
                    // component (disclosed home-injection substitution).
                    let home_pinned = scenario["homePinned"].as_bool().unwrap();
                    let (lines, masked) = if home_pinned {
                        let composed = (theme(scenario["theme"].as_str().unwrap()).fallback_color)(
                            scenario["fallback"].as_str().unwrap(),
                        );
                        (
                            vec![truncate_to_width(&composed, width, "...", false)],
                            false,
                        )
                    } else {
                        let lines = image.render(width);
                        (lines, probe["masked"].as_bool().unwrap())
                    };
                    if masked {
                        masked_probes += 1;
                        let id = image
                            .get_image_id()
                            .unwrap_or_else(|| panic!("{}: expected an allocated id", label()));
                        assert!(
                            (1..=0xffff_ffff).contains(&id),
                            "{}: allocated id {id} outside [1, 0xffffffff]",
                            label()
                        );
                        let masked: Vec<String> =
                            lines.iter().map(|line| mask_allocated_id(line)).collect();
                        assert_eq!(masked, expected, "{}: masked lines", label());
                    } else {
                        assert_eq!(lines, expected, "{}: lines", label());
                        if !home_pinned {
                            assert_eq!(
                                image.get_image_id(),
                                probe["imageId"].as_u64(),
                                "{}: image id",
                                label()
                            );
                        }
                    }
                    for (line, visible) in lines
                        .iter()
                        .zip(probe["visibleWidths"].as_array().unwrap().iter())
                    {
                        assert_eq!(
                            visible_width(line) as u64,
                            visible.as_u64().unwrap(),
                            "{}: visible width",
                            label()
                        );
                    }
                }
                other => panic!("{}: unknown probe op {other}", label()),
            }
        }
        reset_capabilities_cache();
        set_cell_dimensions(CellDimensions::default());
    }
    assert!(
        masked_probes >= 2,
        "the upstream tests allocate ids in two scenarios; got {masked_probes}"
    );
    // the two win32-URL wrap scenarios (one render probe each) are skipped
    // on unix — see the loop head for the platform seam
    #[cfg(windows)]
    let expected_render_probes = 27;
    #[cfg(unix)]
    let expected_render_probes = 25;
    assert_eq!(
        render_probes, expected_render_probes,
        "every captured render probe was replayed (plus one invalidate probe)"
    );
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

// ------------------------------------------- upstream terminal-image.test.ts

/// Upstream "caps Image component height to a square pixel box by default".
#[test]
fn caps_image_component_height_to_square_pixel_box_by_default() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: Some("kitty"),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 20,
    });
    let mut image = Image::with_options(
        "AAAA",
        "image/png",
        identity_theme(),
        ImageOptions {
            max_width_cells: Some(10.0),
            ..ImageOptions::default()
        },
        Some(super::ImageDimensions {
            width_px: 10,
            height_px: 100,
        }),
    );
    let lines = image.render(12);
    assert_eq!(lines.len(), 5);
    assert!(lines[0].contains(",c=1,r=5"), "lines[0]: {:?}", lines[0]);
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

/// Upstream "places image sequence on first line with empty padding rows".
#[test]
fn places_image_sequence_on_first_line_with_empty_padding_rows() {
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
    let mut image = Image::with_options(
        "AAAA",
        "image/png",
        identity_theme(),
        ImageOptions {
            max_width_cells: Some(2.0),
            ..ImageOptions::default()
        },
        Some(super::ImageDimensions {
            width_px: 20,
            height_px: 20,
        }),
    );
    let lines = image.render(4);
    let image_id = image.get_image_id();
    assert!(image_id.is_some(), "expected a numeric Kitty image id");
    let id = image_id.unwrap();
    assert!((1..=0xffff_ffff).contains(&id), "id {id} outside the range");
    assert!(lines[0].starts_with("\x1b_G"), "lines[0]: {:?}", lines[0]);
    assert!(lines[0].contains(",C=1,"), "lines[0]: {:?}", lines[0]);
    assert!(
        lines[0].contains(&format!(",i={id}")),
        "lines[0]: {:?}",
        lines[0]
    );
    assert!(lines[0].ends_with("\x1b\\"), "lines[0]: {:?}", lines[0]);
    assert_eq!(lines[1..], [""]);
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

/// Upstream "truncates long image fallback lines to render width".
#[test]
fn truncates_long_image_fallback_lines_to_render_width() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });
    let home = dirs::home_dir().expect("home dir exists");
    let long_path = std::path::Path::new(&home)
        .join("images")
        .join(format!(
            "{}.png",
            "generated-image-with-a-very-long-absolute-path".repeat(4)
        ))
        .to_string_lossy()
        .into_owned();
    let width = 40;
    let mut image = Image::with_options(
        "AAAA",
        "image/png",
        yellow_theme(),
        ImageOptions {
            filename: Some(long_path),
            ..ImageOptions::default()
        },
        Some(super::ImageDimensions {
            width_px: 1280,
            height_px: 720,
        }),
    );
    let lines = image.render(width);
    assert_eq!(lines.len(), 1);
    assert!(
        visible_width(&lines[0]) <= width,
        "fallback line wider than {width}: visible={} raw={:?}",
        visible_width(&lines[0]),
        lines[0]
    );
    assert!(
        lines[0].contains("..."),
        "expected ellipsis when truncating long fallback path"
    );
    assert!(
        lines[0].contains("~"),
        "expected home-shortened path in fallback"
    );
    reset_capabilities_cache();
}

// ------------------------------------------------ upstream-registered case

/// Component-level essence of the registered `tui-alt-screen.test.ts` case
/// "does not emit Kitty graphics commands or OSC 133 zones in iTerm2": with
/// `images: "iterm2"` the component itself emits the OSC 1337 placement
/// (never `\x1b_G`), and once the alt screen pins images off before terminal
/// start (`beforeTerminalStart`), the same component renders the "[Image:"
/// text fallback instead.
#[test]
fn iterm2_component_emits_osc1337_and_falls_back_when_images_disabled() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: Some("iterm2"),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });
    let mut image = Image::with_options(
        "AAAA",
        "image/png",
        identity_theme(),
        ImageOptions {
            filename: Some("example.png".to_owned()),
            ..ImageOptions::default()
        },
        Some(super::ImageDimensions {
            width_px: 10,
            height_px: 10,
        }),
    );
    let lines = image.render(20);
    let joined = lines.join("");
    assert!(!joined.contains("\x1b_G"), "no Kitty graphics commands");
    assert!(
        joined.contains("\x1b]1337;File="),
        "expected the OSC 1337 placement: {joined:?}"
    );

    // Alt-screen start pins images off (`setCapabilities({...caps, images: null})`)
    // and invalidates every component.
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: true,
        hyperlinks: true,
    });
    image.invalidate();
    let lines = image.render(20);
    let joined = lines.join("");
    assert!(
        joined.contains("[Image:"),
        "expected the text fallback after images are pinned off: {joined:?}"
    );
    assert!(
        !joined.contains("\x1b]1337;File=") && !joined.contains("\x1b_G"),
        "no image protocol sequences after images are pinned off: {joined:?}"
    );
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

// ----------------------------------------------------- component semantics

/// An explicit `imageId` of 0 must survive: `0 !== undefined` skips
/// allocation and the truthy `if (result.imageId)` adoption never fires
/// (upstream `encodeKitty` also treats 0 as falsy and omits `i=`).
#[test]
fn kitty_image_id_zero_stays_untouched() {
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
    let mut image = Image::with_options(
        "AAAA",
        "image/png",
        identity_theme(),
        ImageOptions {
            max_width_cells: Some(2.0),
            image_id: Some(0),
            ..ImageOptions::default()
        },
        Some(super::ImageDimensions {
            width_px: 20,
            height_px: 20,
        }),
    );
    let lines = image.render(4);
    assert_eq!(image.get_image_id(), Some(0), "id 0 must not be replaced");
    assert!(
        !lines[0].contains(",i="),
        "encodeKitty omits the falsy id: {:?}",
        lines[0]
    );
    assert!(lines[0].starts_with("\x1b_Ga=T"));
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

/// The rendered lines are cached per width; a same-width render must serve
/// the cache even after the global cell dimensions changed, and
/// `invalidate()` (the TUI resize path) must force a recompute.
#[test]
fn render_cache_is_keyed_by_width_and_invalidate_forces_recompute() {
    let _guard = state_lock().lock().unwrap();
    set_capabilities(TerminalCapabilities {
        images: Some("kitty"),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 9,
        height_px: 18,
    });
    let mut image = Image::new("AAAA", "image/png", identity_theme());
    let before = image.render(30);
    let id = image.get_image_id().expect("kitty allocated an id");

    set_cell_dimensions(CellDimensions {
        width_px: 10,
        height_px: 10,
    });
    let cached = image.render(30);
    let masked = |lines: &[String]| -> Vec<String> {
        lines.iter().map(|line| mask_allocated_id(line)).collect()
    };
    assert_eq!(
        masked(&cached),
        masked(&before),
        "same width must serve the cache regardless of cell size changes"
    );

    image.invalidate();
    let recomputed = image.render(30);
    assert_eq!(image.get_image_id(), Some(id), "invalidate keeps the id");
    assert_ne!(
        recomputed.len(),
        before.len(),
        "recompute must observe the new cell dimensions"
    );
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions::default());
}

/// Component trait mapping: the image is a plain leaf widget.
#[test]
fn component_trait_defaults_match_upstream_shape() {
    let mut image = Image::new("AAAA", "image/png", identity_theme());
    assert!(!image.is_focusable());
    assert!(!image.focused());
    assert!(image.layout_node_mut().is_none());
    assert!(image.layout_cache_id().is_none());
    let event = crate::tui::component::TuiMouseEvent {
        event_type: crate::tui::component::TuiMouseEventType::Press,
        button: crate::tui::component::TuiMouseButton::Left,
        x: 0,
        y: 0,
        screen_x: 0,
        screen_y: 0,
        width: 0,
        height: 0,
        shift: false,
        alt: false,
        ctrl: false,
        wheel_delta: None,
        click_count: None,
    };
    assert!(image.mouse_action(&event).is_none());
    image.handle_input("\x03");
}

const ORACLE_JSON: &str = r#"{"allocatedIdFacts":{"allInteger":true,"inRange":true},"meta":{"node":"v22.14.0","platform":"win32","upstreamImageSha256":"e92c3f66f36eb579ff1eabd9dc3b9f34bd8a07997bd62ffe3fb63a21f97a7462","upstreamTerminalImageSha256":"26beb4d4154a6a7fc800d40af39cde8984749d14096756182faba4af0380093d","upstreamUtilsSha256":"258ff73a0ff4d2b05f8a60515a9cc37eb96c9fc03a4072ac6db2906cad1862d9"},"renderImageSmoke":{"columns":2,"rows":2,"sequence":"\u001b_Ga=T,f=100,q=2,c=2,r=2;AAAA\u001b\\"},"scenarios":[{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":20,"widthPx":10},"ctorDimensions":{"heightPx":100,"widthPx":10},"ctorOptions":{"maxWidthCells":10},"fallback":"[Image: [image/png] 10x100]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"caps height square pixel box","renders":[{"imageId":"<allocated>","lines":["\u001b_Ga=T,f=100,q=2,C=1,c=1,r=5,i=<ID>;AAAA\u001b\\","","","",""],"masked":true,"op":"render","visibleWidths":[0,0,0,0,0],"width":12},{"imageId":"<allocated>","lines":["\u001b_Ga=T,f=100,q=2,C=1,c=1,r=5,i=<ID>;AAAA\u001b\\","","","",""],"masked":true,"op":"render","visibleWidths":[0,0,0,0,0],"width":12},{"imageId":"<allocated>","lines":["\u001b_Ga=T,f=100,q=2,C=1,c=1,r=5,i=<ID>;AAAA\u001b\\","","","",""],"masked":true,"op":"render","visibleWidths":[0,0,0,0,0],"width":20}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":20,"widthPx":20},"ctorOptions":{"maxWidthCells":2},"fallback":"[Image: [image/png] 20x20]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"first line placement padding rows","renders":[{"imageId":"<allocated>","lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=<ID>;AAAA\u001b\\",""],"masked":true,"op":"render","visibleWidths":[0,0],"width":4}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":20,"widthPx":20},"ctorOptions":{"imageId":42307,"maxWidthCells":2},"fallback":"[Image: [image/png] 20x20]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"explicit id reuse across widths","renders":[{"imageId":42307,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42307;AAAA\u001b\\",""],"masked":false,"op":"render","visibleWidths":[0,0],"width":4},{"imageId":42307,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42307;AAAA\u001b\\",""],"masked":false,"op":"render","visibleWidths":[0,0],"width":4},{"imageId":42307,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42307;AAAA\u001b\\",""],"masked":false,"op":"render","visibleWidths":[0,0],"width":8},{"op":"invalidate"},{"imageId":42307,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2,i=42307;AAAA\u001b\\",""],"masked":false,"op":"render","visibleWidths":[0,0],"width":6}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":20,"widthPx":20},"ctorOptions":{"imageId":0,"maxWidthCells":2},"fallback":"[Image: [image/png] 20x20]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"image id zero preserved","renders":[{"imageId":0,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=2,r=2;AAAA\u001b\\",""],"masked":false,"op":"render","visibleWidths":[0,0],"width":4}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":100,"widthPx":10},"ctorOptions":{"imageId":42308,"maxHeightCells":5,"maxWidthCells":10},"fallback":"[Image: [image/png] 10x100]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"explicit maxHeightCells","renders":[{"imageId":42308,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=1,r=5,i=42308;AAAA\u001b\\","","","",""],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0],"width":40}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":600,"widthPx":800},"ctorOptions":{"imageId":42309,"maxWidthCells":7.9},"fallback":"[Image: [image/png] 800x600]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"fractional maxWidthCells","renders":[{"imageId":42309,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=7,r=3,i=42309;AAAA\u001b\\","",""],"masked":false,"op":"render","visibleWidths":[0,0,0],"width":30}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":null,"ctorOptions":{"imageId":42310},"fallback":"[Image: [image/png]]","group":"kitty","homePinned":false,"mimeType":"image/png","name":"default dimensions on failed detection","renders":[{"imageId":42310,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=28,r=11,i=42310;AAAA\u001b\\","","","","","","","","","",""],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0,0,0,0,0,0,0],"width":30}],"theme":"identity"},{"base64":"iVBORw0KGgoAAAANSUhEUgAABQAAAALQAAAAAA==","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":null,"ctorOptions":{"imageId":42311},"fallback":null,"group":"kitty","homePinned":false,"mimeType":"image/png","name":"detected png dimensions","renders":[{"imageId":42311,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=28,r=8,i=42311;iVBORw0KGgoAAAANSUhEUgAABQAAAALQAAAAAA==\u001b\\","","","","","","",""],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0,0,0,0],"width":30},{"imageId":42311,"lines":["\u001b_Ga=T,f=100,q=2,C=1,c=60,r=17,i=42311;iVBORw0KGgoAAAANSUhEUgAABQAAAALQAAAAAA==\u001b\\","","","","","","","","","","","","","","","",""],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"width":80}],"theme":"identity"},{"base64":"iVBORw0KGgoAAAANSUhEUgAAAoAAAAHgAAAAAA==","caps":{"hyperlinks":true,"images":"kitty","trueColor":true},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":null,"ctorOptions":{"imageId":42312},"fallback":null,"group":"kitty","homePinned":false,"mimeType":"image/gif","name":"mime mismatch falls back to defaults","renders":[{"imageId":42312,"lines":["[Image: [\u001b[0m...\u001b[0m"],"masked":false,"op":"render","visibleWidths":[12],"width":12}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"iterm2","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":10,"widthPx":10},"ctorOptions":{},"fallback":"[Image: [image/png] 10x10]","group":"iterm2","homePinned":false,"mimeType":"image/png","name":"placement with move up prefix","renders":[{"imageId":null,"lines":["","","","","","","","","","","","","","","","","","\u001b[17A\u001b]1337;File=inline=1;size=3;width=18;height=auto:AAAA\u0007"],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,4],"width":20},{"imageId":null,"lines":["\u001b]1337;File=inline=1;size=3;width=1;height=auto:AAAA\u0007"],"masked":false,"op":"render","visibleWidths":[0],"width":1}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"iterm2","trueColor":true},"cell":{"heightPx":10,"widthPx":10},"ctorDimensions":{"heightPx":20,"widthPx":20},"ctorOptions":{"imageId":42442},"fallback":"[Image: [image/png] 20x20]","group":"iterm2","homePinned":false,"mimeType":"image/png","name":"constructor id untouched","renders":[{"imageId":42442,"lines":["","\u001b[1A\u001b]1337;File=inline=1;size=3;width=2;height=auto:AAAA\u0007"],"masked":false,"op":"render","visibleWidths":[0,3],"width":4}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":"iterm2","trueColor":true},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":null,"ctorOptions":{},"fallback":"[Image: [image/png]]","group":"iterm2","homePinned":false,"mimeType":"image/png","name":"default dimensions rows","renders":[{"imageId":null,"lines":["","","","","","","","","","","\u001b[10A\u001b]1337;File=inline=1;size=3;width=28;height=auto:AAAA\u0007"],"masked":false,"op":"render","visibleWidths":[0,0,0,0,0,0,0,0,0,0,4],"width":30}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":false,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":720,"widthPx":1280},"ctorOptions":{"filename":"C:\\images\\generated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-path.png"},"fallback":"[Image: C:\\images\\generated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-pathgenerated-image-with-a-very-long-absolute-path.png [image/png] 1280x720]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"long absolute path truncated to width","renders":[{"imageId":null,"lines":["\u001b[33m[Image: C:\\images\\generated-image-wit\u001b[0m...\u001b[0m"],"masked":false,"op":"render","visibleWidths":[40],"width":40}],"theme":"yellow"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":10,"widthPx":10},"ctorOptions":{"filename":"C:/images/pics/shot.png"},"fallback":"[Image: \u001b]8;;file:///C:/images/pics/shot.png\u001b\\C:/images/pics/shot.png\u001b]8;;\u001b\\ [image/png] 10x10]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"hyperlinks wrap absolute path","renders":[{"imageId":null,"lines":["[Image: \u001b]8;;file:///C:/images/pics/shot.png\u001b\\C:/images/pics/shot.png\u001b]8;;\u001b\\ [image/png] 10x10]"],"masked":false,"op":"render","visibleWidths":[50],"width":200}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":1,"widthPx":1},"ctorOptions":{"filename":"clankolas.png"},"fallback":"[Image: clankolas.png [image/png] 1x1]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"basename not hyperlinked","renders":[{"imageId":null,"lines":["[Image: clankolas.png [image/png] 1x1]"],"masked":false,"op":"render","visibleWidths":[38],"width":200}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":false,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":6,"widthPx":8},"ctorOptions":{},"fallback":"[Image: [image/png] 8x6]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"no filename segment","renders":[{"imageId":null,"lines":["[Image: [image/png] 8x6]"],"masked":false,"op":"render","visibleWidths":[24],"width":200}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":4,"widthPx":3},"ctorOptions":{"filename":"sub/dir/pic.jpg"},"fallback":"[Image: sub/dir/pic.jpg [image/png] 3x4]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"relative path kept as-is","renders":[{"imageId":null,"lines":["[Image: sub/dir/pic.jpg [image/png] 3x4]"],"masked":false,"op":"render","visibleWidths":[40],"width":200}],"theme":"identity"},{"base64":"AAAA","caps":{"hyperlinks":false,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":1080,"widthPx":1920},"ctorOptions":{"filename":"C:/media/photo album.png"},"fallback":"[Image: C:/media/photo album.png [image/png] 1920x1080]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"wide render keeps fallback intact","renders":[{"imageId":null,"lines":["\u001b[33m[Image: C:/media/photo album.png [image/png] 1920x1080]\u001b[0m"],"masked":false,"op":"render","visibleWidths":[55],"width":120}],"theme":"yellow"},{"base64":"AAAA","caps":{"hyperlinks":false,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":1080,"widthPx":1920},"ctorOptions":{"filename":"C:/media/photo album.png"},"fallback":"[Image: C:/media/photo album.png [image/png] 1920x1080]","group":"fallback","homePinned":false,"mimeType":"image/png","name":"narrow render truncates colored fallback","renders":[{"imageId":null,"lines":["\u001b[33m[Image: C:/media/phot\u001b[0m...\u001b[0m"],"masked":false,"op":"render","visibleWidths":[24],"width":24}],"theme":"yellow"},{"base64":"AAAA","caps":{"hyperlinks":true,"images":null,"trueColor":false},"cell":{"heightPx":18,"widthPx":9},"ctorDimensions":{"heightPx":10,"widthPx":10},"ctorOptions":{"filename":"C:/Users/oracle-user/pics/shot.png"},"fallback":"[Image: \u001b]8;;file:///C:/Users/oracle-user/pics/shot.png\u001b\\~/pics/shot.png\u001b]8;;\u001b\\ [image/png] 10x10]","group":"fallback","homePinned":true,"mimeType":"image/png","name":"home path shortened and hyperlinked","renders":[{"lines":["[Image: \u001b]8;;file:///C:/Users/oracle-user/pics/shot.png\u001b\\~/pics/shot.png\u001b]8;;\u001b\\ [image/png] 10x10]"],"masked":false,"op":"render","visibleWidths":[42],"width":200}],"theme":"identity"}]}"#;
