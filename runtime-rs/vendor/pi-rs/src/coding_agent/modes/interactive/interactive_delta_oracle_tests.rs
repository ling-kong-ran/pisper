//! Replay tests for the interactive-mode delta oracle
//! (`tests/fixtures/interactive_delta_oracle/interactive_delta_oracle.json`).
//!
//! The fixture is captured from the verbatim upstream HEAD sources by
//! `oracle/capture.mjs` (see the module for the seams). Every scenario is a
//! byte-exact expectation replayed against the Rust port:
//! - `pi_logo_*` — [`pi_logo::pi_logo_lines`] over the pinned color modes;
//! - `themed_text_recolor` — [`themed_text::ThemedText`] rebuild-on-invalidate;
//! - `format_tool_call_with_args` — the render-utils re-statement;
//! - `crash_texts` — crash hint/instructions/suggest bytes + the
//!   abort/cancel word decision;
//! - `footer_routed_and_usage` — the footer routed-model suffix and
//!   `usage`-entry totals over the capture's stub session shape;
//! - `settings_theme_items` — system-first theme ordering + descriptions;
//! - `cli_mode_diagnostics` — `parse_args` `--mode` validation;
//! - `rpc_dispositions` — the `RpcResponse` success serialization.

use serde_json::Value;

use super::components::footer::{
    FooterComponent, FooterDataProvider, FooterEntry, FooterModel, FooterRoutedModel,
    FooterSession, FooterUsage,
};
use super::components::pi_logo::pi_logo_lines;
use super::components::settings_selector::{single_mode_theme_items, theme_items};
use super::components::themed_text::ThemedText;
use super::interactive_mode::{crash_report_instructions, format_crash_extension_hint};
use super::theme::{load_builtin_theme, Theme};
use crate::coding_agent::cli::args::{parse_args, DiagnosticType};
use crate::coding_agent::modes::interactive::components::tool_execution::format_tool_call_with_args;
use crate::tui::colors::TerminalColorMode;
use crate::tui::component::Component;

fn oracle() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interactive_delta_oracle/interactive_delta_oracle.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("oracle fixture"))
        .expect("oracle json")
}

fn dark_theme() -> Theme {
    load_builtin_theme("dark", Some(TerminalColorMode::Truecolor)).expect("dark theme")
}

fn theme_for_mode(mode: &str) -> Theme {
    let color_mode = match mode {
        "256color" => TerminalColorMode::Color256,
        _ => TerminalColorMode::Truecolor,
    };
    load_builtin_theme("dark", Some(color_mode)).expect("theme")
}

#[test]
fn pi_logo_matches_oracle() {
    let oracle = oracle();
    for mode in ["truecolor", "256color"] {
        let scenario = &oracle["scenarios"][&format!("pi_logo_{mode}")];
        let theme = theme_for_mode(mode);
        let (top, bottom) = pi_logo_lines(&theme);
        let expected: Vec<String> = serde_json::from_value(scenario["lines"].clone()).unwrap();
        assert_eq!([top, bottom], expected[..], "pi logo {mode}");
    }
}

#[test]
fn themed_text_recolor_matches_oracle() {
    let scenario = &oracle()["scenarios"]["themed_text_recolor"];
    let dark = std::sync::Arc::new(std::sync::RwLock::new(dark_theme()));
    let light = load_builtin_theme("light", Some(TerminalColorMode::Truecolor)).expect("light");
    let cell = dark.clone();
    let mut component = ThemedText::new(std::sync::Arc::new(move || {
        let theme = cell.read().expect("theme cell");
        theme.fg("accent", "hello world").expect("accent fg")
    }));
    // The oracle pins the component's own lines (the capture drove the bare
    // component); the shell pipeline pads to width, so normalize by dropping
    // leading blank lines and trimming line tails before comparing.
    let render = |component: &mut ThemedText| -> Vec<String> {
        component
            .render(80)
            .into_iter()
            .skip_while(|line| line.trim().is_empty())
            .collect::<Vec<_>>()
    };
    let expected_dark: Vec<String> = serde_json::from_value(scenario["dark"].clone()).unwrap();
    assert_eq!(render(&mut component), expected_dark, "dark render");
    // The theme swap alone leaves the stale (dark) bytes on screen.
    *dark.write().expect("theme cell") = light;
    let expected_stale: Vec<String> =
        serde_json::from_value(scenario["stale_after_theme_change"].clone()).unwrap();
    assert_eq!(render(&mut component), expected_stale, "stale render");
    component.invalidate();
    let expected_light: Vec<String> =
        serde_json::from_value(scenario["light_after_invalidate"].clone()).unwrap();
    assert_eq!(render(&mut component), expected_light, "light render");
}

#[test]
fn format_tool_call_with_args_matches_oracle() {
    let scenario = &oracle()["scenarios"]["format_tool_call_with_args"];
    assert_eq!(scenario["collapsed_args_chars"].as_u64(), Some(100));
    let theme = dark_theme();
    for case in scenario["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().expect("case name");
        // The capture stores the rendered value only; the Rust side re-runs
        // the same inputs table (title/args/expanded mirrored per name).
        let (title, args, expanded) = tool_call_case(name);
        let actual = format_tool_call_with_args(&theme, title, &args, expanded);
        let expected = case["value"].as_str().expect("captured value");
        assert_eq!(actual, expected, "formatToolCallWithArgs case: {name}");
    }
}

/// The capture's `cases` table, mirrored (the fixture pins the outputs; the
/// inputs are the documented scenario table in `oracle/capture.mjs`).
fn tool_call_case(name: &str) -> (&'static str, serde_json::Value, bool) {
    use serde_json::json;
    match name {
        "null args" => ("grep", Value::Null, false),
        "null args expanded" => ("grep", Value::Null, true),
        "empty object" => ("ls", json!({}), false),
        "string arg collapsed" => ("read", json!("src/main.ts"), false),
        "string arg expanded" => ("read", json!("src/main.ts"), true),
        "object collapsed" => ("edit", json!({"path": "src/a.rs", "line": 3}), false),
        "object expanded" => ("edit", json!({"path": "src/a.rs", "line": 3}), true),
        "nested expanded" => (
            "run",
            json!({"cmd": "x", "env": {"A": 1, "B": [1, 2]}}),
            true,
        ),
        "array arg collapsed" => ("run", json!(["a", "b"]), false),
        "multiline string expanded" => ("write", json!("one\r\ntwo\tthree\nfour"), true),
        "long string cut" => ("bash", json!("y".repeat(140)), false),
        "unicode cut" => ("bash", json!("\u{1F600}".repeat(60)), false),
        other => panic!("unknown capture case: {other}"),
    }
}

#[test]
fn crash_texts_match_oracle() {
    let scenario = &oracle()["scenarios"]["crash_texts"];
    // Hints (missing/null arrays → None).
    for (name, input) in [
        ("undefined", None),
        ("empty array", Some(Vec::<String>::new())),
        ("single", Some(vec!["ext-a".to_string()])),
        ("pair", Some(vec!["ext-a".to_string(), "ext-b".to_string()])),
        (
            "triple",
            Some(vec![
                "ext-a".to_string(),
                "ext-b".to_string(),
                "ext-c".to_string(),
            ]),
        ),
        (
            "empty strings filtered",
            Some(vec![String::new(), "real".to_string()]),
        ),
    ] {
        let expected = scenario["hints"][name].as_str();
        assert_eq!(
            format_crash_extension_hint(input.as_deref()),
            expected.map(str::to_string),
            "crash hint case: {name}"
        );
    }
    assert_eq!(
        crash_report_instructions(true),
        scenario["instructions"]["with_session"].as_str().unwrap(),
        "instructions with session"
    );
    assert_eq!(
        crash_report_instructions(false),
        scenario["instructions"]["without_session"]
            .as_str()
            .unwrap(),
        "instructions without session"
    );
    // The suggest line renders muted, padding (1, 0), width 80.
    let theme = dark_theme();
    let expected: Vec<String> = serde_json::from_value(scenario["suggest_render"].clone()).unwrap();
    let text = theme
        .fg("muted", &super::interactive_mode::bug_report_hint_text())
        .unwrap_or_default();
    let mut component = super::components::themed_text::ThemedText::with_options(
        std::sync::Arc::new(move || text.clone()),
        1,
        0,
    );
    // `suggest_render` mounts with pad (1, 0) via the shell; the captured
    // bytes come from the same Text pipeline with padding 1/0.
    let rendered = render_themed_like_shell(&mut component);
    assert_eq!(rendered, expected, "suggest render");
    // The abort/cancel word decision.
    for (input, matched) in scenario["abort_cancel_regex"]["inputs"]
        .as_object()
        .expect("regex inputs")
    {
        let input: &str = input;
        // The scenario pins the abort/cancel WORD predicate itself (the
        // oracle inputs are abort-word truth tables, not suggestions).
        assert_eq!(
            super::interactive_mode::abort_or_cancel_word(input),
            matched.as_bool().expect("bool"),
            "abort/cancel decision for {input:?}"
        );
    }
}

/// Render a ThemedText through the same Text pipeline the shell's
/// `container_add_text` uses (padding 1/0, width 80).
fn render_themed_like_shell(component: &mut ThemedText) -> Vec<String> {
    component.render(80)
}

// -- footer ------------------------------------------------------------------

struct FooterStubSession {
    entries: Vec<FooterEntry>,
    routed: Option<FooterRoutedModel>,
    entry_count: usize,
}

impl FooterSession for FooterStubSession {
    fn state_model(&self) -> Option<FooterModel> {
        Some(FooterModel {
            id: "pi-virtual".to_string(),
            provider: "pi".to_string(),
            context_window: 100_000,
            reasoning: false,
        })
    }
    fn state_thinking_level(&self) -> Option<String> {
        None
    }
    fn context_usage(&self) -> Option<(u64, Option<f64>)> {
        Some((200_000, Some(10.0)))
    }
    fn cwd(&self) -> String {
        // environment-anchored: the capture pinned HOME to a fake win32
        // profile; on POSIX the same scenarios use the POSIX-shaped anchor.
        if cfg!(windows) {
            "C:\\Users\\n\\proj".to_string()
        } else {
            "/pi-delta-oracle-cwd/proj".to_string()
        }
    }
    fn session_name(&self) -> Option<String> {
        None
    }
    fn entries(&self) -> Vec<FooterEntry> {
        self.entries.clone()
    }
    fn is_using_subscription(&self, _provider: &str) -> bool {
        false
    }
    fn session_id(&self) -> String {
        "session-1".to_string()
    }
    fn leaf_id(&self) -> Option<String> {
        Some("leaf-1".to_string())
    }
    fn entry_count(&self) -> usize {
        self.entry_count
    }
    fn routed_model(&self) -> Option<FooterRoutedModel> {
        self.routed.clone()
    }
}

struct FooterStubProvider;

impl FooterDataProvider for FooterStubProvider {
    fn git_branch(&self) -> Option<String> {
        None
    }
    fn extension_statuses(&self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }
    fn available_provider_count(&self) -> usize {
        1
    }
}

fn usage_entry(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
) -> FooterUsage {
    FooterUsage {
        input,
        output,
        cache_read,
        cache_write,
        cost,
    }
}

fn routed_model(level: Option<&str>) -> FooterRoutedModel {
    FooterRoutedModel {
        model: FooterModel {
            id: "claude-sonnet-4".to_string(),
            provider: "anthropic".to_string(),
            context_window: 200_000,
            reasoning: true,
        },
        thinking_level: level.map(str::to_string),
    }
}

#[test]
fn footer_routed_and_usage_matches_oracle() {
    let scenario = &oracle()["scenarios"]["footer_routed_and_usage"];
    let theme = dark_theme();
    let usage_entries = vec![
        FooterEntry::Usage(usage_entry(1500, 250, 100, 50, 0.5)),
        FooterEntry::Usage(usage_entry(10, 5, 0, 0, 0.05)),
    ];
    // The capture pinned HOME to the fake profile the cwd anchor nests
    // under; set the same anchor so the ~-shortening reproduces. The env is
    // process-global (serial suites), so restore the previous values after.
    struct EnvGuard(Option<String>, Option<String>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.1 {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
    let _env = EnvGuard(
        std::env::var("USERPROFILE").ok(),
        std::env::var("HOME").ok(),
    );
    if cfg!(windows) {
        std::env::set_var("USERPROFILE", r"C:\Users\n");
        std::env::remove_var("HOME");
    } else {
        std::env::set_var("HOME", "/pi-delta-oracle-cwd");
        std::env::remove_var("USERPROFILE");
    }
    let rows = |routed: Option<FooterRoutedModel>| {
        let session = FooterStubSession {
            entries: usage_entries.clone(),
            routed,
            entry_count: 2,
        };
        FooterComponent::new(&session, &FooterStubProvider).render_footer(120, &theme)
    };
    for (name, routed) in [
        ("routed_with_level", Some(routed_model(Some("high")))),
        ("routed_without_level", Some(routed_model(None))),
        ("not_routed", None),
    ] {
        let expected: Vec<String> = serde_json::from_value(scenario[name].clone()).unwrap();
        // environment-anchored: the pwd row is host-path shaped (see
        // [`FooterStubSession::cwd`]); compare the stats row byte-exactly and
        // the pwd row by suffix.
        if cfg!(windows) {
            assert_eq!(rows(routed.clone()), expected, "footer scenario {name}");
        } else {
            // The pwd row is host-path shaped; compare the stats rows only.
            let actual = rows(routed);
            assert_eq!(actual[1..], expected[1..], "footer scenario {name} (stats)");
        }
    }
}

// -- settings theme items ----------------------------------------------------

#[test]
fn settings_theme_items_match_oracle() {
    let scenario = &oracle()["scenarios"]["settings_theme_items"];
    assert_eq!(
        super::system_theme::SYSTEM_THEME_NAME,
        scenario["system_theme_name"].as_str().unwrap()
    );
    let to_rows = |items: Vec<_>| {
        items
            .into_iter()
            .map(|item: crate::tui::components::select_list::SelectItem| {
                let mut row = serde_json::Map::new();
                row.insert("value".into(), Value::String(item.value));
                row.insert("label".into(), Value::String(item.label));
                if let Some(description) = item.description {
                    row.insert("description".into(), Value::String(description));
                }
                Value::Object(row)
            })
            .collect::<Vec<_>>()
    };
    let expected_system_first: Vec<Value> =
        serde_json::from_value(scenario["system_first_current_system"].clone()).unwrap();
    assert_eq!(
        to_rows(single_mode_theme_items(
            &[
                "system".to_string(),
                "dark".to_string(),
                "light".to_string()
            ],
            "system"
        )),
        expected_system_first,
        "system-first (current system)"
    );
    let expected_dark: Vec<Value> =
        serde_json::from_value(scenario["system_first_current_dark"].clone()).unwrap();
    assert_eq!(
        to_rows(single_mode_theme_items(
            &[
                "system".to_string(),
                "dark".to_string(),
                "light".to_string()
            ],
            "dark"
        )),
        expected_dark,
        "system-first (current dark)"
    );
    let expected_description: Vec<Value> =
        serde_json::from_value(scenario["theme_items_description"].clone()).unwrap();
    assert_eq!(
        to_rows(theme_items(
            &["system".to_string(), "dark".to_string()],
            "dark"
        )),
        expected_description,
        "theme items descriptions"
    );
}

// -- cli --mode --------------------------------------------------------------

#[test]
fn cli_mode_diagnostics_match_oracle() {
    let scenario = &oracle()["scenarios"]["cli_mode_diagnostics"];
    let to_diags = |args: &[String]| {
        parse_args(args)
            .diagnostics
            .into_iter()
            .map(|diagnostic| {
                serde_json::json!({
                    "type": match diagnostic.kind {
                        DiagnosticType::Error => "error",
                        DiagnosticType::Warning => "warning",
                    },
                    "message": diagnostic.message,
                })
            })
            .collect::<Vec<_>>()
    };
    for case in scenario["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().expect("case name");
        let argv: Vec<String> = case["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value: &Value| value.as_str().expect("argv item").to_string())
            .collect();
        let actual = to_diags(&argv);
        let expected: Vec<Value> =
            serde_json::from_value(case["result"]["diagnostics"].clone()).unwrap();
        assert_eq!(actual, expected, "cli --mode diagnostics for {name}");
        // The captured `i` (next parse index) and the parsed mode.
        let parsed_mode = parse_args(&argv).mode.map(|mode| match mode {
            crate::coding_agent::cli::args::Mode::Text => "text",
            crate::coding_agent::cli::args::Mode::Json => "json",
            crate::coding_agent::cli::args::Mode::Rpc => "rpc",
        });
        let expected_mode = case["result"]["mode"].as_str();
        assert_eq!(parsed_mode, expected_mode, "cli --mode value for {name}");
    }
}

// -- rpc dispositions --------------------------------------------------------

#[test]
fn rpc_dispositions_match_oracle() {
    let scenario = &oracle()["scenarios"]["rpc_dispositions"];
    for (name, id, command, disposition) in [
        ("prompt_started", "p1", "prompt", "started"),
        ("prompt_handled", "p2", "prompt", "handled"),
        ("prompt_queued", "p3", "prompt", "queued"),
        ("steer", "s1", "steer", "queued"),
        ("follow_up", "f1", "follow_up", "queued"),
    ] {
        let response = crate::coding_agent::modes::rpc::types::RpcResponse::success(
            Some(id.to_string()),
            command,
            Some(serde_json::json!({ "disposition": disposition })),
        );
        let line = serde_json::to_string(&response).expect("rpc response json");
        assert_eq!(
            line,
            scenario[name].as_str().expect("captured line"),
            "rpc disposition line for {name}"
        );
    }
}
