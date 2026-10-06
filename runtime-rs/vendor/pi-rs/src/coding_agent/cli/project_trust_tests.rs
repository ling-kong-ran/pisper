//! Tests for the ported `coding-agent/src/cli/project-trust.ts` — the
//! deterministic gating of `ctx.ui` (hasUI / interactive mode) and the
//! context-mode mapping.

use std::sync::{Arc, Mutex};

use crate::coding_agent::cli::project_trust::{create_project_trust_context, AppMode, NotifyType};
use crate::coding_agent::cli::startup_ui::{
    set_startup_ui_host, StartupSelectorOption, StartupUiHost,
};

#[derive(Default)]
struct RecordingHost {
    calls: Mutex<Vec<String>>,
}

impl StartupUiHost for RecordingHost {
    fn show_selector(
        &self,
        title: &str,
        _options: Vec<StartupSelectorOption<String>>,
    ) -> Option<String> {
        self.calls.lock().unwrap().push(format!("select:{title}"));
        None
    }
    fn show_input(&self, title: &str, _placeholder: Option<&str>) -> Option<String> {
        self.calls.lock().unwrap().push(format!("input:{title}"));
        None
    }
}

/// Upstream `createProjectTrustContext`: mode mapping.
#[test]
fn context_mode_mapping() {
    assert_eq!(AppMode::Interactive.context_mode(), "tui");
    assert_eq!(AppMode::Print.context_mode(), "print");
    assert_eq!(AppMode::Json.context_mode(), "json");
    assert_eq!(AppMode::Rpc.context_mode(), "rpc");
}

/// Upstream gating: no UI and non-interactive modes short-circuit to
/// `undefined`/`false` without touching the TUI.
#[test]
fn ui_gating_short_circuits_without_ui_or_interactive() {
    let host = Arc::new(RecordingHost::default());
    let host_trait: Arc<dyn StartupUiHost + Send + Sync> = host.clone();
    set_startup_ui_host(Some(host_trait));

    // Interactive + UI: host is consulted (returns None here).
    let context = create_project_trust_context(
        crate::coding_agent::cli::project_trust::CreateProjectTrustContextOptions {
            cwd: "/tmp/project".to_string(),
            mode: AppMode::Interactive,
            settings_manager: Arc::new(settings_manager()),
            has_ui: true,
        },
    );
    assert_eq!(
        context
            .ui
            .select("Trust?", &["Yes".to_string(), "No".to_string()]),
        None
    );
    assert!(!context.ui.confirm("Trust?", "really?"));
    assert_eq!(context.ui.input("Name", Some("placeholder")), None);
    assert_eq!(host.calls.lock().unwrap().len(), 3);

    // Print mode: no host calls at all.
    let print_context = create_project_trust_context(
        crate::coding_agent::cli::project_trust::CreateProjectTrustContextOptions {
            cwd: "/tmp/project".to_string(),
            mode: AppMode::Print,
            settings_manager: Arc::new(settings_manager()),
            has_ui: true,
        },
    );
    assert_eq!(
        print_context.ui.select("Trust?", &["Yes".to_string()]),
        None
    );
    assert!(!print_context.ui.confirm("Trust?", "really?"));
    assert_eq!(print_context.ui.input("Name", None), None);

    // Interactive but hasUI=false: no host calls either.
    let headless = create_project_trust_context(
        crate::coding_agent::cli::project_trust::CreateProjectTrustContextOptions {
            cwd: "/tmp/project".to_string(),
            mode: AppMode::Interactive,
            settings_manager: Arc::new(settings_manager()),
            has_ui: false,
        },
    );
    assert_eq!(headless.ui.select("Trust?", &["Yes".to_string()]), None);
    assert!(!headless.ui.confirm("Trust?", "really?"));
    assert_eq!(
        host.calls.lock().unwrap().len(),
        3,
        "no additional host calls"
    );

    set_startup_ui_host(None);
}

/// Minimal in-memory settings manager (the context carries it for the TUI
/// seam).
fn settings_manager() -> crate::coding_agent::core::settings_manager::SettingsManager {
    use crate::coding_agent::core::settings_manager::SettingsValue;
    crate::coding_agent::core::settings_manager::SettingsManager::in_memory(SettingsValue::obj(
        vec![],
    ))
}

/// Non-interactive notify mirrors upstream: stderr without colors.
#[test]
fn notify_in_non_interactive_mode_writes_plain_text() {
    let context = create_project_trust_context(
        crate::coding_agent::cli::project_trust::CreateProjectTrustContextOptions {
            cwd: "/tmp/project".to_string(),
            mode: AppMode::Json,
            settings_manager: Arc::new(settings_manager()),
            has_ui: false,
        },
    );
    // The notify path must not panic for any severity; upstream picks a chalk
    // color per type (plain here — divergence 1).
    context.ui.notify("untrusted folder", NotifyType::Warning);
    context.ui.notify("fatal", NotifyType::Error);
    context.ui.notify("info", NotifyType::Info);
}

#[tokio::test]
async fn runtime_trust_bridge_keeps_mode_flag_and_async_ui_gating() {
    use super::{create_runtime_project_trust_context, CreateProjectTrustContextOptions};
    use crate::coding_agent::extensions::types::{ExtensionMode, ExtensionUiDialogOptions};
    for (mode, expected) in [
        (AppMode::Interactive, ExtensionMode::Tui),
        (AppMode::Print, ExtensionMode::Print),
        (AppMode::Json, ExtensionMode::Json),
        (AppMode::Rpc, ExtensionMode::Rpc),
    ] {
        let ctx = create_runtime_project_trust_context(CreateProjectTrustContextOptions {
            cwd: "bridge-cwd".into(),
            mode,
            settings_manager: Arc::new(settings_manager()),
            has_ui: false,
        });
        assert_eq!(ctx.cwd, "bridge-cwd");
        assert_eq!(ctx.mode, expected);
        assert!(!ctx.has_ui);
        let ui = ctx.ui.unwrap();
        let options = ExtensionUiDialogOptions::default();
        assert_eq!(
            ui.select("trust", &["yes".into()], &options).await.unwrap(),
            None
        );
        assert!(!ui.confirm("trust", "really?", &options).await.unwrap());
        assert_eq!(ui.input("name", None, &options).await.unwrap(), None);
    }
}
