//! Tests for the ported `coding-agent/src/core/settings-manager.ts`.
//!
//! Sources of truth:
//! - upstream `test/settings-manager.test.ts`,
//!   `test/settings-manager-compaction.test.ts` (the #8133 regression
//!   coverage), and `test/settings-manager-bug.test.ts` (external-edit
//!   preservation) — all reachable scenarios are ported,
//! - oracle captures of the real upstream module
//!   (`tests/fixtures/core_oracle_w37/settings_manager.oracle.json`): rewritten
//!   settings.json bytes (migration key order, modified-field merge order,
//!   nested-field persistence, undefined-drops-key), the full validation
//!   error text battery, getter batteries, and drainErrors structure.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{
    js_number_to_string_for_tests, js_to_string_for_tests, parse_settings_value,
    settings_value_to_serde_for_tests, stringify_pretty_for_tests, DefaultProjectTrust,
    FullscreenExitOutput, FullscreenScrollbar, InMemorySettingsStorage, MermaidRenderingMode,
    QuietStartup, SettingsManager, SettingsScope, SettingsStorage, SettingsValue, TreeFilterMode,
    TuiMode,
};
use crate::ai::types::primitives::ThinkingLevel;
use crate::coding_agent::core::http_dispatcher::DEFAULT_HTTP_IDLE_TIMEOUT_MS;

fn oracle() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/core_oracle_w37/settings_manager.oracle.json"
    ))
    .unwrap()
}

/// Serializes the env-mutating tests (`VISUAL`/`EDITOR`/`PI_*`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    agent_dir: PathBuf,
    project_dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pi-settings-manager-w37-{tag}-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let agent_dir = root.join("agent");
        let project_dir = root.join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(project_dir.join(".pi")).unwrap();
        Self {
            root,
            agent_dir,
            project_dir,
        }
    }

    fn global_path(&self) -> PathBuf {
        self.agent_dir.join("settings.json")
    }

    fn project_path(&self) -> PathBuf {
        self.project_dir.join(".pi").join("settings.json")
    }

    fn write_global(&self, content: &str) {
        std::fs::write(self.global_path(), content).unwrap();
    }

    fn write_project(&self, content: &str) {
        std::fs::write(self.project_path(), content).unwrap();
    }

    fn global_bytes(&self) -> String {
        std::fs::read_to_string(self.global_path()).unwrap()
    }

    fn project_bytes(&self) -> String {
        std::fs::read_to_string(self.project_path()).unwrap()
    }

    fn manager(&self) -> SettingsManager {
        SettingsManager::create_with(
            self.project_dir.to_str().unwrap(),
            self.agent_dir.to_str().unwrap(),
            Default::default(),
        )
        .unwrap()
    }

    fn manager_untrusted(&self) -> SettingsManager {
        SettingsManager::create_with(
            self.project_dir.to_str().unwrap(),
            self.agent_dir.to_str().unwrap(),
            super::SettingsManagerCreateOptions {
                project_trusted: false,
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn obj(pairs: &[(&str, SettingsValue)]) -> SettingsValue {
    SettingsValue::Obj(
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
    )
}

fn arr(items: &[&str]) -> SettingsValue {
    SettingsValue::Arr(items.iter().map(|item| SettingsValue::str(item)).collect())
}

// ---------------------------------------------------------------------------
// Oracle: migration + write bytes
// ---------------------------------------------------------------------------

#[test]
fn migration_and_write_bytes_match_the_oracle() {
    let capture = oracle();
    let fixture = Fixture::new("migrate");

    // Legacy keys migrate on load; the first save rewrites the migrated
    // document (delete removes keys, new keys append).
    fixture.write_global(
        r#"{"queueMode":"all","websockets":true,"skills":{"enableSkillCommands":false,"customDirectories":["/a","/b"]},"retry":{"maxDelayMs":45000,"enabled":true},"theme":"dark"}"#,
    );
    let manager = fixture.manager();
    let values = &capture["values"];
    assert_eq!(
        manager.get_steering_mode(),
        values["migrated_steering"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_transport(),
        values["migrated_transport"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_skill_paths(),
        serde_json::from_value::<Vec<String>>(values["migrated_skills"].clone()).unwrap()
    );
    assert!(!manager.get_enable_skill_commands());
    let provider_retry = manager.get_provider_retry_settings();
    assert_eq!(provider_retry.max_retry_delay_ms, 45000);
    manager.set_default_thinking_level(ThinkingLevel::High);
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["migrated_plus_write"].as_str().unwrap(),
        "migrated_plus_write bytes"
    );
}

#[test]
fn preserves_externally_added_settings_bytes() {
    let capture = oracle();
    let fixture = Fixture::new("preserve");
    let values = &capture["values"];
    let _ = values;

    fixture.write_global(r#"{"theme":"dark","defaultModel":"claude-sonnet"}"#);
    let manager = fixture.manager();

    // External edit adds enabledModels (insertion order, like JSON.stringify
    // of a JS object).
    let mut current =
        parse_settings_value(r#"{"theme":"dark","defaultModel":"claude-sonnet"}"#).unwrap();
    current.set("enabledModels", arr(&["claude-opus-4-5", "gpt-5.2-codex"]));
    fixture.write_global(&stringify_pretty_for_tests(&current));

    manager.set_default_thinking_level(ThinkingLevel::High);
    manager.flush();

    let saved: serde_json::Value = serde_json::from_str(&fixture.global_bytes()).unwrap();
    assert_eq!(
        saved["enabledModels"],
        serde_json::json!(["claude-opus-4-5", "gpt-5.2-codex"])
    );
    assert_eq!(saved["defaultThinkingLevel"], serde_json::json!("high"));
    assert_eq!(saved["theme"], serde_json::json!("dark"));
    assert_eq!(saved["defaultModel"], serde_json::json!("claude-sonnet"));
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["preserve_enabled_models"]
            .as_str()
            .unwrap(),
        "preserve_enabled_models bytes"
    );
}

#[test]
fn in_memory_changes_override_file_changes_for_the_same_key_bytes() {
    let capture = oracle();
    let fixture = Fixture::new("same-key");

    fixture.write_global(r#"{"theme":"dark"}"#);
    let manager = fixture.manager();

    // External edit sets the thinking level to "low" (insertion order).
    let mut current = parse_settings_value(r#"{"theme":"dark"}"#).unwrap();
    current.set("defaultThinkingLevel", SettingsValue::str("low"));
    fixture.write_global(&stringify_pretty_for_tests(&current));

    // …but the in-memory change to "high" wins.
    manager.set_default_thinking_level(ThinkingLevel::High);
    manager.flush();

    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["in_memory_wins_same_key"]
            .as_str()
            .unwrap(),
        "in_memory_wins_same_key bytes"
    );
}

#[test]
fn packages_migration_and_filtering_objects() {
    let capture = oracle();
    let fixture = Fixture::new("packages");
    let values = &capture["values"];

    fixture.write_global(
        r#"{"packages":["npm:simple-pkg",{"source":"npm:shitty-extensions","extensions":["extensions/oracle.ts"],"skills":[]}],"extensions":["/local/ext.ts","./relative/ext.ts"]}"#,
    );
    let manager = fixture.manager();
    let packages = manager.get_packages();
    assert_eq!(
        serde_json::to_value(packages).unwrap(),
        values["packages"],
        "packages"
    );
    assert_eq!(
        manager.get_extension_paths(),
        serde_json::from_value::<Vec<String>>(values["extension_paths"].clone()).unwrap()
    );

    // Local-only extensions stay in the extensions array (empty packages).
    fixture.write_global(r#"{"extensions":["/local/ext.ts","./relative/ext.ts"]}"#);
    let manager = fixture.manager();
    assert!(manager.get_packages().is_empty());
    assert_eq!(
        manager.get_extension_paths(),
        vec!["/local/ext.ts".to_string(), "./relative/ext.ts".to_string()]
    );
}

#[test]
fn nested_modified_field_persistence_bytes() {
    let capture = oracle();
    let fixture = Fixture::new("nested");

    fixture.write_global(
        r#"{"compaction":{"enabled":true,"reserveTokens":1234,"modelOverrides":{"p/m":{"reserveTokens":1}}},"theme":"dark"}"#,
    );
    let manager = fixture.manager();
    manager.set_compaction_enabled(false);
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["nested_modified_persist"]
            .as_str()
            .unwrap(),
        "nested_modified_persist bytes"
    );
}

#[test]
fn unset_via_undefined_drops_the_key_bytes() {
    let capture = oracle();
    let fixture = Fixture::new("unset");

    fixture.write_global(r#"{"shellPath":"/bin/zsh","theme":"dark"}"#);
    let manager = fixture.manager();
    manager.set_shell_path(None);
    manager.set_default_thinking_level(ThinkingLevel::Low);
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["undefined_drops_key"].as_str().unwrap(),
        "undefined_drops_key bytes"
    );
}

// ---------------------------------------------------------------------------
// reload + error tracking
// ---------------------------------------------------------------------------

#[test]
fn reload_picks_up_global_changes_from_disk() {
    let capture = oracle();
    let values = &capture["values"];
    let fixture = Fixture::new("reload");

    fixture.write_global(r#"{"theme":"dark","extensions":["/before.ts"]}"#);
    let manager = fixture.manager();
    fixture.write_global(
        r#"{"theme":"light","extensions":["/after.ts"],"defaultModel":"claude-sonnet"}"#,
    );
    manager.reload();

    assert_eq!(manager.get_theme(), Some("light".to_string()));
    assert_eq!(manager.get_extension_paths(), vec!["/after.ts".to_string()]);
    assert_eq!(
        manager.get_default_model(),
        Some("claude-sonnet".to_string())
    );
    assert_eq!(
        manager.get_theme().as_deref(),
        Some(values["reload_theme"].as_str().unwrap())
    );

    // Invalid JSON keeps the previous settings and reports scope + path.
    fixture.write_global("{ invalid json");
    manager.reload();
    assert_eq!(
        manager.get_theme(),
        Some("light".to_string()),
        "the previous settings survive a parse failure"
    );
    let errors = manager.drain_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].scope, SettingsScope::Global);
    assert_eq!(
        errors[0].path.as_deref(),
        Some(fixture.global_path().to_str().unwrap())
    );
}

#[test]
fn error_tracking_collects_and_clears_load_errors() {
    let capture = oracle();
    let fixture = Fixture::new("errors");

    fixture.write_global("{ invalid global json");
    fixture.write_project("{ invalid project json");
    let manager = fixture.manager();
    let errors = manager.drain_errors();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].scope, SettingsScope::Global);
    assert_eq!(
        errors[0].path.as_deref(),
        Some(fixture.global_path().to_str().unwrap())
    );
    assert_eq!(errors[1].scope, SettingsScope::Project);
    assert_eq!(
        errors[1].path.as_deref(),
        Some(fixture.project_path().to_str().unwrap())
    );
    assert!(manager.drain_errors().is_empty());
    let expected = &capture["errors"]["initial_load"];
    assert_eq!(expected.as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// theme setting
// ---------------------------------------------------------------------------

#[test]
fn slash_separated_theme_settings_store_separately() {
    let capture = oracle();
    let fixture = Fixture::new("theme");

    fixture.write_global(r#"{"theme":"light/dark"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_theme(), None);
    assert_eq!(manager.get_theme_setting(), Some("light/dark".to_string()));

    manager.set_theme("solarized-light/tokyo-night");
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["theme_write"].as_str().unwrap(),
        "theme_write bytes"
    );
}

// ---------------------------------------------------------------------------
// project trust
// ---------------------------------------------------------------------------

#[test]
fn project_trust_gates_reads_and_writes() {
    let capture = oracle();
    let values = &capture["values"];
    let fixture = Fixture::new("trust");

    fixture.write_global(r#"{"theme":"global"}"#);
    fixture.write_project(r#"{"theme":"project"}"#);
    let manager = fixture.manager_untrusted();
    assert!(!manager.is_project_trusted());
    assert_eq!(manager.get_theme(), Some("global".to_string()));
    assert_eq!(manager.get_project_settings(), obj(&[]));

    manager.set_project_trusted(true);
    assert!(manager.is_project_trusted());
    assert_eq!(manager.get_theme(), Some("project".to_string()));

    // Untrusted writes throw before touching the file.
    let untrusted = fixture.manager_untrusted();
    fixture.write_project(r#"{"packages":["npm:existing"]}"#);
    let error = untrusted
        .set_project_packages(vec![SettingsValue::str("npm:new")])
        .unwrap_err();
    assert_eq!(
        error,
        capture["errors"]["untrusted_write"].as_str().unwrap(),
        "untrusted write error text"
    );
    untrusted.flush();
    assert_eq!(untrusted.get_project_settings(), obj(&[]));
    assert_eq!(
        fixture.project_bytes(),
        capture["bytes"]["untrusted_write_file"].as_str().unwrap(),
        "untrusted project file untouched"
    );

    // Default project trust reads global settings only.
    fixture.write_global(r#"{"defaultProjectTrust":"always"}"#);
    fixture.write_project(r#"{"defaultProjectTrust":"never"}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_default_project_trust(),
        DefaultProjectTrust::Always
    );

    fixture.write_global(r#"{"defaultProjectTrust":"sometimes"}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_default_project_trust(),
        DefaultProjectTrust::Ask
    );
    let _ = values;
}

// ---------------------------------------------------------------------------
// project settings directory creation
// ---------------------------------------------------------------------------

#[test]
fn project_directory_is_created_only_when_writing() {
    let capture = oracle();
    let fixture = Fixture::new("pi-dir");

    fixture.write_global(r#"{"theme":"dark"}"#);
    std::fs::remove_dir_all(fixture.project_dir.join(".pi")).unwrap();
    assert!(!fixture.project_dir.join(".pi").exists());

    let manager = fixture.manager();
    assert!(!fixture.project_dir.join(".pi").exists());
    assert_eq!(manager.get_theme(), Some("dark".to_string()));

    manager
        .set_project_packages(vec![obj(&[("source", SettingsValue::str("npm:test-pkg"))])])
        .unwrap();
    manager.flush();
    assert!(fixture.project_dir.join(".pi").exists());
    assert!(fixture.project_path().exists());
    assert_eq!(
        fixture.project_bytes(),
        capture["bytes"]["project_first_write"].as_str().unwrap(),
        "project_first_write bytes"
    );
}

// ---------------------------------------------------------------------------
// terminal capability overrides / retry / timeouts
// ---------------------------------------------------------------------------

#[test]
fn terminal_capability_overrides_map_explicit_and_omit_auto() {
    let values = oracle()["values"].clone();
    let overrides = |terminal: SettingsValue| {
        let manager = SettingsManager::in_memory(obj(&[("terminal", terminal)]));
        manager.get_terminal_capability_overrides()
    };

    let clear = overrides(obj(&[
        ("images", SettingsValue::Bool(false)),
        ("trueColor", SettingsValue::Bool(false)),
        ("hyperlinks", SettingsValue::Bool(false)),
    ]));
    assert_eq!(clear.to_json_value(), values["term_overrides_clear"]);
    let explicit = overrides(obj(&[
        ("images", SettingsValue::str("kitty")),
        ("trueColor", SettingsValue::Bool(true)),
        ("hyperlinks", SettingsValue::Bool(true)),
    ]));
    assert_eq!(explicit.to_json_value(), values["term_overrides_explicit"]);
    let auto = overrides(obj(&[
        ("images", SettingsValue::str("auto")),
        ("trueColor", SettingsValue::str("auto")),
        ("hyperlinks", SettingsValue::str("auto")),
    ]));
    assert_eq!(auto.to_json_value(), values["term_overrides_auto"]);
}

#[test]
fn retry_settings_defaults_and_overrides() {
    let values = oracle()["values"].clone();
    let defaults = SettingsManager::in_memory(obj(&[])).get_retry_settings();
    assert_eq!(
        serde_json::to_value(defaults).unwrap(),
        values["retry_defaults"]
    );
    let overridden = SettingsManager::in_memory(obj(&[(
        "retry",
        obj(&[
            ("enabled", SettingsValue::Bool(true)),
            ("maxRetries", SettingsValue::Num(10.0)),
            ("baseDelayMs", SettingsValue::Num(500.0)),
            ("maxAgentDelayMs", SettingsValue::Num(5000.0)),
        ]),
    )]))
    .get_retry_settings();
    assert_eq!(
        serde_json::to_value(overridden).unwrap(),
        values["retry_overrides"]
    );
}

#[test]
fn http_idle_timeout_defaults_merges_and_validates() {
    let capture = oracle();
    let fixture = Fixture::new("http-idle");

    let manager = fixture.manager();
    assert_eq!(
        manager.get_http_idle_timeout_ms().unwrap(),
        DEFAULT_HTTP_IDLE_TIMEOUT_MS
    );

    fixture.write_global(r#"{"httpIdleTimeoutMs":300000}"#);
    fixture.write_project(r#"{"httpIdleTimeoutMs":0}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_http_idle_timeout_ms().unwrap(), 0);
    assert_eq!(capture["values"]["http_idle_merged"].as_u64(), Some(0));

    // Invalid values throw with the pinned text (project file removed so
    // its 0 cannot mask the global -1).
    std::fs::remove_file(fixture.project_path()).unwrap();
    fixture.write_global(r#"{"httpIdleTimeoutMs":-1}"#);
    let manager = fixture.manager();
    let error = manager.get_http_idle_timeout_ms().unwrap_err();
    assert_eq!(
        error,
        capture["errors"]["http_idle_invalid"].as_str().unwrap(),
        "http_idle_invalid error text"
    );
}

#[test]
fn set_http_idle_timeout_rejects_invalid_runtime_values() {
    let capture = oracle();
    let manager = SettingsManager::in_memory(obj(&[]));
    assert_eq!(
        manager.set_http_idle_timeout_ms(f64::NAN).unwrap_err(),
        capture["errors"]["set_http_idle_NaN"].as_str().unwrap()
    );
    assert_eq!(
        manager.set_http_idle_timeout_ms(-2.5).unwrap_err(),
        capture["errors"]["set_http_idle_-2.5"].as_str().unwrap()
    );
}

#[test]
fn external_editor_resolves_by_precedence() {
    let capture = oracle();
    let values = &capture["values"];
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let original_visual = std::env::var("VISUAL").ok();
    let original_editor = std::env::var("EDITOR").ok();
    let set_env = |visual: Option<&str>, editor: Option<&str>| {
        match visual {
            Some(value) => std::env::set_var("VISUAL", value),
            None => std::env::remove_var("VISUAL"),
        }
        match editor {
            Some(value) => std::env::set_var("EDITOR", value),
            None => std::env::remove_var("EDITOR"),
        }
    };

    set_env(Some("vim"), Some("nano"));
    let manager = SettingsManager::in_memory(obj(&[(
        "externalEditor",
        SettingsValue::str("code --wait"),
    )]));
    assert_eq!(
        manager.get_external_editor_command(),
        values["editor_configured_setting"]
    );
    assert_eq!(
        SettingsManager::in_memory(obj(&[])).get_external_editor_command(),
        values["editor_env_visual"]
    );

    set_env(None, Some("emacs"));
    assert_eq!(
        SettingsManager::in_memory(obj(&[])).get_external_editor_command(),
        values["editor_env_editor"]
    );

    // Platform default: this oracle capture is the win32 pin ("notepad").
    set_env(None, None);
    let platform_default = SettingsManager::in_memory(obj(&[])).get_external_editor_command();
    if cfg!(windows) {
        assert_eq!(
            platform_default,
            values["editor_platform_default"].as_str().unwrap()
        );
    } else {
        assert_eq!(platform_default, "nano");
    }

    match original_visual {
        Some(value) => std::env::set_var("VISUAL", value),
        None => std::env::remove_var("VISUAL"),
    }
    match original_editor {
        Some(value) => std::env::set_var("EDITOR", value),
        None => std::env::remove_var("EDITOR"),
    }
}

// ---------------------------------------------------------------------------
// TUI / fullscreen / outputPad / mermaid
// ---------------------------------------------------------------------------

#[test]
fn tui_mode_defaults_persists_and_validates() {
    let capture = oracle();
    let fixture = Fixture::new("tui");

    let manager = fixture.manager();
    // v1.0.0 flips the default to fullscreen and only recognizes "regular";
    // the captured oracle predates the flip, so the default is pinned here
    // instead of compared against it.
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);
    manager.set_tui_mode(TuiMode::Fullscreen);
    manager.flush();
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);

    let saved: serde_json::Value = serde_json::from_str(&fixture.global_bytes()).unwrap();
    assert_eq!(saved["tuiMode"], serde_json::json!("fullscreen"));

    // An unrecognized value falls back to the (now fullscreen) default.
    fixture.write_global(r#"{"tuiMode":"other"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);

    // The old uiMode key is not recognized.
    fixture.write_global(r#"{"uiMode":"fullscreen"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);
    let _ = capture;
}

#[test]
fn fullscreen_settings_validate_and_persist() {
    let capture = oracle();
    let fixture = Fixture::new("fullscreen");

    let manager = fixture.manager();
    assert_eq!(
        manager.get_fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        manager.get_fullscreen_scrollbar(),
        FullscreenScrollbar::Auto
    );
    assert!(manager.get_fullscreen_copy_on_select());

    // The oracle capture runs this sequence on top of the legacy `uiMode`
    // document; replicate it so the byte pin lines up.
    fixture.write_global(r#"{"uiMode":"fullscreen"}"#);
    let manager = fixture.manager();
    manager.set_fullscreen_exit_output(FullscreenExitOutput::ResumeHint);
    manager.set_fullscreen_scrollbar(FullscreenScrollbar::Hidden);
    manager.set_fullscreen_copy_on_select(false);
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["fullscreen_write"].as_str().unwrap(),
        "fullscreen_write bytes"
    );

    fixture.write_global(r#"{"fullscreenExitOutput":"nothing","fullscreenScrollbar":"sometimes"}"#);
    let reloaded = fixture.manager();
    assert_eq!(
        reloaded.get_fullscreen_exit_output(),
        FullscreenExitOutput::Transcript
    );
    assert_eq!(
        reloaded.get_fullscreen_scrollbar(),
        FullscreenScrollbar::Auto
    );
    assert!(reloaded.get_fullscreen_copy_on_select());

    // outputPad: 0 persists; other values fall back to 1 (bytes captured
    // after the invalid-fullscreen document, like the oracle run).
    assert_eq!(reloaded.get_output_pad(), 1);
    reloaded.set_output_pad(0);
    reloaded.flush();
    assert_eq!(reloaded.get_output_pad(), 0);
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["output_pad_write"].as_str().unwrap(),
        "output_pad_write bytes"
    );

    fixture.write_global(r#"{"outputPad":2}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_output_pad(), 1);

    // mermaid: streaming default, persisted final mode (bytes captured on
    // top of the outputPad document, like the oracle run).
    let manager = fixture.manager();
    assert_eq!(
        manager.get_mermaid_rendering_mode(),
        MermaidRenderingMode::Streaming
    );
    manager.set_mermaid_rendering_mode(MermaidRenderingMode::Final);
    manager.flush();
    assert_eq!(
        manager.get_mermaid_rendering_mode(),
        MermaidRenderingMode::Final
    );
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["mermaid_write"].as_str().unwrap(),
        "mermaid_write bytes"
    );

    fixture.write_global(r#"{"markdown":{"mermaid":"sometimes"}}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_mermaid_rendering_mode(),
        MermaidRenderingMode::Streaming
    );
}

// ---------------------------------------------------------------------------
// shellCommandPrefix / defaultTools / sessionDir / shellPath
// ---------------------------------------------------------------------------

#[test]
fn shell_command_prefix_loads_and_survives_unrelated_saves() {
    let capture = oracle();
    let fixture = Fixture::new("shell-prefix");

    fixture.write_global(r#"{"shellCommandPrefix":"shopt -s expand_aliases"}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_shell_command_prefix(),
        Some("shopt -s expand_aliases".to_string())
    );
    manager.set_theme("light");
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["shell_prefix_preserved"].as_str().unwrap(),
        "shell_prefix_preserved bytes"
    );

    fixture.write_global(r#"{"theme":"dark"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_shell_command_prefix(), None);
}

#[test]
fn default_tools_load_globally_and_project_replaces() {
    let values = oracle()["values"].clone();
    let fixture = Fixture::new("default-tools");

    fixture.write_global(r#"{"defaultTools":["read","bash"]}"#);
    assert_eq!(
        fixture.manager().get_default_tools(),
        Some(vec!["read".to_string(), "bash".to_string()])
    );
    fixture.write_project(r#"{"defaultTools":["grep"]}"#);
    assert_eq!(
        fixture.manager().get_default_tools(),
        Some(vec!["grep".to_string()])
    );
    assert_eq!(
        SettingsManager::in_memory(obj(&[("defaultTools", arr(&[]))])).get_default_tools(),
        Some(Vec::new()),
        "an empty tool list is preserved"
    );
    assert_eq!(
        SettingsManager::in_memory(obj(&[])).get_default_tools(),
        None
    );
    let _ = values;
}

#[test]
fn session_dir_and_shell_path_expand_tilde() {
    let fixture = Fixture::new("paths");
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .expect("home directory must be resolvable like node's os.homedir()");

    fixture.write_global(r#"{"theme":"dark"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_session_dir(), None);

    fixture.write_global(r#"{"sessionDir":"/tmp/sessions"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_session_dir(), Some("/tmp/sessions".to_string()));

    // Project overrides global; the value is returned as configured.
    fixture.write_global(r#"{"sessionDir":"/global/sessions"}"#);
    fixture.write_project(r#"{"sessionDir":"./sessions"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_session_dir(), Some("./sessions".to_string()));

    // `~` expands to the home directory (join semantics per host platform);
    // the project override from the previous scenario is removed first.
    std::fs::remove_file(fixture.project_path()).unwrap();
    fixture.write_global(r#"{"sessionDir":"~/sessions"}"#);
    let manager = fixture.manager();
    let expected = join_home(&home, "sessions");
    assert_eq!(manager.get_session_dir().unwrap(), expected);

    fixture.write_global(r#"{"shellPath":"~/.local/bin/agent-shell-sandbox"}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_shell_path().unwrap(),
        join_home(&home, ".local/bin/agent-shell-sandbox")
    );

    fixture.write_global(r#"{"shellPath":"~"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_shell_path().unwrap(), home);

    fixture.write_global(r#"{"shellPath":"/bin/zsh"}"#);
    let manager = fixture.manager();
    assert_eq!(manager.get_shell_path(), Some("/bin/zsh".to_string()));
    assert_eq!(manager.get_shell_path(), Some("/bin/zsh".to_string()));
}

fn join_home(home: &str, segment: &str) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&[home, segment])
    } else {
        crate::coding_agent::utils::node_path::posix_join(&[home, segment])
    }
}

// ---------------------------------------------------------------------------
// Compaction (settings-manager-compaction.test.ts)
// ---------------------------------------------------------------------------

fn compaction_model() -> (&'static str, &'static str) {
    ("provider", "family/model")
}

#[test]
fn compaction_uses_defaults_without_settings() {
    let values = oracle()["values"].clone();
    let manager = SettingsManager::in_memory(obj(&[]));
    let (provider, id) = compaction_model();
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings().unwrap()).unwrap(),
        values["compaction_defaults_plain"]
    );
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_defaults_model"]
    );
}

#[test]
fn compaction_resolves_each_field_independently() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let model_key = format!("{provider}/{id}");
    let manager = SettingsManager::in_memory(obj(&[(
        "compaction",
        obj(&[
            ("reserveTokens", SettingsValue::Num(8192.0)),
            ("keepRecentTokens", SettingsValue::Num(10000.0)),
            (
                "modelOverrides",
                obj(&[(
                    model_key.as_str(),
                    obj(&[("reserveTokens", SettingsValue::Num(400000.0))]),
                )]),
            ),
        ]),
    )]));

    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_per_field"]
    );
    assert_eq!(
        manager
            .get_compaction_reserve_tokens_for(provider, id)
            .unwrap(),
        400000
    );
    assert_eq!(
        manager
            .get_compaction_keep_recent_tokens_for(provider, id)
            .unwrap(),
        10000
    );
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings().unwrap()).unwrap(),
        values["compaction_no_model"]
    );

    manager.apply_overrides(&obj(&[(
        "compaction",
        obj(&[(
            "modelOverrides",
            obj(&[(
                model_key.as_str(),
                obj(&[("keepRecentTokens", SettingsValue::Num(30000.0))]),
            )]),
        )]),
    )]));
    assert_eq!(
        manager
            .get_compaction_keep_recent_tokens_for(provider, id)
            .unwrap(),
        30000
    );
    assert_eq!(
        manager
            .get_compaction_reserve_tokens_for(provider, id)
            .unwrap(),
        400000
    );
}

#[test]
fn compaction_falls_back_to_defaults_for_missing_fields() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let model_key = format!("{provider}/{id}");
    let manager = SettingsManager::in_memory(obj(&[(
        "compaction",
        obj(&[(
            "modelOverrides",
            obj(&[(
                model_key.as_str(),
                obj(&[("keepRecentTokens", SettingsValue::Num(1024.0))]),
            )]),
        )]),
    )]));
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_partial_override"]
    );
}

#[test]
fn compaction_matches_exact_provider_model_ids_including_slashes() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let model_key = format!("{provider}/{id}");
    let manager = SettingsManager::in_memory(obj(&[(
        "compaction",
        obj(&[(
            "modelOverrides",
            obj(&[
                (
                    model_key.as_str(),
                    obj(&[("reserveTokens", SettingsValue::Num(400000.0))]),
                ),
                (
                    "provider/*",
                    obj(&[("reserveTokens", SettingsValue::Num(1.0))]),
                ),
                (
                    "family/model",
                    obj(&[("reserveTokens", SettingsValue::Num(2.0))]),
                ),
            ]),
        )]),
    )]));
    assert_eq!(
        manager
            .get_compaction_reserve_tokens_for(provider, id)
            .unwrap(),
        400000
    );
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for("other", id).unwrap()).unwrap(),
        values["compaction_other_provider"]
    );
    assert_eq!(
        serde_json::to_value(
            manager
                .get_compaction_settings_for(provider, "other")
                .unwrap()
        )
        .unwrap(),
        values["compaction_other_id"]
    );
    assert_eq!(
        serde_json::to_value(
            manager
                .get_compaction_settings_for(provider, "family/Model")
                .unwrap()
        )
        .unwrap(),
        values["compaction_case_id"]
    );
}

#[test]
fn compaction_merges_project_overrides_per_field() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let storage = InMemorySettingsStorage::default();
    storage
        .with_lock(SettingsScope::Global, Box::new(|_| {
            Ok(Some(
                r#"{"compaction":{"reserveTokens":8192,"modelOverrides":{"provider/family/model":{"reserveTokens":400000,"keepRecentTokens":30000},"provider/other":{"keepRecentTokens":4096}}}}"#.to_string(),
            ))
        }))
        .unwrap();
    storage
        .with_lock(SettingsScope::Project, Box::new(|_| {
            Ok(Some(
                r#"{"compaction":{"reserveTokens":1024,"modelOverrides":{"provider/family/model":{"keepRecentTokens":2000}}}}"#
                    .to_string(),
            ))
        }))
        .unwrap();
    let manager = SettingsManager::from_storage(Arc::new(storage), Default::default());
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_project_merge"]
    );
    assert_eq!(
        serde_json::to_value(
            manager
                .get_compaction_settings_for(provider, "other")
                .unwrap()
        )
        .unwrap(),
        values["compaction_project_merge_other"]
    );
    manager.reload();
    assert_eq!(
        manager
            .get_compaction_keep_recent_tokens_for(provider, id)
            .unwrap(),
        2000
    );
    manager.set_project_trusted(false);
    assert_eq!(
        manager
            .get_compaction_keep_recent_tokens_for(provider, id)
            .unwrap(),
        30000
    );
}

#[test]
fn compaction_keeps_enabled_global_and_preserves_overrides_on_toggle() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let storage = InMemorySettingsStorage::default();
    storage
        .with_lock(SettingsScope::Global, Box::new(|_| {
            Ok(Some(
                r#"{"compaction":{"modelOverrides":{"provider/family/model":{"enabled":false,"reserveTokens":400000}}}}"#
                    .to_string(),
            ))
        }))
        .unwrap();
    let manager = SettingsManager::from_storage(Arc::new(storage), Default::default());
    assert!(
        manager
            .get_compaction_settings_for(provider, id)
            .unwrap()
            .enabled
    );
    manager.set_compaction_enabled(false);
    manager.reload();
    let settings = manager.get_compaction_settings_for(provider, id).unwrap();
    assert_eq!(
        serde_json::to_value(settings).unwrap(),
        values["compaction_after_toggle"]
    );
}

#[test]
fn compaction_reports_invalid_token_values() {
    let errors = oracle()["errors"].clone();
    let (provider, id) = compaction_model();
    let model_key = format!("{provider}/{id}");

    let invalid_values: Vec<(&str, SettingsValue)> = vec![
        ("null", SettingsValue::Null),
        ("-1", SettingsValue::Num(-1.0)),
        ("1.5", SettingsValue::Num(1.5)),
        ("\"400000\"", SettingsValue::str("400000")),
        ("true", SettingsValue::Bool(true)),
        ("{}", obj(&[])),
        ("[]", SettingsValue::Arr(Vec::new())),
        (
            "9007199254740992",
            SettingsValue::Num(9_007_199_254_740_992.0),
        ),
    ];
    for field in ["reserveTokens", "keepRecentTokens"] {
        for (label, value) in &invalid_values {
            // Model override entry (full document keeps key order aligned
            // with the oracle captures).
            let document = format!(
                r#"{{"compaction":{{"modelOverrides":{{"provider/family/model":{{"{field}":{}}}}}}}}}"#,
                serde_json::to_string(&settings_value_to_serde_for_tests(value)).unwrap()
            );
            let storage = InMemorySettingsStorage::default();
            storage
                .with_lock(SettingsScope::Global, Box::new(move |_| Ok(Some(document))))
                .unwrap();
            let manager = SettingsManager::from_storage(Arc::new(storage), Default::default());
            let error = manager
                .get_compaction_settings_for(provider, id)
                .unwrap_err();
            assert_eq!(
                error,
                errors[format!("override_{field}_{label}")]
                    .as_str()
                    .unwrap(),
                "override {field} {label}"
            );
            assert!(manager.get_compaction_settings().is_ok());
            assert!(manager.get_compaction_settings_for("other", id).is_ok());

            // Ordinary setting (invalid even when a valid override exists).
            let document = format!(
                r#"{{"compaction":{{"{field}":{},"modelOverrides":{{"provider/family/model":{{"{field}":4096}}}}}}}}"#,
                serde_json::to_string(&settings_value_to_serde_for_tests(value)).unwrap()
            );
            let storage = InMemorySettingsStorage::default();
            storage
                .with_lock(SettingsScope::Global, Box::new(move |_| Ok(Some(document))))
                .unwrap();
            let manager = SettingsManager::from_storage(Arc::new(storage), Default::default());
            let error = manager.get_compaction_settings().unwrap_err();
            assert_eq!(
                error,
                errors[format!("ordinary_{field}_{label}")]
                    .as_str()
                    .unwrap(),
                "ordinary {field} {label}"
            );
            assert!(manager.get_compaction_settings_for(provider, id).is_err());
        }

        // Non-finite runtime values injected through applyOverrides (JSON
        // cannot carry them).
        for (name, value) in [
            ("NaN", f64::NAN),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            let manager = SettingsManager::in_memory(obj(&[]));
            manager.apply_overrides(&obj(&[(
                "compaction",
                obj(&[(
                    "modelOverrides",
                    obj(&[(
                        model_key.as_str(),
                        obj(&[(field, SettingsValue::Num(value))]),
                    )]),
                )]),
            )]));
            let error = manager
                .get_compaction_settings_for(provider, id)
                .unwrap_err();
            assert_eq!(
                error,
                errors[format!("runtime_override_{field}_{name}")]
                    .as_str()
                    .unwrap()
            );

            let manager = SettingsManager::in_memory(obj(&[]));
            manager.apply_overrides(&obj(&[(
                "compaction",
                obj(&[(field, SettingsValue::Num(value))]),
            )]));
            let error = manager.get_compaction_settings().unwrap_err();
            assert_eq!(
                error,
                errors[format!("runtime_ordinary_{field}_{name}")]
                    .as_str()
                    .unwrap()
            );
        }
    }
}

#[test]
fn compaction_reports_malformed_model_entries() {
    let errors = oracle()["errors"].clone();
    let (provider, id) = compaction_model();
    for (label, entry) in [
        ("null", "null"),
        ("false", "false"),
        ("42", "42"),
        ("\"invalid\"", "\"invalid\""),
        ("[]", "[]"),
    ] {
        let document =
            format!(r#"{{"compaction":{{"modelOverrides":{{"provider/family/model":{entry}}}}}}}"#);
        let storage = InMemorySettingsStorage::default();
        storage
            .with_lock(SettingsScope::Global, Box::new(move |_| Ok(Some(document))))
            .unwrap();
        let manager = SettingsManager::from_storage(Arc::new(storage), Default::default());
        let error = manager
            .get_compaction_settings_for(provider, id)
            .unwrap_err();
        assert_eq!(
            error,
            errors[format!("entry_{label}")].as_str().unwrap(),
            "entry {label}"
        );
    }
}

#[test]
fn compaction_accepts_zero() {
    let values = oracle()["values"].clone();
    let (provider, id) = compaction_model();
    let model_key = format!("{provider}/{id}");
    let manager = SettingsManager::in_memory(obj(&[(
        "compaction",
        obj(&[
            ("reserveTokens", SettingsValue::Num(0.0)),
            ("keepRecentTokens", SettingsValue::Num(0.0)),
        ]),
    )]));
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_zero"]
    );
    manager.apply_overrides(&obj(&[(
        "compaction",
        obj(&[
            ("reserveTokens", SettingsValue::Num(1000.0)),
            ("keepRecentTokens", SettingsValue::Num(1000.0)),
            (
                "modelOverrides",
                obj(&[(
                    model_key.as_str(),
                    obj(&[
                        ("reserveTokens", SettingsValue::Num(0.0)),
                        ("keepRecentTokens", SettingsValue::Num(0.0)),
                    ]),
                )]),
            ),
        ]),
    )]));
    assert_eq!(
        serde_json::to_value(manager.get_compaction_settings_for(provider, id).unwrap()).unwrap(),
        values["compaction_zero_override"]
    );
}

// ---------------------------------------------------------------------------
// External-edit preservation (settings-manager-bug.test.ts)
// ---------------------------------------------------------------------------

#[test]
fn preserves_file_changes_to_arrays_when_changing_unrelated_settings() {
    let fixture = Fixture::new("bug-packages");

    fixture.write_global(r#"{"theme":"dark","packages":["npm:pi-mcp-adapter"]}"#);
    let manager = fixture.manager();
    assert_eq!(
        manager.get_packages(),
        vec![SettingsValue::str("npm:pi-mcp-adapter")]
    );

    // External edit empties the packages array (insertion order).
    let mut current =
        parse_settings_value(r#"{"theme":"dark","packages":["npm:pi-mcp-adapter"]}"#).unwrap();
    current.set("packages", SettingsValue::Arr(Vec::new()));
    fixture.write_global(&stringify_pretty_for_tests(&current));

    // An unrelated save must not resurrect the stale packages.
    manager.set_theme("light");
    manager.flush();
    let saved: serde_json::Value = serde_json::from_str(&fixture.global_bytes()).unwrap();
    assert_eq!(saved["packages"], serde_json::json!([]));
    assert_eq!(saved["theme"], serde_json::json!("light"));
}

#[test]
fn preserves_file_changes_to_extensions_array() {
    let fixture = Fixture::new("bug-extensions");

    fixture.write_global(r#"{"theme":"dark","extensions":["/old/extension.ts"]}"#);
    let manager = fixture.manager();

    let mut current =
        parse_settings_value(r#"{"theme":"dark","extensions":["/old/extension.ts"]}"#).unwrap();
    current.set("extensions", arr(&["/new/extension.ts"]));
    fixture.write_global(&stringify_pretty_for_tests(&current));

    manager.set_default_thinking_level(ThinkingLevel::High);
    manager.flush();
    let saved: serde_json::Value = serde_json::from_str(&fixture.global_bytes()).unwrap();
    assert_eq!(
        saved["extensions"],
        serde_json::json!(["/new/extension.ts"])
    );
}

#[test]
fn preserves_external_project_changes_when_updating_unrelated_project_fields() {
    let fixture = Fixture::new("bug-project");

    fixture.write_project(r#"{"extensions":["./old-extension.ts"],"prompts":["./old-prompt.md"]}"#);
    let manager = fixture.manager();

    let mut current = parse_settings_value(
        r#"{"extensions":["./old-extension.ts"],"prompts":["./old-prompt.md"]}"#,
    )
    .unwrap();
    current.set("prompts", arr(&["./new-prompt.md"]));
    fixture.write_project(&stringify_pretty_for_tests(&current));

    manager
        .set_project_extension_paths(vec!["./updated-extension.ts".to_string()])
        .unwrap();
    manager.flush();
    let saved: serde_json::Value = serde_json::from_str(&fixture.project_bytes()).unwrap();
    assert_eq!(saved["prompts"], serde_json::json!(["./new-prompt.md"]));
    assert_eq!(
        saved["extensions"],
        serde_json::json!(["./updated-extension.ts"])
    );
}

#[test]
fn in_memory_project_changes_override_external_changes_for_same_project_field() {
    let fixture = Fixture::new("bug-project-same-key");

    fixture.write_project(r#"{"extensions":["./initial-extension.ts"]}"#);
    let manager = fixture.manager();

    let mut current = parse_settings_value(r#"{"extensions":["./initial-extension.ts"]}"#).unwrap();
    current.set("extensions", arr(&["./external-extension.ts"]));
    fixture.write_project(&stringify_pretty_for_tests(&current));

    manager
        .set_project_extension_paths(vec!["./in-memory-extension.ts".to_string()])
        .unwrap();
    manager.flush();
    let saved: serde_json::Value = serde_json::from_str(&fixture.project_bytes()).unwrap();
    assert_eq!(
        saved["extensions"],
        serde_json::json!(["./in-memory-extension.ts"])
    );
}

// ---------------------------------------------------------------------------
// Empty-manager getter battery + analytics uuid + misc writes
// ---------------------------------------------------------------------------

#[test]
fn empty_manager_getter_battery_matches_the_oracle() {
    let _env = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous_clear = std::env::var("PI_CLEAR_ON_SHRINK").ok();
    let previous_cursor = std::env::var("PI_HARDWARE_CURSOR").ok();
    std::env::remove_var("PI_CLEAR_ON_SHRINK");
    std::env::remove_var("PI_HARDWARE_CURSOR");

    let capture = oracle();
    let values = &capture["values"]["empty_battery"];
    let manager = SettingsManager::in_memory(obj(&[]));

    assert_eq!(
        manager.get_last_changelog_version(),
        opt_str(&values["last_changelog_version"])
    );
    assert_eq!(manager.get_session_dir(), opt_str(&values["session_dir"]));
    assert_eq!(
        manager.get_default_provider(),
        opt_str(&values["default_provider"])
    );
    assert_eq!(
        manager.get_default_model(),
        opt_str(&values["default_model"])
    );
    assert_eq!(
        manager.get_steering_mode(),
        values["steering"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_follow_up_mode(),
        values["follow_up"].as_str().unwrap()
    );
    assert_eq!(manager.get_theme(), opt_str(&values["theme"]));
    assert_eq!(
        manager.get_theme_setting(),
        opt_str(&values["theme_setting"])
    );
    assert_eq!(
        manager.get_transport(),
        values["transport"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_compaction_enabled(),
        values["compaction_enabled"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_compaction_reserve_tokens().unwrap(),
        values["compaction_reserve"].as_i64().unwrap()
    );
    assert_eq!(
        manager.get_compaction_keep_recent_tokens().unwrap(),
        values["compaction_keep"].as_i64().unwrap()
    );
    assert_eq!(
        serde_json::to_value(manager.get_branch_summary_settings()).unwrap(),
        values["branch_summary"]
    );
    assert_eq!(
        manager.get_branch_summary_skip_prompt(),
        values["branch_summary_skip"]
    );
    assert_eq!(
        manager.get_retry_enabled(),
        values["retry_enabled"].as_bool().unwrap()
    );
    assert_eq!(
        serde_json::to_value(manager.get_retry_settings()).unwrap(),
        values["retry_settings"]
    );
    assert_eq!(
        serde_json::to_value(manager.get_provider_retry_settings()).unwrap(),
        values["provider_retry"]
    );
    assert_eq!(
        manager.get_http_idle_timeout_ms().unwrap(),
        values["http_idle"].as_u64().unwrap()
    );
    assert_eq!(
        manager.get_websocket_connect_timeout_ms().unwrap(),
        opt_i64(&values["websocket_connect"])
    );
    assert_eq!(
        manager.get_hide_thinking_block(),
        values["hide_thinking_block"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_show_cache_miss_notices(),
        values["show_cache_miss"].as_bool().unwrap()
    );
    // The captured oracle predates v1.0.0's QuietStartup widening, where the
    // accessor returns `boolean | "header"`; the boolean values it captured map
    // to `true`/`false` below.
    assert_eq!(
        manager.get_quiet_startup() == QuietStartup::Full,
        values["quiet_startup"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_default_project_trust() == DefaultProjectTrust::Ask,
        values["default_project_trust"] == "ask"
    );
    assert_eq!(
        manager.get_shell_command_prefix(),
        opt_str(&values["shell_prefix"])
    );
    assert_eq!(
        manager.get_npm_command(),
        opt_string_vec(&values["npm_command"])
    );
    assert_eq!(
        manager.get_collapse_changelog(),
        values["collapse_changelog"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_enable_install_telemetry(),
        values["install_telemetry"].as_bool().unwrap()
    );
    assert!(!manager.get_enable_analytics());
    assert_eq!(manager.get_tracking_id(), None);
    assert!(manager.get_packages().is_empty());
    assert!(manager.get_extension_paths().is_empty());
    assert!(manager.get_skill_paths().is_empty());
    assert!(manager.get_prompt_template_paths().is_empty());
    assert!(manager.get_theme_paths().is_empty());
    assert_eq!(
        manager.get_enable_skill_commands(),
        values["skill_commands"].as_bool().unwrap()
    );
    assert_eq!(manager.get_thinking_budgets(), None);
    assert_eq!(
        manager.get_show_images(),
        values["show_images"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_image_width_cells(),
        values["image_width_cells"].as_i64().unwrap()
    );
    assert_eq!(
        manager.get_clear_on_shrink(),
        values["clear_on_shrink"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_show_terminal_progress(),
        values["show_terminal_progress"].as_bool().unwrap()
    );
    // v1.0.0 flips the default to fullscreen; the empty-manager oracle
    // captured "regular" (the old default), so the new default is pinned.
    assert_eq!(manager.get_tui_mode(), TuiMode::Fullscreen);
    assert_eq!(
        manager.get_fullscreen_exit_output() == FullscreenExitOutput::Transcript,
        values["fullscreen_exit"] == "transcript"
    );
    assert_eq!(
        manager.get_fullscreen_scrollbar() == FullscreenScrollbar::Auto,
        values["fullscreen_scrollbar"] == "auto"
    );
    assert_eq!(
        manager.get_fullscreen_copy_on_select(),
        values["fullscreen_copy_on_select"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_image_auto_resize(),
        values["image_auto_resize"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_block_images(),
        values["block_images"].as_bool().unwrap()
    );
    assert_eq!(
        manager.get_enabled_models(),
        opt_string_vec(&values["enabled_models"])
    );
    assert_eq!(
        manager.get_default_tools(),
        opt_string_vec(&values["default_tools"])
    );
    assert_eq!(
        manager.get_double_escape_action(),
        values["double_escape"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_tree_filter_mode() == TreeFilterMode::Default,
        values["tree_filter"] == "default"
    );
    assert!(!manager.get_show_hardware_cursor());
    assert_eq!(
        manager.get_editor_padding_x(),
        values["editor_padding_x"].as_i64().unwrap()
    );
    assert_eq!(
        manager.get_output_pad(),
        values["output_pad"].as_i64().unwrap()
    );
    assert_eq!(
        manager.get_autocomplete_max_visible(),
        values["autocomplete_max_visible"].as_i64().unwrap()
    );
    assert_eq!(
        manager.get_code_block_indent(),
        values["code_block_indent"].as_str().unwrap()
    );
    assert_eq!(
        manager.get_mermaid_rendering_mode() == MermaidRenderingMode::Streaming,
        values["mermaid"] == "streaming"
    );
    assert_eq!(
        serde_json::to_value(manager.get_warnings()).unwrap(),
        values["warnings"]
    );
    assert!(manager.get_all_model_thinking_levels().is_empty());
    assert_eq!(manager.get_model_thinking_level("p", "m"), None);
    assert_eq!(
        manager.get_terminal_capability_overrides().to_json_value(),
        values["terminal_overrides"]
    );

    // Analytics opt-in generates a v4 tracking id (format pinned; the value
    // is random like upstream's randomUUID).
    manager.set_enable_analytics(true);
    assert!(manager.get_enable_analytics());
    let tracking_id = manager.get_tracking_id().expect("tracking id generated");
    assert_eq!(
        capture["values"]["analytics_tracking_id_shape"],
        serde_json::json!(is_uuid_v4(&tracking_id))
    );

    match previous_clear {
        Some(value) => std::env::set_var("PI_CLEAR_ON_SHRINK", value),
        None => std::env::remove_var("PI_CLEAR_ON_SHRINK"),
    }
    match previous_cursor {
        Some(value) => std::env::set_var("PI_HARDWARE_CURSOR", value),
        None => std::env::remove_var("PI_HARDWARE_CURSOR"),
    }
}

fn is_uuid_v4(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5
        || parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
    {
        return false;
    }
    value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && parts[2].starts_with('4')
        && matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b'))
}

fn opt_str(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn opt_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64()
}

fn opt_string_vec(value: &serde_json::Value) -> Option<Vec<String>> {
    if value.is_null() {
        return None;
    }
    Some(
        value
            .as_array()
            .expect("expected array or null")
            .iter()
            .map(|item| item.as_str().expect("string item").to_string())
            .collect(),
    )
}

#[test]
fn warnings_and_npm_command_write_bytes() {
    let capture = oracle();
    let fixture = Fixture::new("warnings");

    let manager = fixture.manager();
    manager.set_warnings(obj(&[("anthropicExtraUsage", SettingsValue::Bool(false))]));
    manager.set_npm_command(Some(vec![
        "mise".to_string(),
        "exec".to_string(),
        "node@20".to_string(),
        "--".to_string(),
        "npm".to_string(),
    ]));
    manager.flush();
    assert_eq!(
        fixture.global_bytes(),
        capture["bytes"]["warnings_npm_write"].as_str().unwrap(),
        "warnings_npm_write bytes"
    );
}

// ---------------------------------------------------------------------------
// inMemory() + fromStorage() surface
// ---------------------------------------------------------------------------

#[test]
fn in_memory_seeds_migrated_global_settings_and_persists_them() {
    let manager = SettingsManager::in_memory(obj(&[
        ("queueMode", SettingsValue::str("all")),
        ("theme", SettingsValue::str("dark")),
    ]));
    // queueMode migrated in memory.
    assert_eq!(manager.get_steering_mode(), "all");

    // A save merges the in-memory state into storage and the reload keeps
    // it (the seeded document already went through the migration).
    manager.set_theme("light");
    let global = manager.get_global_settings();
    assert_eq!(global.get("steeringMode"), Some(&SettingsValue::str("all")));
    assert_eq!(global.get("theme"), Some(&SettingsValue::str("light")));
    assert_eq!(global.get("queueMode"), None);
}

#[test]
fn file_settings_storage_round_trips_through_in_memory_trait_object() {
    let storage = Arc::new(InMemorySettingsStorage::default()) as Arc<dyn super::SettingsStorage>;
    storage
        .with_lock(
            SettingsScope::Global,
            Box::new(|_| Ok(Some(r#"{"theme":"dark"}"#.to_string()))),
        )
        .unwrap();
    let manager = SettingsManager::from_storage(storage, Default::default());
    assert_eq!(manager.get_theme(), Some("dark".to_string()));
}

#[test]
fn settings_value_helpers_render_like_javascript() {
    // `JSON.stringify` pretty form (document order, two-space indent).
    let document = parse_settings_value(r#"{"b":1,"a":{"y":[1,2],"x":null},"c":[]}"#).unwrap();
    assert_eq!(
        stringify_pretty_for_tests(&document),
        "{\n  \"b\": 1,\n  \"a\": {\n    \"y\": [\n      1,\n      2\n    ],\n    \"x\": null\n  },\n  \"c\": []\n}"
    );

    // `String(value)` forms pinned by the validation battery.
    assert_eq!(js_to_string_for_tests(&SettingsValue::Num(-1.0)), "-1");
    assert_eq!(js_to_string_for_tests(&SettingsValue::Num(1.5)), "1.5");
    assert_eq!(
        js_to_string_for_tests(&SettingsValue::str("400000")),
        "400000"
    );
    assert_eq!(js_to_string_for_tests(&SettingsValue::Bool(true)), "true");
    assert_eq!(js_to_string_for_tests(&obj(&[])), "[object Object]");
    assert_eq!(js_to_string_for_tests(&SettingsValue::Arr(Vec::new())), "");
    assert_eq!(
        js_to_string_for_tests(&SettingsValue::Num(9_007_199_254_740_992.0)),
        "9007199254740992"
    );
    assert_eq!(js_to_string_for_tests(&SettingsValue::Num(f64::NAN)), "NaN");
    assert_eq!(js_number_to_string_for_tests(f64::INFINITY), "Infinity");
    assert_eq!(
        js_number_to_string_for_tests(f64::NEG_INFINITY),
        "-Infinity"
    );

    // `JSON.stringify` renders non-finite numbers as null.
    assert_eq!(
        stringify_pretty_for_tests(&SettingsValue::Num(f64::NAN)),
        "null"
    );
    assert_eq!(
        stringify_pretty_for_tests(&SettingsValue::Arr(Vec::new())),
        "[]"
    );
}
