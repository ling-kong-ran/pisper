//! Tests for the ported `coding-agent/src/cli/args.ts`.
//!
//! Source of truth: upstream `test/args.test.ts` plus the node-captured
//! oracle (`tests/fixtures/cli_oracle/oracle.json`, 92 `parseArgs` batteries, help
//! text, `normalizeSessionName`) captured from the real upstream module under
//! `node --experimental-strip-types`. Comparisons are byte-exact.

use serde_json::{json, Value};

use crate::coding_agent::cli::args::*;

const ORACLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/cli_oracle/oracle.json"
));

fn oracle() -> &'static Value {
    use std::sync::OnceLock;
    static ORACLE_VALUE: OnceLock<Value> = OnceLock::new();
    ORACLE_VALUE.get_or_init(|| serde_json::from_str(ORACLE).expect("oracle json"))
}

fn argv_from_key(key: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(key).expect("oracle keys are JSON string arrays")
}

fn opt_str(value: Option<&String>) -> Value {
    value.map(|value| json!(value)).unwrap_or(Value::Null)
}

fn opt_list(value: Option<&Vec<String>>) -> Value {
    match value {
        Some(values) => json!(values),
        None => Value::Null,
    }
}

fn opt_bool(value: Option<bool>) -> Value {
    value.map(|value| json!(value)).unwrap_or(Value::Null)
}

/// Mirror of the oracle's `pick()` projection of a parsed `Args`.
fn args_to_json(args: &Args) -> Value {
    fn insert(map: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
        map.insert(key.to_string(), value);
    }
    let mut map = serde_json::Map::new();
    insert(&mut map, "provider", opt_str(args.provider.as_ref()));
    insert(&mut map, "model", opt_str(args.model.as_ref()));
    insert(&mut map, "apiKey", opt_str(args.api_key.as_ref()));
    insert(
        &mut map,
        "systemPrompt",
        opt_str(args.system_prompt.as_ref()),
    );
    insert(
        &mut map,
        "appendSystemPrompt",
        opt_list(args.append_system_prompt.as_ref()),
    );
    insert(
        &mut map,
        "thinking",
        args.thinking
            .map(|level| {
                json!(match level {
                    ModelThinkingLevel::Off => "off",
                    ModelThinkingLevel::Minimal => "minimal",
                    ModelThinkingLevel::Low => "low",
                    ModelThinkingLevel::Medium => "medium",
                    ModelThinkingLevel::High => "high",
                    ModelThinkingLevel::Xhigh => "xhigh",
                    ModelThinkingLevel::Max => "max",
                })
            })
            .unwrap_or(Value::Null),
    );
    insert(&mut map, "continue_", opt_bool(args.r#continue));
    insert(&mut map, "resume", opt_bool(args.resume));
    insert(&mut map, "help", opt_bool(args.help));
    insert(&mut map, "version", opt_bool(args.version));
    insert(
        &mut map,
        "mode",
        args.mode
            .map(|mode| {
                json!(match mode {
                    Mode::Text => "text",
                    Mode::Json => "json",
                    Mode::Rpc => "rpc",
                })
            })
            .unwrap_or(Value::Null),
    );
    insert(&mut map, "name", opt_str(args.name.as_ref()));
    insert(&mut map, "noSession", opt_bool(args.no_session));
    insert(&mut map, "session", opt_str(args.session.as_ref()));
    insert(&mut map, "sessionId", opt_str(args.session_id.as_ref()));
    insert(&mut map, "fork", opt_str(args.fork.as_ref()));
    insert(&mut map, "sessionDir", opt_str(args.session_dir.as_ref()));
    insert(&mut map, "models", opt_list(args.models.as_ref()));
    insert(&mut map, "tools", opt_list(args.tools.as_ref()));
    insert(
        &mut map,
        "excludeTools",
        opt_list(args.exclude_tools.as_ref()),
    );
    insert(&mut map, "noTools", opt_bool(args.no_tools));
    insert(&mut map, "noBuiltinTools", opt_bool(args.no_builtin_tools));
    insert(&mut map, "extensions", opt_list(args.extensions.as_ref()));
    insert(&mut map, "noExtensions", opt_bool(args.no_extensions));
    insert(&mut map, "print", opt_bool(args.print));
    insert(&mut map, "export_", opt_str(args.export.as_ref()));
    insert(&mut map, "noSkills", opt_bool(args.no_skills));
    insert(&mut map, "skills", opt_list(args.skills.as_ref()));
    insert(
        &mut map,
        "promptTemplates",
        opt_list(args.prompt_templates.as_ref()),
    );
    insert(
        &mut map,
        "noPromptTemplates",
        opt_bool(args.no_prompt_templates),
    );
    insert(&mut map, "themes", opt_list(args.themes.as_ref()));
    insert(&mut map, "useTheme", opt_str(args.use_theme.as_ref()));
    insert(&mut map, "noThemes", opt_bool(args.no_themes));
    insert(&mut map, "noContextFiles", opt_bool(args.no_context_files));
    insert(
        &mut map,
        "listModels",
        match &args.list_models {
            Some(Some(pattern)) => json!(pattern),
            Some(None) => json!(true),
            None => Value::Null,
        },
    );
    insert(&mut map, "offline", opt_bool(args.offline));
    insert(
        &mut map,
        "tuiMode",
        args.tui_mode
            .map(|mode| {
                json!(match mode {
                    TuiMode::Regular => "regular",
                    TuiMode::Fullscreen => "fullscreen",
                })
            })
            .unwrap_or(Value::Null),
    );
    insert(&mut map, "verbose", opt_bool(args.verbose));
    insert(
        &mut map,
        "projectTrustOverride",
        opt_bool(args.project_trust_override),
    );
    insert(&mut map, "messages", json!(args.messages));
    insert(&mut map, "fileArgs", json!(args.file_args));
    insert(
        &mut map,
        "unknownFlags",
        Value::Array(
            args.unknown_flags
                .iter()
                .map(|(name, value)| match value {
                    UnknownFlagValue::Boolean(flag) => json!([name, *flag]),
                    UnknownFlagValue::Text(text) => json!([name, text]),
                })
                .collect(),
        ),
    );
    insert(
        &mut map,
        "diagnostics",
        Value::Array(
            args.diagnostics
                .iter()
                .map(|diagnostic| {
                    json!({
                        "type": match diagnostic.kind {
                            DiagnosticType::Warning => "warning",
                            DiagnosticType::Error => "error",
                        },
                        "message": diagnostic.message,
                    })
                })
                .collect(),
        ),
    );
    Value::Object(map)
}

#[test]
fn parse_args_matches_the_node_oracle_byte_for_byte() {
    let oracle = oracle();
    let cases = oracle["args"].as_object().expect("args object");
    for (key, expected) in cases {
        let argv = argv_from_key(key);
        let actual = args_to_json(&parse_args(&argv));
        assert_eq!(actual, *expected, "parseArgs mismatch for argv {key}");
    }
}

#[test]
fn normalize_session_name_matches_the_oracle() {
    let expected = oracle()["normalizeSessionName"].as_array().unwrap();
    assert_eq!(
        normalize_session_name("  named session  "),
        Some("named session".to_string())
    );
    assert_eq!(normalize_session_name("   "), None);
    assert_eq!(expected.len(), 2);
}

/// Upstream args.test.ts: "parses prompt after -p even when it starts with
/// YAML frontmatter".
#[test]
fn frontmatter_prompt_following_print_is_a_message() {
    let prompt = "---\ntitle: hello\n---\nSay hi.".to_string();
    let result = parse_args(&["-p".to_string(), prompt.clone()]);
    assert_eq!(result.print, Some(true));
    assert_eq!(result.messages, vec![prompt]);
    assert!(result.unknown_flags.is_empty());
}

/// Upstream args.test.ts: "does not consume options after -p as prompts".
#[test]
fn options_after_print_are_not_prompts() {
    let result = parse_args(&[
        "-p".to_string(),
        "--provider".to_string(),
        "openai".to_string(),
        "Say hi.".to_string(),
    ]);
    assert_eq!(result.print, Some(true));
    assert_eq!(result.provider.as_deref(), Some("openai"));
    assert_eq!(result.messages, vec!["Say hi.".to_string()]);
}

/// Upstream args.test.ts: "does not recognize the old --ui-mode flag".
#[test]
fn old_ui_mode_flag_is_unknown() {
    let result = parse_args(&["--ui-mode".to_string(), "fullscreen".to_string()]);
    assert_eq!(result.tui_mode, None);
    assert_eq!(
        result.unknown_flags.get("ui-mode"),
        Some(&UnknownFlagValue::Text("fullscreen".to_string()))
    );
}

/// Upstream args.test.ts: help text (byte-compared against the oracle
/// `help.main`, which `print_help(&[])` must reproduce exactly).
#[test]
fn print_help_matches_the_node_oracle_byte_for_byte() {
    assert_eq!(
        format!(
            "{}
",
            print_help(&[])
        ),
        oracle()["help"]["main"].as_str().unwrap()
    );
}

/// Upstream args.test.ts: extension flags section (padEnd + descriptions).
#[test]
fn print_help_appends_extension_flags() {
    use crate::coding_agent::extensions::types::{ExtensionFlag, FlagType};
    let flags = vec![ExtensionFlag {
        name: "plan".to_string(),
        description: None,
        flag_type: FlagType::String,
        default: None,
        extension_path: "/ext/plan.ts".to_string(),
    }];
    let help = print_help(&flags);
    let expected_tail =
        "\nExtension CLI Flags:\n  --plan <value>              Registered by /ext/plan.ts\n";
    assert!(
        help.contains(expected_tail),
        "help tail was: {}",
        &help[help.len() - 160..]
    );
}
