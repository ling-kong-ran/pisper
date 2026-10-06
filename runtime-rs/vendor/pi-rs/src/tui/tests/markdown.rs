//! Differential tests for the Markdown component against fixtures generated
//! from the ACTUAL upstream `markdown.ts` (pi @ 590144609) executed under
//! Node 25 with the unpacked `marked@18.0.11` reference dependency (the exact
//! upstream pin, restored from the npm content cache; SHA-512 matches
//! package-lock.json). The stored corpora were regenerated for 18.0.11; the
//! 18.0.5-substitution deviation is resolved. Supplementary delta oracles:
//! `markdown_18_0_11_fixtures.json` (render) and
//! `markdown_tokens_fixtures.json` (token trees, consumed by
//! markdown_lexer.rs tests).
//!
//! The chalk level-3 theme used by upstream tests is reproduced by the
//! `Chalk` emulator below (algorithm ported from chalk 5.6.2 `applyStyle`).

use std::sync::Arc;
use std::sync::Mutex;

use serde::Deserialize;

use crate::tui::components::markdown::{
    DefaultTextStyle, Markdown, MarkdownOptions, MarkdownTheme,
};
use crate::tui::terminal_image::{set_capabilities, TerminalCapabilities};
use crate::tui::utf16::{raw_text, Utf16Text};

// The capability cache is process-global (as upstream's module-level cache),
// so every markdown test holds this lock to stay mutually exclusive.
static CAPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// chalk 5.6.2 emulator (level 3)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Chalk(Vec<(&'static str, &'static str)>);

impl Chalk {
    fn paint(&self, s: impl Into<Utf16Text>) -> Utf16Text {
        let mut s = s.into();
        if s.is_empty() {
            return s;
        }
        let open: String = self.0.iter().map(|(o, _)| *o).collect();
        let close: String = self.0.iter().rev().map(|(_, c)| *c).collect();
        if s.contains('\u{1b}') {
            // Replace any instance of a chain member's close code with
            // close+open (chalk's stringReplaceAll keeps the search
            // substring and appends the replacer - reopen behavior).
            for (o, c) in self.0.iter().rev() {
                let mut rep = String::new();
                rep.push_str(c);
                rep.push_str(o);
                s = s.replace(*c, rep.as_str());
            }
        }
        // Close styling before a linebreak and reopen after the next line.
        if let Some(lf) = s.find('\n') {
            let mut out = Utf16Text::new();
            let mut end = 0usize;
            let mut index = lf;
            loop {
                let got_cr = index > 0 && s.as_units()[index - 1] == u16::from(b'\r');
                let cut = if got_cr { index - 1 } else { index };
                out.push_str(s.slice(end..cut));
                out.push_str(&close);
                out.push_str(if got_cr { "\r\n" } else { "\n" });
                out.push_str(&open);
                end = index + 1;
                match s.slice(end..).find('\n') {
                    Some(p) => index = end + p,
                    None => break,
                }
            }
            out.push_str(s.slice(end..));
            s = out;
        }
        raw_text!(open, s, close)
    }

    fn style(mut self, o: &'static str, c: &'static str) -> Self {
        self.0.push((o, c));
        self
    }
    fn bold(self) -> Self {
        self.style("\x1b[1m", "\x1b[22m")
    }
    fn dim(self) -> Self {
        self.style("\x1b[2m", "\x1b[22m")
    }
    fn italic(self) -> Self {
        self.style("\x1b[3m", "\x1b[23m")
    }
    fn underline(self) -> Self {
        self.style("\x1b[4m", "\x1b[24m")
    }
    fn strikethrough(self) -> Self {
        self.style("\x1b[9m", "\x1b[29m")
    }
    fn cyan(self) -> Self {
        self.style("\x1b[36m", "\x1b[39m")
    }
    fn blue(self) -> Self {
        self.style("\x1b[34m", "\x1b[39m")
    }
    fn yellow(self) -> Self {
        self.style("\x1b[33m", "\x1b[39m")
    }
    fn green(self) -> Self {
        self.style("\x1b[32m", "\x1b[39m")
    }
    fn bg_blue(self) -> Self {
        self.style("\x1b[44m", "\x1b[49m")
    }
}

fn style_arc(
    f: impl Fn(&Utf16Text) -> Utf16Text + Send + Sync + 'static,
) -> Arc<dyn Fn(&Utf16Text) -> Utf16Text + Send + Sync> {
    Arc::new(f)
}

/// Upstream `defaultMarkdownTheme` (test-themes.ts, chalk level 3).
fn default_markdown_theme() -> MarkdownTheme {
    let ch = Chalk(vec![]);
    MarkdownTheme {
        heading: style_arc(move |t| ch.clone().bold().cyan().paint(t)),
        link: style_arc(|t| Chalk(vec![]).blue().paint(t)),
        link_url: style_arc(|t| Chalk(vec![]).dim().paint(t)),
        code: style_arc(|t| Chalk(vec![]).yellow().paint(t)),
        code_block: style_arc(|t| Chalk(vec![]).green().paint(t)),
        code_block_border: style_arc(|t| Chalk(vec![]).dim().paint(t)),
        quote: style_arc(|t| Chalk(vec![]).italic().paint(t)),
        quote_border: style_arc(|t| Chalk(vec![]).dim().paint(t)),
        hr: style_arc(|t| Chalk(vec![]).dim().paint(t)),
        list_bullet: style_arc(|t| Chalk(vec![]).cyan().paint(t)),
        bold: style_arc(|t| Chalk(vec![]).bold().paint(t)),
        italic: style_arc(|t| Chalk(vec![]).italic().paint(t)),
        strikethrough: style_arc(|t| Chalk(vec![]).strikethrough().paint(t)),
        underline: style_arc(|t| Chalk(vec![]).underline().paint(t)),
        highlight_code: None,
        code_block_indent: None,
    }
}

fn chalk_of(style: &str) -> Chalk {
    let _ = style;
    Chalk(vec![])
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureFile {
    #[allow(dead_code)]
    theme_probe: serde_json::Value,
    #[allow(dead_code)]
    deviation_checks: serde_json::Value,
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureCase {
    name: String,
    md: String,
    width: usize,
    padding_x: usize,
    padding_y: usize,
    style: String,
    preserve_markers: bool,
    preserve_escapes: bool,
    render_latex: bool,
    hyperlinks: bool,
    expected: Vec<String>,
}

const FIXTURES: &str = include_str!("../markdown_fixtures.json");

fn build_markdown(c: &FixtureCase) -> Markdown {
    let default_text_style = match c.style.as_str() {
        "grayItalic" => Some(DefaultTextStyle {
            color: Some(style_arc(|t| Chalk(vec![]).dim().paint(t))),
            italic: true,
            ..Default::default()
        }),
        "bold" => Some(DefaultTextStyle {
            bold: true,
            ..Default::default()
        }),
        "unitCount" => Some(DefaultTextStyle {
            color: Some(style_arc(|t| {
                raw_text!("{", t.len().to_string(), ":", t, "}")
            })),
            ..Default::default()
        }),
        "reverseUnits" => Some(DefaultTextStyle {
            color: Some(style_arc(|t| {
                Utf16Text::from_units(t.as_units().iter().copied().rev().collect())
            })),
            ..Default::default()
        }),
        "bgBlue" => Some(DefaultTextStyle {
            bg_color: Some(style_arc(|t| Chalk(vec![]).bg_blue().paint(t))),
            ..Default::default()
        }),
        _ => None,
    };
    let options = MarkdownOptions {
        preserve_ordered_list_markers: c.preserve_markers,
        preserve_backslash_escapes: c.preserve_escapes,
        transform: None,
        render_latex: c.render_latex,
    };
    Markdown::new(
        c.md.clone(),
        c.padding_x,
        c.padding_y,
        default_markdown_theme(),
        default_text_style,
        Some(options),
    )
}

#[test]
fn markdown_18_0_11_semantics_renders_match_upstream() {
    // Supplementary oracle (tests/fixtures/marked-18.0.11-oracle/oracle/gen-extra.mjs):
    // actual upstream markdown.ts under the exact marked 18.0.11 pin, covering
    // the 18.0.5 -> 18.0.11 deltas: nested links, escaped-astral mask, emStrong
    // mid-run, blockquote continuation, list loose/checkbox two-pass, lheading
    // interrupt, tab-only paragraph break, fence EOF interrupt and html
    // end-line swallowing.
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    #[derive(Deserialize)]
    struct ExtraCorpus {
        cases: Vec<FixtureCase>,
    }
    let corpus: ExtraCorpus =
        serde_json::from_str(include_str!("../markdown_18_0_11_fixtures.json")).unwrap();
    assert_eq!(corpus.cases.len(), 31);
    for case in &corpus.cases {
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let lines = build_markdown(case).render(case.width);
        assert_eq!(lines, case.expected, "{}", case.name);
    }
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn markdown_matches_upstream_fixtures_byte_for_byte() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file: FixtureFile = serde_json::from_str(FIXTURES).expect("fixtures parse");
    let mut checked = 0usize;
    for case in &file.cases {
        eprintln!("markdown differential fixture: {}", case.name);
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let mut md = build_markdown(case);
        let lines = md.render(case.width);
        assert_eq!(lines, case.expected, "fixture mismatch: {}", case.name);
        checked += 1;
    }
    crate::tui::terminal_image::reset_capabilities_cache();
    assert_eq!(checked, file.cases.len(), "every fixture must run");
    assert!(checked >= 94, "do not silently shrink the upstream corpus");
}

// ---------------------------------------------------------------------------
// Behavioral tests ported from markdown.test.ts
// ---------------------------------------------------------------------------

fn strip_ansi(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[test]
fn task_list_renders_markers_exactly() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    let mut md = Markdown::new(
        "- [ ] beep\n- [x] boop",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let lines: Vec<String> = md
        .render(80)
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect();
    assert_eq!(lines, vec!["- [ ] beep", "- [x] boop"]);
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn transform_cache_is_keyed_by_source_and_width() {
    let calls: std::sync::Arc<Mutex<Vec<(String, usize)>>> =
        std::sync::Arc::new(Mutex::new(Vec::new()));
    let calls2 = std::sync::Arc::clone(&calls);
    let options = MarkdownOptions {
        transform: Some(Arc::new(move |source: &str, available_width: usize| {
            calls2
                .lock()
                .unwrap()
                .push((source.to_string(), available_width));
            format!("{source} {available_width}")
        })),
        ..Default::default()
    };
    let mut md = Markdown::new(
        "source",
        2,
        0,
        default_markdown_theme(),
        None,
        Some(options),
    );
    let plain = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .map(|l| strip_ansi(l).trim().to_string())
            .collect()
    };
    assert_eq!(plain(&md.render(80)), vec!["source 76"]);
    md.render(80);
    assert_eq!(plain(&md.render(60)), vec!["source 56"]);
    assert_eq!(
        calls.lock().unwrap().clone(),
        vec![("source".to_string(), 76), ("source".to_string(), 56)]
    );
    md.set_text("updated");
    assert_eq!(plain(&md.render(60)), vec!["updated 56"]);
    assert_eq!(
        calls.lock().unwrap().last().unwrap(),
        &("updated".to_string(), 56)
    );
    md.invalidate();
    md.render(60);
    assert_eq!(
        calls.lock().unwrap().last().unwrap(),
        &("updated".to_string(), 56)
    );
    assert_eq!(calls.lock().unwrap().len(), 4);
}

#[test]
fn links_switch_between_osc8_and_fallback_by_capability() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let src = "See [the docs](https://example.com/a) for details.";
    // hyperlinks off: URL printed in parentheses
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    let mut md = Markdown::new(src, 0, 0, default_markdown_theme(), None, None);
    let off = strip_ansi(&md.render(160).join("\n"));
    assert!(
        off.contains("(https://example.com/a)"),
        "fallback url: {off}"
    );
    // hyperlinks on: OSC 8 wrapping, no inline URL
    set_capabilities(TerminalCapabilities {
        hyperlinks: true,
        images: None,
        true_color: true,
    });
    let mut md = Markdown::new(src, 0, 0, default_markdown_theme(), None, None);
    let on = md.render(160).join("\n");
    assert!(
        on.contains("\x1b]8;;https://example.com/a\x1b\\"),
        "osc8: {on}"
    );
    assert!(!strip_ansi(&on).contains("(https://example.com/a)"));
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn heading_spacing_and_prefix_follow_upstream_rules() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    // h3+ keeps the "# " prefix; spacing line after heading before paragraph
    let mut md = Markdown::new(
        "### Deep heading\nText",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let lines: Vec<String> = md
        .render(80)
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect();
    assert_eq!(lines, vec!["### Deep heading", "", "Text"]);
    // heading as last block: no trailing spacing line
    let mut md = Markdown::new("# Only heading", 0, 0, default_markdown_theme(), None, None);
    let lines: Vec<String> = md
        .render(80)
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect();
    assert_eq!(lines, vec!["Only heading"]);
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn escapes_are_normalized_by_default_and_preserved_on_option() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    let src = "Symbols \\* \\_ \\# appear literally.";
    let mut md = Markdown::new(src, 0, 0, default_markdown_theme(), None, None);
    let plain = strip_ansi(&md.render(80).join("\n"));
    assert!(plain.contains("Symbols * _ #"), "normalized: {plain}");
    let options = MarkdownOptions {
        preserve_backslash_escapes: true,
        ..Default::default()
    };
    let mut md = Markdown::new(src, 0, 0, default_markdown_theme(), None, Some(options));
    let plain = strip_ansi(&md.render(80).join("\n"));
    assert!(plain.contains("Symbols \\* \\_ \\#"), "preserved: {plain}");
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn streamed_partial_closing_fence_does_not_shrink_code_block() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    // A partial closing fence line ("``") is trimmed from the code text.
    let mut md = Markdown::new(
        "```js\nvar x = 1;\n``",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let lines: Vec<String> = md
        .render(80)
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect();
    assert_eq!(lines, vec!["```js", "  var x = 1;", "```"]);
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn pending_latex_streams_raw_then_renders_when_closed() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    // Pending inline \( — falls back to raw while streaming.
    let mut md = Markdown::new(
        "Streaming \\(x +",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let plain = strip_ansi(&md.render(80).join("\n"));
    assert!(plain.contains("Streaming \\(x +"), "pending raw: {plain}");
    // Closed unsupported latex also stays raw (renderLatex returns None).
    let mut md = Markdown::new(
        "$\\unknowncmd{x}$ stays raw.",
        0,
        0,
        default_markdown_theme(),
        None,
        None,
    );
    let plain = strip_ansi(&md.render(80).join("\n"));
    assert!(
        plain.contains("\\unknowncmd{x}"),
        "unsupported raw: {plain}"
    );
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn table_narrow_width_falls_back_to_raw_markdown() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_capabilities(TerminalCapabilities {
        hyperlinks: false,
        images: None,
        true_color: true,
    });
    let raw = "| VeryLongHeaderName | AnotherVeryLongHeaderName |\n| --- | --- |\n| a | b |";
    let mut md = Markdown::new(raw, 0, 0, default_markdown_theme(), None, None);
    let lines = md.render(16);
    // Upstream squeezes the table into the narrow width (bordered grid
    // with wrapped cells); byte-exact coverage lives in the
    // table_narrow_fallback fixture. Assert structural invariants here.
    assert!(lines.iter().any(|l| l.contains('│')));
    let plain = strip_ansi(&lines.join("\n"));
    assert!(plain.contains('a') && plain.contains('b'));
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn empty_input_renders_nothing() {
    let mut md = Markdown::new("", 0, 0, default_markdown_theme(), None, None);
    assert!(md.render(40).is_empty());
    let mut md = Markdown::new("   \n  \n", 0, 0, default_markdown_theme(), None, None);
    assert!(md.render(40).is_empty());
}

#[test]
fn chalk_emulator_matches_probe_sequences() {
    // Values from the fixture generator's themeProbe (chalk level 3).
    assert_eq!(
        Chalk(vec![]).bold().cyan().paint("T"),
        "\x1b[1m\x1b[36mT\x1b[39m\x1b[22m"
    );
    assert_eq!(Chalk(vec![]).bold().paint("T"), "\x1b[1mT\x1b[22m");
    assert_eq!(Chalk(vec![]).dim().paint("T"), "\x1b[2mT\x1b[22m");
    assert_eq!(Chalk(vec![]).bg_blue().paint("T"), "\x1b[44mT\x1b[49m");
    assert_eq!(
        Chalk(vec![]).dim().italic().paint("T"),
        "\x1b[2m\x1b[3mT\x1b[23m\x1b[22m"
    );
    // Reopen behavior: bold(underline(x)) wrapped in bold.cyan
    let inner = Chalk(vec![]).bold().underline().paint("Title");
    assert_eq!(
        Chalk(vec![]).bold().cyan().paint(&inner),
        "\x1b[1m\x1b[36m\x1b[1m\x1b[4mTitle\x1b[24m\x1b[22m\x1b[1m\x1b[39m\x1b[22m"
    );
    let _ = chalk_of("unused");
}

/// Final UTF-8 is encoded by Node only after upstream Markdown wrapping/padding.
#[test]
fn markdown_latex_utf16_terminal_output_matches_upstream() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    #[derive(Deserialize)]
    struct Corpus {
        cases: Vec<FixtureCase>,
    }
    let file: Corpus = serde_json::from_str(include_str!("../markdown_utf16_fixtures.json"))
        .expect("raw-unit Markdown corpus parses");
    assert_eq!(file.cases.len(), 2880);
    for case in &file.cases {
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let mut md = build_markdown(case);
        assert_eq!(
            md.render(case.width),
            case.expected,
            "UTF-16 fixture: {}",
            case.name
        );
    }
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn markdown_latex_utf16_raw_lines_widths_and_cache_match_upstream() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    #[derive(Deserialize)]
    struct Corpus {
        cases: Vec<RawCase>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RawCase {
        #[serde(flatten)]
        case: FixtureCase,
        expected_utf16: Vec<Vec<u16>>,
        expected_widths: Vec<usize>,
    }
    let file: Corpus =
        serde_json::from_str(include_str!("../markdown_utf16_fixtures.json")).unwrap();
    assert_eq!(file.cases.len(), 2880);
    for raw in &file.cases {
        let case = &raw.case;
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let mut md = build_markdown(case);
        let lines = md.render_utf16(case.width);
        let units: Vec<_> = lines.iter().map(|line| line.as_units().to_vec()).collect();
        assert_eq!(units, raw.expected_utf16, "raw fixture: {}", case.name);
        let widths: Vec<_> = lines
            .iter()
            .map(|line| crate::tui::utils::visible_width_utf16(line.as_units()))
            .collect();
        assert_eq!(widths, raw.expected_widths, "width fixture: {}", case.name);
        assert_eq!(
            md.render(case.width),
            case.expected,
            "raw then UTF-8 cached: {}",
            case.name
        );
        assert_eq!(
            md.render_utf16(case.width),
            lines,
            "raw cache: {}",
            case.name
        );
        md.invalidate();
        assert_eq!(
            md.render_utf16(case.width),
            lines,
            "after invalidate: {}",
            case.name
        );
    }
    crate::tui::terminal_image::reset_capabilities_cache();
}

#[test]
fn markdown_oracle_artifact_hashes_match_manifest() {
    use sha2::{Digest, Sha256};
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/migration/reference/markdown/source-manifest.json"
    ))
    .unwrap();
    let files: &[(&str, &[u8])] = &[
        ("fixtures.json", include_bytes!("../markdown_fixtures.json")),
        (
            "inline-tail-fixtures.json",
            include_bytes!("../markdown_inline_tail_fixtures.json"),
        ),
        (
            "source-utf16-fixtures.json",
            include_bytes!("../markdown_source_utf16_fixtures.json"),
        ),
        (
            "source-fixtures.json",
            include_bytes!("../markdown_source_fixtures.json"),
        ),
        (
            "utf16-fixtures.json",
            include_bytes!("../markdown_utf16_fixtures.json"),
        ),
        (
            "utf16-wrap-fixtures.json",
            include_bytes!("../utils/utf16/wrap-fixtures.json"),
        ),
    ];
    // The v0.99.1 latex delta (script layout nodes) changed the render of
    // the `x^<astral>` markdown-latex rows, so utf16-fixtures.json was
    // re-captured against upstream HEAD 2bbfcca43 by re-running
    // tests/fixtures/marked-18.0.11-oracle/oracle/gen.mjs with the HEAD
    // markdown.ts/latex.ts/utils.ts (SHAs verified: markdown 30a47f.. is the
    // editor; latex c4ef99be..., utils 5ecbc6c9...). All other artifacts are
    // untouched and still match the baseline manifest.
    let recaptured: &[(&str, &str, u64)] = &[(
        "utf16-fixtures.json",
        "b596493f13a41704fdcd7c1ca51768d4da531d62c3411d7c22746afa354ce877",
        4335846,
    )];
    for (name, data) in files {
        let hash: String = Sha256::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let recaptured_row = recaptured.iter().find(|(key, _, _)| *key == *name);
        match recaptured_row {
            Some((_, sha, len)) => {
                assert_eq!(&hash, sha, "artifact: {name}");
                assert_eq!(data.len() as u64, *len, "bytes: {name}");
            }
            None => {
                assert_eq!(
                    hash,
                    manifest["artifacts"][name]["sha256"].as_str().unwrap(),
                    "artifact: {name}"
                );
                assert_eq!(
                    data.len() as u64,
                    manifest["artifacts"][name]["bytes"].as_u64().unwrap(),
                    "bytes: {name}"
                );
            }
        }
    }
}

#[test]
fn markdown_source_lexer_matches_actual_upstream() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    #[derive(Deserialize)]
    struct SourceCorpus {
        seeds: usize,
        cases: Vec<FixtureCase>,
    }
    let corpus: SourceCorpus =
        serde_json::from_str(include_str!("../markdown_source_fixtures.json")).unwrap();
    assert_eq!(corpus.seeds, 1234);
    assert_eq!(corpus.cases.len(), 3702);
    let mut failures = Vec::new();
    for (i, case) in corpus.cases.iter().enumerate() {
        if i % 100 == 0 {
            eprintln!(
                "source corpus progress: {i}/{} {}",
                corpus.cases.len(),
                case.name
            );
        }
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let actual = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_markdown(case).render(case.width)
        }));
        match actual {
            Ok(lines) if lines == case.expected => {}
            Ok(lines) => {
                eprintln!(
                    "SOURCE MISMATCH {} input={:?}\nactual={lines:?}\nexpected={:?}",
                    case.name, case.md, case.expected
                );
                failures.push(case.name.clone());
            }
            Err(_) => {
                eprintln!("SOURCE PANIC {} input={:?}", case.name, case.md);
                failures.push(case.name.clone());
            }
        }
    }
    crate::tui::terminal_image::reset_capabilities_cache();
    assert!(
        failures.is_empty(),
        "{} source cases failed: {failures:?}",
        failures.len()
    );
}

#[test]
fn markdown_source_utf16_units_styles_layout_and_cache_match_upstream() {
    let _guard = CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Lossless token raw spans reconstruct valid source even when marked's
    // link substring divides a surrogate pair. Display String fields cannot
    // be used to measure consumption at this boundary.
    let source = "[a](  😀)tail)";
    let mut lexer = crate::tui::markdown_lexer::Lexer::new();
    let mut tokens = Vec::new();
    lexer.inline_tokens(
        source,
        &mut tokens,
        &crate::tui::markdown_lexer::NoExtensions,
    );
    assert_eq!(
        tokens[0].raw_utf16.as_ref().unwrap().as_units().last(),
        Some(&0xd83d)
    );
    assert_eq!(
        tokens[1].text_utf16.as_ref().unwrap().as_units().first(),
        Some(&0xde00)
    );
    let raw: Vec<_> = tokens
        .iter()
        .flat_map(|token| {
            token
                .raw_utf16
                .clone()
                .unwrap_or_else(|| Utf16Text::from(&token.raw))
                .into_units()
        })
        .collect();
    assert_eq!(raw, source.encode_utf16().collect::<Vec<_>>());

    #[derive(Deserialize)]
    struct Corpus {
        cases: Vec<RawCase>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RawCase {
        #[serde(flatten)]
        case: FixtureCase,
        expected_utf16: Vec<Vec<u16>>,
        expected_widths: Vec<usize>,
    }
    let file: Corpus =
        serde_json::from_str(include_str!("../markdown_source_utf16_fixtures.json")).unwrap();
    assert_eq!(file.cases.len(), 2688);
    let mut failures = Vec::new();
    for raw in &file.cases {
        let case = &raw.case;
        set_capabilities(TerminalCapabilities {
            hyperlinks: case.hyperlinks,
            images: None,
            true_color: true,
        });
        let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut md = build_markdown(case);
            let lines = md.render_utf16(case.width);
            let units: Vec<_> = lines.iter().map(|line| line.as_units().to_vec()).collect();
            assert_eq!(units, raw.expected_utf16, "raw fixture: {}", case.name);
            let widths: Vec<_> = lines
                .iter()
                .map(|line| crate::tui::utils::visible_width_utf16(line.as_units()))
                .collect();
            assert_eq!(widths, raw.expected_widths, "width fixture: {}", case.name);
            assert_eq!(
                md.render(case.width),
                case.expected,
                "raw then UTF-8 cached: {}",
                case.name
            );
            assert_eq!(
                md.render_utf16(case.width),
                lines,
                "raw cache: {}",
                case.name
            );
            md.invalidate();
            assert_eq!(
                md.render_utf16(case.width),
                lines,
                "after invalidate: {}",
                case.name
            );
        }));
        if check.is_err() {
            eprintln!("RAW SOURCE FAILURE {}", case.name);
            failures.push(case.name.clone());
        }
    }
    crate::tui::terminal_image::reset_capabilities_cache();
    assert!(
        failures.is_empty(),
        "{} raw source failures: {failures:?}",
        failures.len()
    );
}
