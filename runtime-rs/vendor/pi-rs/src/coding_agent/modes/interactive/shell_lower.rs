// The interactive UI glue mirrors upstream callback signatures whose types
// are inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! The interactive shell's LOWER HALF (upstream interactive-mode.ts lines
//! ~1647-6648): component instantiation and mounting, the selector mechanism
//! and every selector wiring, the footer/status data pump, the extension UI
//! event bridge, the command handlers, and the exit/cleanup paths.
//!
//! Ported from the byte-verbatim oracle bodies captured in
//! `tests/fixtures/interactive_r20_oracle/` (134 driven scenarios; replayed
//! byte-for-byte in `interactive_tests.rs`, `lower_oracle` group).

use std::sync::Arc;

use serde_json::{json, Value};

use super::interactive_mode::ShellEditor;
use super::interactive_mode::{
    get_compact_extension_labels, AuthProviderOption, ComponentKind, ComponentRef, ContainerId,
    FocusTarget, HostError, ModelRef, UserBashOutcome,
};
use super::shell::{
    CommandSink, ExtensionDialog, InteractiveMode, SelectorToken, ShellCommand, WidgetContent,
    WidgetPlacement, MAX_WIDGET_LINES,
};
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
pub use crate::coding_agent::agent_session::parse_skill_block;
use crate::coding_agent::core::settings_manager::QuietStartup;
use crate::coding_agent::modes::interactive::components::oauth_selector::is_env_var_list;
use crate::coding_agent::session_manager::SessionEntry;

/// `CONFIG_DIR_NAME`.
pub const CONFIG_DIR_NAME: &str = ".pi";

/// The selector a cancelled login reopens (v1.0.0 `onBack`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BackTarget {
    #[default]
    None,
    /// Reopen the top-level auth-method selector.
    AuthTypeSelector,
    /// Reopen the provider selector for the auth type it was opened with.
    ProviderSelector(Option<String>),
}

/// Upstream `RADIUS_LOGIN_INTRO` (v1.0.0): the description shown above the
/// Radius login's auth-method options.
pub const RADIUS_LOGIN_INTRO: &str =
    "Radius is a service crafted for Pi by the builders of Pi, Earendil Works";

/// `DEFAULT_THINKING_LEVEL`.
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;

/// `THINKING_LEVEL_OPTIONS`.
pub const THINKING_LEVEL_OPTIONS: [ThinkingLevel; 7] = [
    ThinkingLevel::Off,
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
    ThinkingLevel::Xhigh,
    ThinkingLevel::Max,
];

// ===========================================================================
// Pure helpers (verbatim upstream logic)
// ===========================================================================

/// Upstream `thinking level` string form (`"off" | … | "max"`).
pub fn thinking_level_lower(level: &ThinkingLevel) -> String {
    let s = serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    s.unwrap_or_else(|| "medium".to_string())
}

/// Upstream `assistantToolCalls(message)` — `(id, name, arguments)` triples.
pub fn assistant_tool_calls(message: &AgentMessage) -> Vec<(String, String, Value)> {
    let AgentMessage::Assistant(assistant) = message else {
        return Vec::new();
    };
    assistant
        .content
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::message::AssistantBlock::ToolCall(call) => {
                let mut arguments = call.arguments.clone();
                if arguments.is_null() {
                    arguments = json!({});
                }
                Some((call.id.clone(), call.name.clone(), arguments))
            }
            _ => None,
        })
        .collect()
}

/// Upstream `message.stopReason`.
pub fn assistant_stop_reason(message: &AgentMessage) -> Option<&'static str> {
    let AgentMessage::Assistant(assistant) = message else {
        return None;
    };
    match assistant.stop_reason {
        crate::ai::types::primitives::StopReason::ToolUse => Some("toolUse"),
        crate::ai::types::primitives::StopReason::Aborted => Some("aborted"),
        crate::ai::types::primitives::StopReason::Error => Some("error"),
        _ => None,
    }
}

/// Upstream `message.errorMessage`.
pub fn assistant_error_message(message: &AgentMessage) -> Option<String> {
    let AgentMessage::Assistant(assistant) = message else {
        return None;
    };
    assistant.error_message.clone()
}

/// Upstream `message.errorMessage = …` mutation.
pub fn assistant_set_error_message(message: &mut AgentMessage, error: &str) {
    match message {
        AgentMessage::Assistant(assistant) => assistant.error_message = Some(error.to_string()),
        // The lossless custom capture carries the wire field directly.
        AgentMessage::Custom(custom) => {
            custom
                .data
                .insert("errorMessage".to_string(), Value::String(error.to_string()));
        }
        _ => {}
    }
}

/// Upstream `message.diagnostics`.
pub fn assistant_diagnostics(message: &AgentMessage) -> Option<Value> {
    let AgentMessage::Assistant(assistant) = message else {
        return None;
    };
    serde_json::to_value(assistant.diagnostics.as_ref()?).ok()
}

/// Upstream `message.toolCallId`.
pub fn tool_result_call_id(message: &AgentMessage) -> String {
    let AgentMessage::ToolResult(result) = message else {
        return String::new();
    };
    result.tool_call_id.clone()
}

/// Upstream `createCompactionSummaryMessage` /
/// `createBranchSummaryMessage` (core/messages.ts).
pub fn create_summary_custom_message(
    _role: &str,
    summary: &str,
    tokens_before: i64,
    timestamp_ms: i64,
) -> AgentMessage {
    let mut data = serde_json::Map::new();
    data.insert("summary".to_string(), Value::String(summary.to_string()));
    data.insert("tokensBefore".to_string(), json!(tokens_before));
    data.insert("timestamp".to_string(), json!(timestamp_ms));
    AgentMessage::Custom(crate::agent_core::types::CustomAgentMessage {
        role: "compactionSummary".to_string(),
        data,
    })
}

/// Upstream `##\s+\[?(\d+\.\d+\.\d+)\]?` first-version probe.
pub fn first_changelog_version(markdown: &str) -> Option<String> {
    for line in markdown.lines() {
        let trimmed = line.trim_start_matches(['#', ' ', '[', ']']);
        let candidate: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if candidate.matches('.').count() == 2 {
            return Some(candidate);
        }
    }
    None
}

/// Upstream `CompactionReason` → `"manual" | "threshold" | "overflow"`.
pub fn reason_str(reason: crate::coding_agent::agent_session::CompactionReason) -> &'static str {
    match reason {
        crate::coding_agent::agent_session::CompactionReason::Manual => "manual",
        crate::coding_agent::agent_session::CompactionReason::Threshold => "threshold",
        crate::coding_agent::agent_session::CompactionReason::Overflow => "overflow",
    }
}

/// Upstream `RenderSessionItem` union.
#[derive(Debug, Clone)]
pub enum RenderSessionItem {
    CustomEntry(SessionEntry),
    /// `usage` entries with `kind: "cache_warm"` (delta: rendered as cache
    /// warming notices on replay).
    UsageEntry(crate::coding_agent::session_manager::UsageEntry),
    Message(AgentMessage),
    CostNotice(super::interactive_mode::CompactionCostNotice),
}

impl InteractiveMode {
    // =========================================================================
    // Selector mechanism (S3)
    // =========================================================================

    /// Upstream `disposeActiveSelector`: dispose the stored component, then
    /// clear the slot.
    pub fn dispose_active_selector(&self) {
        let dispose = self.lock().active_selector.clone();
        self.lock().active_selector = None;
        if let Some((_, Some(component))) = dispose {
            self.io
                .view
                .update_component(&component, "dispose", Value::Null);
        }
    }

    /// Upstream `showSelector`: swap the editor for a component, focus it, and
    /// restore on `done` ([`InteractiveMode::selector_done`]).
    pub fn show_selector(
        &self,
        component: &ComponentRef,
        focus: &ComponentRef,
        dispose: Option<ComponentRef>,
    ) -> SelectorToken {
        let token = SelectorToken(self.next_id());
        self.dispose_active_selector();
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, component);
        self.io
            .view
            .set_focus(FocusTarget::Component(focus.clone()));
        self.io.view.request_render(None);
        self.lock().active_selector = Some((token, dispose));
        token
    }

    /// The `done` closure of `showSelector`: dispose, then restore the editor
    /// when the closed selector is still active.
    pub fn selector_done(&self, token: SelectorToken) {
        let dispose = self.lock().active_selector.clone();
        if let Some((active, dispose_component)) = dispose {
            if let Some(component) = dispose_component {
                self.io
                    .view
                    .update_component(&component, "dispose", Value::Null);
            }
            if active != token {
                return;
            }
            self.lock().active_selector = None;
            self.restore_editor_in_container();
        }
    }

    /// `editorContainer.clear(); editorContainer.addChild(this.editor);
    /// ui.setFocus(this.editor);` — the shared restore.
    fn restore_editor_in_container(&self) {
        self.io.view.container_clear(ContainerId::EditorContainer);
        let editor = self.editor();
        let _ = editor;
        self.io.view.container_add_component(
            ContainerId::EditorContainer,
            &ComponentRef {
                kind: if self.lock().editor_is_custom {
                    "customEditor".to_string()
                } else {
                    "editor".to_string()
                },
                id: 0,
            },
        );
        self.io.view.set_focus(FocusTarget::Editor);
    }

    // =========================================================================
    // Loaded resources
    // =========================================================================

    /// Upstream `shouldShowStartupHeader` (v1.0.0): the startup header (logo,
    /// version, key hints) is hidden only by `quietStartup: true`.
    pub fn should_show_startup_header(&self) -> bool {
        self.options().verbose || self.io.settings.quiet_startup() != QuietStartup::Full
    }

    /// Upstream `shouldShowStartupDetails` (v1.0.0): startup details (model
    /// scope, loaded resources) are hidden by `quietStartup: true` or "header".
    pub fn should_show_startup_details(&self) -> bool {
        self.options().verbose || self.io.settings.quiet_startup() == QuietStartup::Off
    }

    /// Upstream `showLoadedResources`.
    pub fn show_loaded_resources(&self, force: bool, show_diagnostics_when_quiet: bool) {
        // Resource rendering is idempotent; chat clears no longer clear this
        // separate container.
        self.io.view.container_clear(ContainerId::LoadedResources);

        let show_listing = force || self.should_show_startup_details();
        let show_diagnostics = show_listing || show_diagnostics_when_quiet;
        if !show_listing && !show_diagnostics {
            return;
        }

        let resources = self.io.session.resources();
        let skills = resources.skills();
        let prompts = resources.prompts();
        let themes = resources.themes();
        let (extension_rows, extension_errors) = resources.extensions();
        // Upstream filters hidden extensions before the listing and before
        // the source-info map is built (interactive-mode.ts:1676).
        let extension_rows: Vec<super::interactive_mode::LoadedResource> = extension_rows
            .into_iter()
            .filter(|extension| !extension.hidden)
            .collect();
        let extensions: Vec<(String, Option<super::interactive_mode::SourceInfoView>)> =
            extension_rows
                .iter()
                .map(|e| (e.path.clone(), e.source_info.clone()))
                .collect();

        let mut source_infos: std::collections::HashMap<
            String,
            super::interactive_mode::SourceInfoView,
        > = std::collections::HashMap::new();
        for extension in &extension_rows {
            if let Some(source_info) = &extension.source_info {
                source_infos.insert(extension.path.clone(), source_info.clone());
            }
        }
        for skill in &skills.items {
            source_infos.insert(
                skill.path.clone(),
                skill.source_info.clone().unwrap_or_default(),
            );
        }
        for prompt in &prompts.items {
            source_infos.insert(
                prompt.path.clone(),
                prompt.source_info.clone().unwrap_or_default(),
            );
        }
        for loaded_theme in &themes.items {
            if let Some(source_path) = &loaded_theme.source_path {
                source_infos.insert(
                    source_path.clone(),
                    loaded_theme.source_info.clone().unwrap_or_default(),
                );
            }
        }

        let add_loaded_section = |shell: &Self, name: &str, collapsed: String, expanded: String| {
            let header = shell.fg("mdHeading", &format!("[{name}]"));
            let collapsed_text = format!("{header}\n{collapsed}");
            let expanded_text = format!("{header}\n{expanded}");
            // Upstream `new ExpandableText(..., this.getStartupExpansionState(),
            // 0, 0)` — the initial body follows the startup expansion state.
            shell.io.view.container_add_expandable_text(
                ContainerId::LoadedResources,
                &collapsed_text,
                &expanded_text,
                shell.get_startup_expansion_state(),
                0,
                0,
            );
            shell
                .io
                .view
                .container_add_spacer(ContainerId::LoadedResources);
        };

        if show_listing {
            let context_files: Vec<String> = resources
                .system_prompt_source()
                .into_iter()
                .chain(resources.append_system_prompt_sources())
                .chain(resources.agents_files())
                .map(|f| f.path)
                .collect();
            if !context_files.is_empty() {
                self.io
                    .view
                    .container_add_spacer(ContainerId::LoadedResources);
                let context_list = context_files
                    .iter()
                    .map(|p| {
                        self.fg(
                            "dim",
                            &format!(
                                "  {}",
                                super::interactive_mode::format_display_path(p, &self.io.home)
                            ),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let context_compact_list = self.fg(
                    "dim",
                    &format!(
                        "  {}",
                        context_files
                            .iter()
                            .map(|p| self.format_context_path(p))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                add_loaded_section(self, "Context", context_compact_list, context_list);
            }

            if !skills.items.is_empty() {
                let grouped: Vec<(String, Option<super::interactive_mode::SourceInfoView>)> =
                    skills
                        .items
                        .iter()
                        .map(|s| (s.path.clone(), s.source_info.clone()))
                        .collect();
                let groups = super::interactive_mode::build_scope_groups(&grouped);
                let skill_list = super::interactive_mode::format_scope_groups(
                    &groups,
                    &self.theme(),
                    |item| super::interactive_mode::format_display_path(&item.0, &self.io.home),
                    |item| {
                        super::interactive_mode::get_short_path(
                            &item.0,
                            item.1.as_ref(),
                            &self.io.home,
                        )
                    },
                );
                let skill_compact_list = self.fg(
                    "dim",
                    &format!(
                        "  {}",
                        skills
                            .items
                            .iter()
                            .filter_map(|s| s.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                add_loaded_section(self, "Skills", skill_compact_list, skill_list);
            }

            let templates = resources.prompt_templates();
            if !templates.is_empty() {
                let grouped: Vec<(String, Option<super::interactive_mode::SourceInfoView>)> =
                    templates
                        .iter()
                        .map(|t| (t.path.clone(), t.source_info.clone()))
                        .collect();
                let groups = super::interactive_mode::build_scope_groups(&grouped);
                let template_by_path: std::collections::HashMap<&str, &str> = templates
                    .iter()
                    .filter_map(|t| t.name.as_ref().map(|n| (t.path.as_str(), n.as_str())))
                    .collect();
                let template_list = super::interactive_mode::format_scope_groups(
                    &groups,
                    &self.theme(),
                    |item| match template_by_path.get(item.0.as_str()) {
                        Some(name) => format!("/{name}"),
                        None => {
                            super::interactive_mode::format_display_path(&item.0, &self.io.home)
                        }
                    },
                    |item| match template_by_path.get(item.0.as_str()) {
                        Some(name) => format!("/{name}"),
                        None => {
                            super::interactive_mode::format_display_path(&item.0, &self.io.home)
                        }
                    },
                );
                let prompt_compact_list = self.fg(
                    "dim",
                    &format!(
                        "  {}",
                        templates
                            .iter()
                            .filter_map(|t| t.name.as_ref().map(|n| format!("/{n}")))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                add_loaded_section(self, "Prompts", prompt_compact_list, template_list);
            }

            if !extensions.is_empty() {
                let groups = super::interactive_mode::build_scope_groups(&extensions);
                let ext_list = super::interactive_mode::format_scope_groups(
                    &groups,
                    &self.theme(),
                    |item| {
                        super::interactive_mode::format_extension_display_path(
                            &item.0,
                            &self.io.home,
                        )
                    },
                    |item| {
                        super::interactive_mode::format_extension_display_path(
                            &super::interactive_mode::get_short_path(
                                &item.0,
                                item.1.as_ref(),
                                &self.io.home,
                            ),
                            &self.io.home,
                        )
                    },
                );
                let extension_compact_list = self.fg(
                    "dim",
                    &format!(
                        "  {}",
                        get_compact_extension_labels(&extensions, &self.io.home).join(", ")
                    ),
                );
                add_loaded_section(self, "Extensions", extension_compact_list, ext_list);
            }

            let custom_themes: Vec<&super::interactive_mode::LoadedResource> = themes
                .items
                .iter()
                .filter(|t| t.source_path.is_some())
                .collect();
            if !custom_themes.is_empty() {
                let grouped: Vec<(String, Option<super::interactive_mode::SourceInfoView>)> =
                    custom_themes
                        .iter()
                        .map(|t| {
                            (
                                t.source_path.clone().unwrap_or_default(),
                                t.source_info.clone(),
                            )
                        })
                        .collect();
                let groups = super::interactive_mode::build_scope_groups(&grouped);
                let theme_list = super::interactive_mode::format_scope_groups(
                    &groups,
                    &self.theme(),
                    |item| super::interactive_mode::format_display_path(&item.0, &self.io.home),
                    |item| {
                        super::interactive_mode::get_short_path(
                            &item.0,
                            item.1.as_ref(),
                            &self.io.home,
                        )
                    },
                );
                let theme_compact_list = self.fg(
                    "dim",
                    &format!(
                        "  {}",
                        custom_themes
                            .iter()
                            .map(|t| {
                                t.name.clone().unwrap_or_else(|| {
                                    super::interactive_mode::get_compact_path_label(
                                        t.source_path.as_deref().unwrap_or_default(),
                                        t.source_info.as_ref(),
                                        &self.io.home,
                                    )
                                })
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                add_loaded_section(self, "Themes", theme_compact_list, theme_list);
            }
        }

        if show_diagnostics {
            let mut sections: Vec<(&str, Vec<super::interactive_mode::ResourceDiagnostic>)> =
                Vec::new();
            if !skills.diagnostics.is_empty() {
                sections.push(("Skill conflicts", skills.diagnostics.clone()));
            }
            if !prompts.diagnostics.is_empty() {
                sections.push(("Prompt conflicts", prompts.diagnostics.clone()));
            }
            let mut extension_diagnostics: Vec<super::interactive_mode::ResourceDiagnostic> =
                extension_errors
                    .iter()
                    .map(
                        |(path, error)| super::interactive_mode::ResourceDiagnostic {
                            kind: super::interactive_mode::DiagnosticKind::Error,
                            message: error.clone(),
                            path: Some(path.clone()),
                            collision: None,
                        },
                    )
                    .collect();
            extension_diagnostics.extend(self.io.session.extensions().command_diagnostics());
            extension_diagnostics.extend(
                super::interactive_mode::get_built_in_command_conflict_diagnostics(
                    &self.io.session.extensions().registered_commands(),
                ),
            );
            extension_diagnostics.extend(self.io.session.extensions().shortcut_diagnostics());
            if !extension_diagnostics.is_empty() {
                sections.push(("Extension issues", extension_diagnostics));
            }
            if !themes.diagnostics.is_empty() {
                sections.push(("Theme conflicts", themes.diagnostics.clone()));
            }

            for (title, diagnostics) in sections {
                let warning_lines = super::interactive_mode::format_diagnostics(
                    &diagnostics,
                    &source_infos,
                    &self.io.home,
                    &self.theme(),
                );
                self.io.view.container_add_text(
                    ContainerId::LoadedResources,
                    &format!(
                        "{}\n{warning_lines}",
                        self.fg("warning", &format!("[{title}]"))
                    ),
                    0,
                    0,
                    false,
                );
                self.io
                    .view
                    .container_add_spacer(ContainerId::LoadedResources);
            }
        }
    }

    /// Upstream `formatContextPath` (interactive-mode.ts:1292): resolve the
    /// session cwd and `p` with node `path.resolve`, prefer
    /// `getCwdRelativePath`, fall back to `formatDisplayPath` of the resolved
    /// absolute. Both oracle harnesses stub `getCwdRelativePath`
    /// (drive_shell.ts) with a resolved-cwd prefix strip
    /// (`p.startsWith(cwd) ? p.slice(cwd.length + 1) : undefined`), which is
    /// the recorded authority for the shell.
    pub fn format_context_path(&self, p: &str) -> String {
        use crate::coding_agent::utils::paths::{resolve_path_with, PathInputOptions};
        let windows = cfg!(windows);
        let resolve = |input: &str, base: &str| {
            resolve_path_with(input, base, &PathInputOptions::default(), windows)
                .unwrap_or_else(|_| input.to_string())
        };
        let cwd = self.io.session_manager.cwd();
        let resolved_cwd = resolve(&cwd, "");
        let absolute = resolve(p, &cwd);
        if absolute.starts_with(&resolved_cwd) {
            // JS `slice(cwd.length + 1)`: drop the prefix plus the separator;
            // an exact match yields "" (out-of-range slice).
            return absolute
                .get(resolved_cwd.len() + 1..)
                .map(str::to_string)
                .unwrap_or_default();
        }
        super::interactive_mode::format_display_path(&absolute, &self.io.home)
    }

    // =========================================================================
    // Settings selector
    // =========================================================================

    /// Upstream `showSettingsSelector` (config projection + callback wiring).
    pub fn show_settings_selector(&self) {
        let default_provider = self.io.settings.default_provider();
        let default_model = self.io.settings.default_model();
        let default_model_line = match (default_provider, default_model) {
            (Some(provider), Some(model)) => format!("{provider}/{model}"),
            _ => "not set".to_string(),
        };
        let current_model = self.io.session.model();
        let config = json!({
            "autoCompact": self.io.session.auto_compaction_enabled(),
            "defaultModel": default_model_line,
            "currentModel": current_model.as_ref().map(|m| m.to_value()).unwrap_or(json!("undefined")),
            "availableDefaultModels": self.io.session.model_runtime().available_snapshot()
                .iter().map(|m| m.to_value()).collect::<Vec<_>>(),
            "showImages": self.io.settings.show_images(),
            "imageWidthCells": self.io.settings.image_width_cells(),
            "autoResizeImages": self.io.settings.image_auto_resize(),
            "blockImages": self.io.settings.block_images(),
            "enableSkillCommands": self.io.settings.enable_skill_commands(),
            "steeringMode": self.io.session.steering_mode(),
            "followUpMode": self.io.session.follow_up_mode(),
            "transport": self.io.settings.transport(),
            "httpIdleTimeoutMs": self.io.settings.http_idle_timeout_ms().map(Value::from).unwrap_or(Value::Null),
            "thinkingLevel": self.io.settings.default_thinking_level().map(|l| thinking_level_lower(&l)).unwrap_or_else(|| thinking_level_lower(&DEFAULT_THINKING_LEVEL)),
            "availableThinkingLevels": THINKING_LEVEL_OPTIONS.iter().map(thinking_level_lower).collect::<Vec<_>>(),
            "modelThinkingLevels": {},
            "currentTheme": "dark",
            "terminalTheme": "dark",
            "availableThemes": ["dark", "light"],
            "hideThinkingBlock": self.lock().hide_thinking_block,
            "mermaidRenderingMode": self.io.settings.mermaid_rendering_mode(),
            "collapseChangelog": self.io.settings.collapse_changelog(),
            "enableInstallTelemetry": self.io.settings.enable_install_telemetry(),
            "doubleEscapeAction": self.io.settings.double_escape_action(),
            "treeFilterMode": self.io.settings.tree_filter_mode(),
            "showHardwareCursor": self.io.settings.show_hardware_cursor(),
            "showCacheMissNotices": self.io.settings.show_cache_miss_notices(),
            "defaultProjectTrust": self.io.settings.default_project_trust(),
            "editorPaddingX": self.io.settings.editor_padding_x(),
            "outputPad": self.io.settings.output_pad(),
            "autocompleteMaxVisible": self.io.settings.autocomplete_max_visible(),
            "quietStartup": match self.io.settings.quiet_startup() {
                QuietStartup::Full => Value::Bool(true),
                QuietStartup::Header => Value::String("header".into()),
                QuietStartup::Off => Value::Bool(false),
            },
            "clearOnShrink": self.io.settings.clear_on_shrink(),
            "showTerminalProgress": self.io.settings.show_terminal_progress(),
            "tuiMode": self.io.view.renderer_mode(),
            "fullscreenExitOutput": self.io.settings.fullscreen_exit_output(),
            "fullscreenScrollbar": self.io.settings.fullscreen_scrollbar(),
            "fullscreenCopyOnSelect": self.io.settings.fullscreen_copy_on_select(),
            "warnings": { "anthropicExtraUsage": self.io.settings.warnings_anthropic_extra_usage() },
        });
        // The full callback map the settings component receives (every member
        // is a function; the describe renders each as `"function"`).
        let callbacks = json!({
            "onAutoCompactChange": "function", "onShowImagesChange": "function",
            "onImageWidthCellsChange": "function", "onAutoResizeImagesChange": "function",
            "onBlockImagesChange": "function", "onEnableSkillCommandsChange": "function",
            "onSteeringModeChange": "function", "onFollowUpModeChange": "function",
            "onTransportChange": "function", "onHttpIdleTimeoutMsChange": "function",
            "onModelThinkingLevelChange": "function", "onModelThinkingLevelRemove": "function",
            "onThemeChange": "function", "onThemePreview": "function",
            "onHideThinkingBlockChange": "function", "onMermaidRenderingModeChange": "function",
            "onShowCacheMissNoticesChange": "function", "onCollapseChangelogChange": "function",
            "onEnableInstallTelemetryChange": "function", "onQuietStartupChange": "function",
            "onDefaultProjectTrustChange": "function", "onDoubleEscapeActionChange": "function",
            "onTreeFilterModeChange": "function", "onShowHardwareCursorChange": "function",
            "onEditorPaddingXChange": "function", "onOutputPadChange": "function",
            "onAutocompleteMaxVisibleChange": "function", "onClearOnShrinkChange": "function",
            "onShowTerminalProgressChange": "function", "onTuiModeChange": "function",
            "onFullscreenExitOutputChange": "function", "onFullscreenScrollbarChange": "function",
            "onFullscreenCopyOnSelectChange": "function", "onWarningsChange": "function",
            "onCancel": "function",
        });
        let component = self
            .io
            .view
            .new_component(ComponentKind::SettingsSelector, json!([config, callbacks]));
        let focus = ComponentRef {
            kind: "SettingsSelectorList".to_string(),
            id: component.id,
        };
        self.show_selector(&component, &focus, None);
        self.lock().settings_selector = Some(component);
    }

    /// The settings selector callbacks (upstream closure bodies), dispatched
    /// by name.
    pub async fn settings_callback(&self, name: &str, value: Value) {
        match name {
            "onAutoCompactChange" => {
                let enabled = value.as_bool().unwrap_or(false);
                self.io.session.set_auto_compaction_enabled(enabled);
                self.ev(json!(["footer.setAutoCompactEnabled", enabled]));
            }
            "onShowImagesChange" => {
                let enabled = value.as_bool().unwrap_or(false);
                self.io.settings.set("setShowImages", Value::Bool(enabled));
                for component in self.io.view.container_components(ContainerId::Chat) {
                    if component.kind == "ToolExecutionComponent" {
                        self.io.view.update_component(
                            &component,
                            "setShowImages",
                            Value::Bool(enabled),
                        );
                    }
                }
            }
            "onImageWidthCellsChange" => {
                self.io.settings.set("setImageWidthCells", value.clone());
                for component in self.io.view.container_components(ContainerId::Chat) {
                    if component.kind == "ToolExecutionComponent" {
                        self.io.view.update_component(
                            &component,
                            "setImageWidthCells",
                            value.clone(),
                        );
                    }
                }
            }
            "onAutoResizeImagesChange" => {
                self.io.settings.set("setImageAutoResize", value);
            }
            "onBlockImagesChange" => {
                self.io.settings.set("setBlockImages", value);
            }
            "onEnableSkillCommandsChange" => {
                self.io.settings.set("setEnableSkillCommands", value);
                self.setup_autocomplete_provider();
            }
            "onSteeringModeChange" => {
                self.io.session.set_steering_mode(value);
            }
            "onFollowUpModeChange" => {
                self.io.session.set_follow_up_mode(value);
            }
            "onTransportChange" => {
                self.io.settings.set("setTransport", value);
            }
            "onHttpIdleTimeoutMsChange" => {
                self.io.settings.set("setHttpIdleTimeoutMs", value.clone());
                self.ev(json!([
                    "configureHttpDispatcher",
                    value.as_u64().map(Value::from).unwrap_or(Value::Null)
                ]));
                self.show_status(&format!(
                    "HTTP idle timeout: {}",
                    format_http_idle_timeout_ms(value.as_u64())
                ));
            }
            "onHideThinkingBlockChange" => {
                let hidden = value.as_bool().unwrap_or(false);
                self.lock().hide_thinking_block = hidden;
                self.io
                    .settings
                    .set("setHideThinkingBlock", Value::Bool(hidden));
                self.update_thinking_block_visibility();
            }
            "onShowCacheMissNoticesChange" => {
                self.io.settings.set("setShowCacheMissNotices", value);
                self.rebuild_chat_from_messages();
            }
            "onOutputPadChange" => {
                let padding = value.as_i64().unwrap_or(0);
                self.io.settings.set("setOutputPad", Value::from(padding));
                self.lock().output_pad = padding;
                let has_streaming = self.lock().streaming_component.is_some();
                if has_streaming || self.io.session.is_streaming() {
                    for component in self.io.view.container_components(ContainerId::Chat) {
                        if matches!(
                            component.kind.as_str(),
                            "AssistantMessageComponent"
                                | "CustomMessageComponent"
                                | "UserMessageComponent"
                        ) {
                            self.io.view.update_component(
                                &component,
                                "setOutputPad",
                                Value::from(padding),
                            );
                        }
                    }
                    if let Some(streaming) = self.lock().streaming_component.clone() {
                        self.io.view.update_component(
                            &streaming,
                            "setOutputPad",
                            Value::from(padding),
                        );
                    }
                    self.io.view.request_render(None);
                    return;
                }
                self.rebuild_chat_from_messages();
            }
            "onClearOnShrinkChange" => {
                let enabled = value.as_bool().unwrap_or(false);
                self.io
                    .settings
                    .set("setClearOnShrink", Value::Bool(enabled));
                self.io.view.set_clear_on_shrink(enabled);
                if !enabled && self.lock().active_status_indicator.is_none() {
                    self.io.view.container_clear(ContainerId::Status);
                }
            }
            "onShowHardwareCursorChange" => {
                let enabled = value.as_bool().unwrap_or(false);
                self.io
                    .settings
                    .set("setShowHardwareCursor", Value::Bool(enabled));
                self.ev(json!(["ui.setShowHardwareCursor", enabled]));
            }
            "onTuiModeChange" => {
                let mode = value.as_str().unwrap_or_default().to_string();
                if !self.switch_tui_mode(&mode, true, true) {
                    let mode_now = self.io.view.renderer_mode();
                    self.ev(json!([
                        "SettingsSelectorList.updateValue",
                        "tui-mode",
                        mode_now
                    ]));
                    self.show_status("Close active overlays before changing TUI mode");
                    return;
                }
                self.io
                    .settings
                    .set("setTuiMode", Value::String(mode.clone()));
                if self.lock().active_status_indicator.is_none() {
                    self.io.view.container_clear(ContainerId::Status);
                }
                self.show_status(&format!("TUI mode: {mode}"));
            }
            "onQuietStartupChange" => {
                self.io.settings.set("setQuietStartup", value);
            }
            "onCancel" => {
                let selector = self.lock().settings_selector.clone();
                if let Some(component) = selector {
                    self.selector_done(SelectorToken(component.id));
                    self.lock().settings_selector = None;
                }
                self.io.view.request_render(None);
            }
            _ => {
                self.io.settings.set(name, value);
            }
        }
    }

    // =========================================================================
    // Autocomplete / extension shortcuts
    // =========================================================================

    /// Upstream `setupAutocompleteProvider`.
    pub fn setup_autocomplete_provider(&self) {
        self.create_base_autocomplete_provider();
        self.io.default_editor.set_autocomplete_provider();
        if self.lock().editor_is_custom {
            self.editor().set_autocomplete_provider();
        }
    }

    /// Upstream `createBaseAutocompleteProvider` (command-list assembly; the
    /// CombinedAutocompleteProvider itself is the pi-tui seam). The command
    /// entries mirror the harness `BUILTIN_SLASH_COMMANDS` projection: name,
    /// description, the model's `argumentHint`, and the argument-completion
    /// hooks on model/thinking.
    pub fn create_base_autocomplete_provider(&self) -> Vec<Value> {
        let mut commands: Vec<Value> = Vec::new();
        for name in super::interactive_mode::BUILTIN_SLASH_COMMAND_NAMES {
            match name {
                "model" => {
                    commands.push(json!({
                        "name": "model",
                        "description": "Switch AI model",
                        "argumentHint": "[term]",
                        "getArgumentCompletions": "function",
                    }));
                }
                "thinking" => {
                    commands.push(json!({
                        "name": "thinking",
                        "description": "Set thinking level",
                        "getArgumentCompletions": "function",
                    }));
                }
                other => {
                    commands.push(json!({
                        "name": other,
                        "description": match other {
                            "settings" => "Open settings",
                            _ => other,
                        },
                    }));
                }
            }
        }
        // Prompt templates and skills flow through the same list.
        self.lock().skill_commands.clear();
        if self.io.settings.enable_skill_commands() {
            let resources = self.io.session.resources();
            for skill in &resources.skills().items {
                let Some(name) = &skill.name else { continue };
                self.lock()
                    .skill_commands
                    .insert(format!("skill:{name}"), skill.path.clone());
                commands.push(json!({
                    "name": format!("skill:{name}"),
                }));
            }
        }
        self.ev(json!([
            "new CombinedAutocompleteProvider",
            commands,
            self.io.session_manager.cwd(),
            self.lock()
                .fd_path
                .clone()
                .map(Value::from)
                .unwrap_or(json!("undefined"))
        ]));
        commands
    }

    /// Upstream `setupExtensionShortcuts` — builds the extension shortcut
    /// context; the wiring itself is silent in the recording vocabulary.
    pub fn setup_extension_shortcuts(&self) {
        let shortcuts = self.io.session.shortcuts().shortcuts();
        if shortcuts.is_empty() {
            return;
        }
        self.io.default_editor.set_on_extension_shortcut(true);
    }

    /// The `defaultEditor.onExtensionShortcut` body: match `data` against the
    /// registered shortcut keys and invoke the matching extension handler
    /// (extension-owned; the port records nothing itself).
    pub fn on_extension_shortcut(&self, data: &str) -> bool {
        let shortcuts = self.io.session.shortcuts().shortcuts();
        for shortcut in shortcuts {
            if data == shortcut.key {
                return true;
            }
        }
        false
    }

    // =========================================================================
    // Thinking command
    // =========================================================================

    /// Upstream `handleThinkingCommand`.
    pub fn handle_thinking_command(&self, search_term: Option<&str>) {
        let available_levels = self.io.session.available_thinking_levels();
        let Some(search_term) = search_term else {
            self.show_thinking_selector();
            return;
        };
        let normalized = search_term.trim().to_lowercase();
        let level = available_levels
            .iter()
            .find(|c| thinking_level_lower(c) == normalized);
        let Some(level) = level else {
            let joined = available_levels
                .iter()
                .map(thinking_level_lower)
                .collect::<Vec<_>>()
                .join(", ");
            self.show_error(&format!(
                "Unknown thinking level \"{search_term}\". Available levels: {joined}."
            ));
            return;
        };
        self.select_thinking_level(*level, false);
    }

    /// Upstream `selectThinkingLevel`.
    pub fn select_thinking_level(&self, level: ThinkingLevel, persist: bool) {
        match self.io.session.set_thinking_level(level, persist) {
            Ok(()) => {
                self.ev(json!(["footer.invalidate"]));
                self.update_editor_border_color();
                self.show_status(&format!(
                    "{}: {}",
                    if persist {
                        "Default thinking level"
                    } else {
                        "Thinking level"
                    },
                    thinking_level_lower(&level)
                ));
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// Upstream `showThinkingSelector`.
    pub fn show_thinking_selector(&self) {
        let current = self.io.session.thinking_level();
        let available = self.io.session.available_thinking_levels();
        let default_level = self
            .io
            .settings
            .default_thinking_level()
            .unwrap_or(DEFAULT_THINKING_LEVEL);
        let args = json!([
            thinking_level_lower(&current),
            available
                .iter()
                .map(thinking_level_lower)
                .collect::<Vec<_>>(),
            "function",
            "function",
            "function",
            thinking_level_lower(&default_level),
        ]);
        let component = self
            .io
            .view
            .new_component(ComponentKind::ThinkingSelector, args);
        self.show_selector(&component, &component, None);
        self.lock().thinking_selector = Some(component);
    }

    /// The thinking selector `onSelect`/`onPersist`/`onCancel`.
    pub fn thinking_selector_callback(&self, kind: &str, level: Option<ThinkingLevel>) {
        let selector = self.lock().thinking_selector.clone();
        let token = selector.map(|c| SelectorToken(c.id));
        match (kind, level) {
            ("onSelect", Some(level)) => {
                self.select_thinking_level(level, false);
                if let Some(token) = token {
                    self.selector_done(token);
                }
                self.lock().thinking_selector = None;
            }
            ("onPersist", Some(level)) => {
                self.select_thinking_level(level, true);
                if let Some(token) = token {
                    self.selector_done(token);
                }
                self.lock().thinking_selector = None;
            }
            _ => {
                if let Some(token) = token {
                    self.selector_done(token);
                }
                self.io.view.request_render(None);
                self.lock().thinking_selector = None;
            }
        }
    }

    // =========================================================================
    // Model command + selectors
    // =========================================================================

    /// Upstream `findExactModelReferenceMatch` over one snapshot.
    fn find_exact_match(reference: &str, models: &[ModelRef]) -> Option<ModelRef> {
        let normalized = reference.trim().to_lowercase();
        if normalized.is_empty() {
            return None;
        }
        let canonical: Vec<&ModelRef> = models
            .iter()
            .filter(|m| m.reference().to_lowercase() == normalized)
            .collect();
        if canonical.len() == 1 {
            return canonical.into_iter().next().cloned();
        }
        if let Some((provider, model_id)) = normalized.split_once('/') {
            if !provider.is_empty() && !model_id.is_empty() {
                let matches: Vec<&ModelRef> = models
                    .iter()
                    .filter(|m| {
                        m.provider.to_lowercase() == provider && m.id.to_lowercase() == model_id
                    })
                    .collect();
                if matches.len() == 1 {
                    return matches.into_iter().next().cloned();
                }
            }
        }
        let by_id: Vec<&ModelRef> = models
            .iter()
            .filter(|m| m.id.to_lowercase() == normalized)
            .collect();
        if by_id.len() == 1 {
            return by_id.into_iter().next().cloned();
        }
        None
    }

    /// Upstream `findExactModelMatch`.
    pub async fn find_exact_model_match(&self, search_term: &str) -> Option<ModelRef> {
        let scoped = self.io.session.scoped_models();
        let cached_models: Vec<ModelRef> = if !scoped.is_empty() {
            scoped.iter().map(model_ref_of_scoped).collect()
        } else {
            self.io.session.model_runtime().available_snapshot()
        };
        let cached_match = Self::find_exact_match(search_term, &cached_models);
        if cached_match.is_some() || !scoped.is_empty() {
            return cached_match;
        }

        self.show_status("Refreshing model catalogs…");
        let result = self.io.session.model_runtime().refresh(None).await;
        if result.aborted {
            self.show_warning("Model refresh timed out; searching cached models.");
        } else if !result.errors.is_empty() {
            self.show_warning(&format!(
                "Could not refresh {}; searching cached models.",
                result.errors.join(", ")
            ));
        }
        Self::find_exact_match(
            search_term,
            &self.io.session.model_runtime().available_snapshot(),
        )
    }

    /// Upstream `handleModelCommand`.
    pub async fn handle_model_command(&self, search_term: Option<&str>) {
        let Some(search_term) = search_term else {
            self.show_model_selector(None);
            return;
        };
        if let Some(model) = self.find_exact_model_match(search_term).await {
            if let Err(error) = self.io.session.set_model(&model, false).await {
                self.show_error(&error);
                return;
            }
            self.ev(json!(["footer.invalidate"]));
            self.update_editor_border_color();
            self.show_status(&format!("Model: {}", model.id));
            self.maybe_warn_about_anthropic_subscription_auth(Some(&model))
                .await;
            return;
        }
        self.show_model_selector(Some(search_term));
    }

    /// Upstream `showModelSelector`. The ctor projection mirrors the harness
    /// describe: the ui handle, the current model (`"undefined"` when unset),
    /// the model-runtime collaborator describe, the scoped models, three
    /// callbacks, the initial search input, another callback, and the
    /// default model reference.
    pub fn show_model_selector(&self, initial_search_input: Option<&str>) {
        let args = json!([
            { "__describe": "ui" },
            self.io
                .session
                .model()
                .as_ref()
                .map(|m| m.to_value())
                .unwrap_or(json!("undefined")),
            {
                "getAvailableSnapshot": "function", "getError": "function",
                "getProviders": "function", "getProviderAuthStatus": "function",
                "isUsingOAuth": "function", "checkAuth": "function",
                "getAuth": "function", "listCredentials": "function",
                "logout": "function", "login": "function", "refresh": "function",
            },
            self.io
                .session
                .scoped_models()
                .iter()
                .map(|s| json!({
                    "model": {
                        "provider": s.model.provider,
                        "id": s.model.id,
                        "name": s.model.name,
                        "reasoning": s.model.reasoning,
                    },
                    "thinkingLevel": s.thinking_level,
                }))
                .collect::<Vec<_>>(),
            "function",
            "function",
            initial_search_input.map(Value::from).unwrap_or(json!("undefined")),
            "function",
            match (
                self.io.settings.default_provider(),
                self.io.settings.default_model()
            ) {
                (Some(provider), Some(model)) => json!({"provider": provider, "id": model}),
                _ => Value::Null,
            },
        ]);
        let component = self
            .io
            .view
            .new_component(ComponentKind::ModelSelector, args);
        self.show_selector(&component, &component, Some(component.clone()));
        self.lock().model_selector = Some(component);
    }

    /// The model selector `selectModel` closure.
    pub async fn model_selector_select(&self, model: &ModelRef, persist: bool) {
        let token = self
            .lock()
            .model_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        match self.io.session.set_model(model, persist).await {
            Ok(()) => {
                self.update_available_provider_count();
                self.ev(json!(["footer.invalidate"]));
                self.update_editor_border_color();
                if let Some(token) = token {
                    self.selector_done(token);
                }
                let status = if persist {
                    format!("Default model: {}/{}", model.provider, model.id)
                } else {
                    format!("Model: {}", model.id)
                };
                self.show_status(&status);
                self.maybe_warn_about_anthropic_subscription_auth(Some(model))
                    .await;
            }
            Err(error) => {
                if let Some(token) = token {
                    self.selector_done(token);
                }
                self.show_error(&error);
            }
        }
        self.lock().model_selector = None;
    }

    /// The model selector cancel.
    pub fn model_selector_cancel(&self) {
        let token = self
            .lock()
            .model_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.io.view.request_render(None);
        self.lock().model_selector = None;
    }

    /// Upstream `showModelsSelector` (scoped-models wiring).
    pub async fn show_models_selector(&self) {
        let available = self.io.session.model_runtime().available_snapshot();
        let configured_patterns = self.io.settings.enabled_models();
        let session_scoped = self.io.session.scoped_models();
        let configured_enabled_ids = |models: &[ModelRef]| -> Option<Vec<String>> {
            let patterns = configured_patterns.as_ref()?;
            let mut ids: Vec<String> = models
                .iter()
                .filter(|m| patterns.iter().any(|p| *p == m.reference() || *p == m.id))
                .map(|m| m.reference())
                .collect();
            for pattern in patterns {
                if !ids.iter().any(|id| id == pattern)
                    && !models
                        .iter()
                        .any(|m| *pattern == m.reference() || *pattern == m.id)
                {
                    ids.push(pattern.clone());
                }
            }
            Some(ids)
        };
        let current_enabled_ids = if !session_scoped.is_empty() {
            Some(
                session_scoped
                    .iter()
                    .map(|s| model_ref_of_scoped(s).reference())
                    .collect::<Vec<_>>(),
            )
        } else {
            configured_enabled_ids(&available)
        };

        let component = self.io.view.new_component(
            ComponentKind::ScopedModelsSelector,
            json!([{
                "allModels": available.iter().map(|m| m.to_value()).collect::<Vec<_>>(),
                "enabledModelIds": current_enabled_ids.clone().map(|ids| json!(ids)).unwrap_or(Value::Null),
                "refreshStatus": "Refreshing model catalogs…",
            }, {
                "onChange": "function", "onPersist": "function", "onCancel": "function",
            }]),
        );

        // Upstream refreshes through the model-catalog coordinator before
        // mounting the selector. The scoped-models selector registers no
        // dispose component.
        let refreshed = self.io.session.model_runtime().refresh(None).await;
        let available = self.io.session.model_runtime().available_snapshot();
        self.show_selector(&component, &component, None);
        self.io.view.update_component(
            &component,
            "updateModels",
            json!([
                available.iter().map(|m| m.to_value()).collect::<Vec<_>>(),
                current_enabled_ids
                    .clone()
                    .map(|ids| json!(ids))
                    .unwrap_or(Value::Null),
            ]),
        );
        if !refreshed.aborted && refreshed.errors.is_empty() {
            self.io.view.update_component(
                &component,
                "setRefreshStatus",
                json!(["Model catalogs refreshed.", "success"]),
            );
        }
        self.io.view.request_render(None);
        if let Some(ids) = &current_enabled_ids {
            self.update_session_models(&available, Some(ids.clone()));
        }

        // The selector stays open; selection updates flow through
        // `models_selector_change` until cancel.
        self.lock().models_selector_models = Some(available);
        self.lock().models_selector_enabled = current_enabled_ids;
        self.lock().models_selector = Some(component);
    }

    /// The scoped-models `updateSessionModels` closure.
    fn update_session_models(&self, available: &[ModelRef], enabled_ids: Option<Vec<String>>) {
        let available_ids: Vec<String> = available.iter().map(|m| m.reference()).collect();
        let has_enabled_available_model = enabled_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| available_ids.contains(id)));
        let all_available_models_enabled = enabled_ids
            .as_ref()
            .is_some_and(|ids| available_ids.iter().all(|id| ids.contains(id)));
        if let Some(ids) = &enabled_ids {
            if has_enabled_available_model && !all_available_models_enabled {
                let scoped: Vec<ModelRef> = available
                    .iter()
                    .filter(|m| ids.contains(&m.reference()))
                    .cloned()
                    .collect();
                self.io.session.set_scoped_models(&scoped);
            } else {
                self.io.session.set_scoped_models(&[]);
            }
        } else {
            self.io.session.set_scoped_models(&[]);
        }
        self.update_available_provider_count();
        self.io.view.request_render(None);
    }

    /// The scoped-models selector `onChange`.
    pub async fn models_selector_change(&self, enabled_ids: Option<Vec<String>>) {
        self.lock().models_selector_enabled = enabled_ids.clone();
        let available = self
            .lock()
            .models_selector_models
            .clone()
            .unwrap_or_default();
        self.update_session_models(&available, enabled_ids);
    }

    /// The scoped-models selector `onPersist`.
    pub fn models_selector_persist(&self, enabled_ids: Option<Vec<String>>) {
        let available = self
            .lock()
            .models_selector_models
            .clone()
            .unwrap_or_default();
        let available_ids: Vec<String> = available.iter().map(|m| m.reference()).collect();
        let all_enabled = enabled_ids.as_ref().is_some_and(|ids| {
            ids.len() == available.len() && ids.iter().all(|id| available_ids.contains(id))
        });
        let new_patterns = if enabled_ids.is_none() || all_enabled {
            None
        } else {
            enabled_ids
        };
        self.io.settings.set(
            "setEnabledModels",
            new_patterns.map(Value::from).unwrap_or(Value::Null),
        );
        self.show_status("Model selection saved to settings");
    }

    /// The scoped-models selector cancel.
    pub fn models_selector_cancel(&self) {
        let token = self
            .lock()
            .models_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.io.view.request_render(None);
        self.lock().model_selector = None;
        self.lock().models_selector_models = None;
        self.lock().models_selector_enabled = None;
        self.lock().models_selector = None;
    }

    // =========================================================================
    // Fork / clone / tree / resume
    // =========================================================================

    /// Upstream `showUserMessageSelector`.
    pub fn show_user_message_selector(&self) {
        let user_messages = self.io.session.user_messages_for_forking();
        if user_messages.is_empty() {
            self.show_status("No messages to fork from");
            return;
        }
        let initial_selected_id = user_messages.last().map(|m| m.entry_id.clone());
        let component = self.io.view.new_component(
            ComponentKind::UserMessageSelector,
            json!([
                user_messages
                    .iter()
                    .map(|m| json!({"id": m.entry_id, "text": m.text}))
                    .collect::<Vec<_>>(),
                "function",
                "function",
                initial_selected_id
                    .map(Value::from)
                    .unwrap_or(json!("undefined")),
            ]),
        );
        // Upstream focuses `selector.getMessageList()`.
        let focus = ComponentRef {
            kind: "MessageList".to_string(),
            id: component.id,
        };
        self.show_selector(&component, &focus, None);
        self.lock().user_message_selector = Some(component);
    }

    /// The user-message selector `onSelect` (fork).
    pub async fn user_message_selector_select(&self, entry_id: &str) {
        let token = self
            .lock()
            .user_message_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.lock().user_message_selector = None;
        match self.io.host.fork(entry_id, Value::Null).await {
            Ok(result) => {
                if result.cancelled {
                    self.io.view.request_render(None);
                    return;
                }
                self.editor()
                    .set_text(result.selected_text.as_deref().unwrap_or(""));
                self.show_status("Forked to new session");
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// The user-message selector cancel.
    pub fn user_message_selector_cancel(&self) {
        let token = self
            .lock()
            .user_message_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.io.view.request_render(None);
        self.lock().user_message_selector = None;
    }

    /// Upstream `handleCloneCommand`.
    pub async fn handle_clone_command(&self) {
        let Some(leaf_id) = self.io.session_manager.leaf_id() else {
            self.show_status("Nothing to clone yet");
            return;
        };
        match self
            .io
            .host
            .fork(&leaf_id, json!({ "position": "at" }))
            .await
        {
            Ok(result) => {
                if result.cancelled {
                    self.io.view.request_render(None);
                    return;
                }
                self.editor().set_text("");
                self.show_status("Cloned to new session");
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// Upstream `showTreeSelector`.
    pub fn show_tree_selector(&self, initial_selected_id: Option<&str>) {
        let tree = self.io.session_manager.tree();
        if tree.is_empty() {
            self.show_status("No entries in session");
            return;
        }
        let real_leaf_id = self.io.session_manager.leaf_id();
        let initial_filter_mode = self.io.settings.tree_filter_mode();
        let component = self.io.view.new_component(
            ComponentKind::TreeSelector,
            json!([
                tree.iter()
                    .map(|(id, ty)| json!({"id": id, "type": ty}))
                    .collect::<Vec<_>>(),
                real_leaf_id.map(Value::from).unwrap_or(json!("undefined")),
                24,
                "function",
                "function",
                "function",
                initial_selected_id
                    .map(Value::from)
                    .unwrap_or(json!("undefined")),
                initial_filter_mode,
            ]),
        );
        self.show_selector(&component, &component, None);
        self.lock().tree_selector = Some(component);
    }

    /// The tree selector `onSelect` navigation ladder.
    pub async fn tree_selector_select(&self, entry_id: &str) {
        if Some(entry_id) == self.io.session_manager.leaf_id().as_deref() {
            self.show_status("Already at this point");
            return;
        }

        // Ask about summarization (loop until complete choice or escape).
        let mut wants_summary = false;
        let mut custom_instructions: Option<String> = None;
        if !self.io.settings.branch_summary_skip_prompt() {
            loop {
                let summary_choice = self
                    .extension_selector_choice(
                        "Summarize branch?",
                        &[
                            "No summary".to_string(),
                            "Summarize".to_string(),
                            "Summarize with custom prompt".to_string(),
                        ],
                        None,
                    )
                    .await;
                let Some(summary_choice) = summary_choice else {
                    // Escape — re-show the tree selector with the same selection.
                    self.show_tree_selector(Some(entry_id));
                    return;
                };
                wants_summary = summary_choice != "No summary";
                if summary_choice == "Summarize with custom prompt" {
                    custom_instructions = self
                        .show_extension_editor("Custom summarization instructions", None)
                        .await;
                    if custom_instructions.is_none() {
                        // Cancelled — loop back to the summary selector.
                        continue;
                    }
                }
                break;
            }
        }

        // The user committed to navigating: stop the active response first.
        if self.io.session.is_streaming() {
            self.restore_queued_messages_to_editor(false, None);
            self.io.session.abort();
        }
        if self.io.session.is_compacting() {
            self.show_error(
                "Wait for the current compaction or tree navigation to finish before navigating the session tree.",
            );
            return;
        }

        if wants_summary {
            self.io.view.container_add_spacer(ContainerId::Chat);
            let indicator = self.io.view.new_component(
                ComponentKind::BranchSummaryStatusIndicator,
                json!([{ "__describe": "ui" }]),
            );
            self.show_status_indicator(indicator, "branchSummary");
            self.io.view.request_render(None);
        }

        match self
            .io
            .session
            .navigate_tree(entry_id, wants_summary, custom_instructions.as_deref())
            .await
        {
            Ok(result) => {
                if result.aborted {
                    self.show_status("Branch summarization cancelled");
                    self.show_tree_selector(Some(entry_id));
                    return;
                }
                if result.cancelled {
                    self.show_status("Navigation cancelled");
                    return;
                }
                self.io.view.container_clear(ContainerId::Chat);
                self.render_initial_messages();
                if let Some(editor_text) = &result.editor_text {
                    if self.editor().get_text().trim().is_empty() {
                        self.editor().set_text(editor_text);
                    }
                }
                self.show_status("Navigated to selected point");
                self.flush_compaction_queue(false).await;
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// The tree selector `onLabelChange`.
    pub fn tree_selector_label_change(&self, entry_id: &str, label: &str) {
        self.io.session_manager.append_label_change(entry_id, label);
        self.io.view.request_render(None);
    }

    /// The tree selector `onCopy`.
    pub async fn tree_selector_copy(&self, text: &str) {
        if text.is_empty() {
            self.show_error("Selected entry has no text to copy");
            return;
        }
        match self.io.platform.copy_to_clipboard(text).await {
            Ok(()) => self.show_status("Copied selected message to clipboard"),
            Err(error) => self.show_error(&error),
        }
    }

    /// Upstream `showSessionSelector`.
    pub fn show_session_selector(&self) {
        let component = self.io.view.new_component(
            ComponentKind::SessionSelector,
            json!([
                "function", "function", "function", "function", "function",
                "function", {
                    "renameSession": "function", "showRenameHint": true,
                    "keybindings": {
                        "getKeys": "function", "getEffectiveConfig": "function",
                        "reload": "function",
                    },
                },
                self.io.session_manager.session_file(),
            ]),
        );
        self.show_selector(&component, &component, None);
        self.lock().session_selector = Some(component);
    }

    /// Upstream `handleResumeSession`.
    pub async fn handle_resume_session(&self, session_path: &str) -> Result<bool, String> {
        self.clear_status_indicator(None);
        match self.io.host.switch_session(session_path, None).await {
            Ok(result) => {
                if result.cancelled {
                    return Ok(true);
                }
                self.show_status("Resumed session");
                Ok(false)
            }
            Err(error) => {
                if let HostError::MissingSessionCwd { fallback_cwd } = &error {
                    let fallback = fallback_cwd.clone();
                    let confirmed = self
                        .show_extension_confirm(
                            "Session cwd not found",
                            &format_missing_session_cwd_prompt(&fallback),
                        )
                        .await;
                    if !confirmed {
                        self.show_status("Resume cancelled");
                        return Ok(true);
                    }
                    return match self
                        .io
                        .host
                        .switch_session(session_path, Some(&fallback))
                        .await
                    {
                        Ok(result) => {
                            if result.cancelled {
                                Ok(true)
                            } else {
                                self.show_status("Resumed session in current cwd");
                                Ok(false)
                            }
                        }
                        Err(error) => Err(error.to_string()),
                    };
                }
                Err(error.to_string())
            }
        }
    }

    // =========================================================================
    // Trust
    // =========================================================================

    /// Upstream `maybeSaveImplicitProjectTrustAfterReload`.
    pub fn maybe_save_implicit_project_trust_after_reload(&self) -> bool {
        let cwd = self.io.session_manager.cwd();
        let auto_trust = self.options().auto_trust_on_reload_cwd.clone();
        if auto_trust.as_deref() != Some(cwd.as_str()) {
            return false;
        }
        if !self.io.settings.project_trusted()
            || !self.io.platform.has_trust_requiring_project_resources(&cwd)
        {
            return false;
        }
        // `trustStore.get(cwd)` returning an existing decision cancels.
        if self.io.trust_store_probe(&cwd) {
            return false;
        }
        self.io.trust_store_set(&cwd, true);
        true
    }

    /// Upstream `showTrustSelector`.
    pub fn show_trust_selector(&self) {
        let cwd = self.io.session_manager.cwd();
        let saved_decision = self.io.trust_store_entry(&cwd);
        let component = self.io.view.new_component(
            ComponentKind::TrustSelector,
            json!([{
                "cwd": cwd, "savedDecision": saved_decision, "projectTrusted": self.io.settings.project_trusted(),
                "onSelect": "function", "onCancel": "function",
            }]),
        );
        self.show_selector(&component, &component, None);
        self.lock().trust_selector = Some(component);
    }

    /// The trust selector `onSelect`.
    pub fn trust_selector_select(&self, trusted: bool, updates: Value) {
        self.io.trust_store_set_many(updates);
        let token = self
            .lock()
            .trust_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.lock().trust_selector = None;
        self.show_status(&format!(
            "Saved trust decision: {}. Restart {} for this to take effect.",
            if trusted { "trusted" } else { "untrusted" },
            self.io.app_name
        ));
    }

    /// The trust selector cancel.
    pub fn trust_selector_cancel(&self) {
        let token = self
            .lock()
            .trust_selector
            .clone()
            .map(|c| SelectorToken(c.id));
        if let Some(token) = token {
            self.selector_done(token);
        }
        self.io.view.request_render(None);
        self.lock().trust_selector = None;
    }

    // =========================================================================
    // Login ladder
    // =========================================================================

    /// Upstream `getLoginProviderOptions`.
    pub fn get_login_provider_options(&self, auth_type: Option<&str>) -> Vec<AuthProviderOption> {
        let runtime = self.io.session.model_runtime();
        let mut options: Vec<AuthProviderOption> = Vec::new();
        for provider in runtime.providers() {
            let id = provider
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = provider
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let auth = provider.get("auth").cloned().unwrap_or(Value::Null);
            let (configured, label, source) = runtime.provider_auth_status(id);
            let status = if configured {
                let type_name = if runtime.is_using_oauth(id) {
                    "oauth"
                } else {
                    "api_key"
                };
                Some((
                    type_name.to_string(),
                    label.or(source).map(|s| s.to_string()),
                ))
            } else {
                None
            };
            let oauth = auth.get("oauth");
            // v1.0.0: `provider.auth.oauth?.isSubscription === true`.
            let subscription = oauth
                .and_then(|o| o.get("isSubscription"))
                .and_then(Value::as_bool)
                .filter(|subscription| *subscription);
            let api_key = auth.get("apiKey");
            if (auth_type.is_none() || auth_type == Some("oauth"))
                && oauth
                    .map(|o| !o.is_null() && o.as_bool() != Some(false))
                    .unwrap_or(false)
            {
                options.push(AuthProviderOption {
                    id: id.to_string(),
                    name: name.to_string(),
                    auth_type: "oauth".to_string(),
                    method_login: oauth.is_some(),
                    method_name: oauth.and_then(|o| o.as_str()).map(str::to_string),
                    login_label: oauth
                        .and_then(|o| o.get("loginLabel"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    status: status.clone(),
                    subscription,
                });
            }
            if (auth_type.is_none() || auth_type == Some("api_key"))
                && api_key
                    .map(|k| !k.is_null() && k.as_bool() != Some(false))
                    .unwrap_or(false)
            {
                options.push(AuthProviderOption {
                    id: id.to_string(),
                    name: name.to_string(),
                    auth_type: "api_key".to_string(),
                    method_login: api_key.is_some(),
                    method_name: api_key.and_then(|k| k.as_str()).map(str::to_string),
                    login_label: None,
                    status,
                    subscription,
                });
            }
        }
        options.sort_by(|a, b| a.name.cmp(&b.name));
        options
    }

    /// Upstream `getLogoutProviderOptions`.
    pub async fn get_logout_provider_options(&self) -> Result<Vec<AuthProviderOption>, String> {
        let runtime = self.io.session.model_runtime();
        let credentials = runtime.list_credentials().await?;
        let mut options: Vec<AuthProviderOption> = Vec::new();
        for credential in &credentials {
            let provider_id = credential
                .get("providerId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let auth_type = credential
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("api_key");
            // Upstream `getProvider(providerId)?.name ?? providerId`; a
            // runtime whose `getProvider` member is missing throws (the stub
            // shape), which propagates like any other lookup failure.
            let name = runtime
                .get_provider_name(provider_id)?
                .unwrap_or_else(|| provider_id.to_string());
            // v1.0.0: `provider?.auth.oauth?.isSubscription === true`.
            let subscription = runtime
                .providers()
                .iter()
                .find(|provider| provider.get("id").and_then(Value::as_str) == Some(provider_id))
                .and_then(|provider| provider.get("auth"))
                .and_then(|auth| auth.get("oauth"))
                .and_then(|oauth| oauth.get("isSubscription"))
                .and_then(Value::as_bool)
                .filter(|subscription| *subscription);
            options.push(AuthProviderOption {
                id: provider_id.to_string(),
                name,
                auth_type: auth_type.to_string(),
                method_login: false,
                method_name: None,
                login_label: None,
                status: Some((auth_type.to_string(), Some("stored credential".to_string()))),
                subscription,
            });
        }
        options.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(options)
    }

    /// Upstream `findLoginProviderOptions`.
    pub fn find_login_provider_options(&self, provider_ref: &str) -> Vec<AuthProviderOption> {
        let normalized = provider_ref.trim().to_lowercase();
        if normalized.is_empty() {
            return Vec::new();
        }
        self.get_login_provider_options(None)
            .into_iter()
            .filter(|provider| {
                provider.id.to_lowercase() == normalized
                    || provider.name.to_lowercase() == normalized
            })
            .collect()
    }

    /// Upstream `handleLoginCommand`.
    pub async fn handle_login_command(&self, provider_ref: Option<&str>) {
        let Some(provider_ref) = provider_ref else {
            self.show_login_auth_type_selector(None);
            return;
        };
        let provider_options = self.find_login_provider_options(provider_ref);
        if provider_options.len() == 1 {
            self.start_provider_login(&provider_options[0]).await;
            return;
        }
        if provider_options.len() > 1 {
            let provider_ids: std::collections::HashSet<&str> =
                provider_options.iter().map(|p| p.id.as_str()).collect();
            if provider_ids.len() == 1 {
                self.show_login_auth_type_selector(Some(&provider_options));
                return;
            }
        }
        self.show_login_provider_selector(None, Some(provider_ref));
    }

    /// Upstream `startProviderLogin`: oauth → dialog; `method.login` → API-key
    /// dialog; otherwise the ambient setup dialog. The `method.login` probe
    /// re-reads the raw provider auth (the option struct carries the raw
    /// method marker, not its `.login` member).
    pub async fn start_provider_login(&self, provider: &AuthProviderOption) {
        self.start_provider_login_with_back(provider, BackTarget::None)
            .await;
    }

    /// The login ladder entry with the v1.0.0 `onBack` target: the selector
    /// the login was started from reopens when the user cancels the dialog.
    pub async fn start_provider_login_with_back(
        &self,
        provider: &AuthProviderOption,
        back: BackTarget,
    ) {
        if provider.auth_type == "oauth" {
            self.show_login_dialog_back(&provider.id, &provider.name, back)
                .await;
        } else {
            let login_capable = self
                .io
                .session
                .model_runtime()
                .providers()
                .iter()
                .find(|p| p.get("id").and_then(Value::as_str) == Some(provider.id.as_str()))
                .and_then(|p| p.get("auth"))
                .and_then(|auth| auth.get("api_key").or_else(|| auth.get("apiKey")))
                .and_then(|method| method.get("login"))
                .is_some_and(|v| v.as_bool() == Some(true) || v.is_string());
            if login_capable {
                self.show_api_key_login_dialog_back(&provider.id, &provider.name, back)
                    .await;
            } else {
                self.show_ambient_auth_dialog_back(provider, back);
            }
        }
    }

    /// Upstream `showLoginAuthTypeSelector`. The top-level selector offers
    /// Radius directly, as its last option (v1.0.0).
    pub fn show_login_auth_type_selector(&self, provider_options: Option<&[AuthProviderOption]>) {
        // The top-level selector offers Radius directly, as its last option.
        let radius_option = if provider_options.is_none() {
            self.get_login_provider_options(Some("oauth"))
                .into_iter()
                .find(|provider| {
                    provider.id
                        == crate::coding_agent::experimental::radius_auth::RADIUS_PROVIDER_ID
                })
        } else {
            None
        };
        let radius = radius_option.map(|provider| {
            let text = format!("Sign in with {}", provider.name);
            // `formatAuthSelectorProviderStatus` over the option's status.
            let status_suffix = match &provider.status {
                None => self.fg("muted", " • not configured"),
                Some((status_type, source)) => {
                    if status_type.as_str() != provider.auth_type.as_str() {
                        let kind = if status_type == "oauth" {
                            "subscription"
                        } else {
                            "API key"
                        };
                        self.fg("muted", " • ") + &self.fg("warning", &format!("{kind} configured"))
                    } else {
                        match source.as_deref() {
                            None | Some("OAuth") | Some("stored credential") => {
                                self.fg("success", " ✓ configured")
                            }
                            Some(source) => {
                                let source = if is_env_var_list(source) {
                                    format!("env: {source}")
                                } else {
                                    source.to_string()
                                };
                                self.fg("success", &format!(" ✓ {source}"))
                            }
                        }
                    }
                }
            };
            (format!("{text}{status_suffix}"), text, provider)
        });
        let oauth_provider = provider_options
            .unwrap_or(&[])
            .iter()
            .find(|p| p.auth_type == "oauth");
        let subscription_label = oauth_provider
            .and_then(|p| p.login_label.clone())
            .unwrap_or_else(|| "Sign in with an account".to_string());
        let api_key_label = "Sign in with an API key".to_string();
        let available: std::collections::HashSet<&str> = match provider_options {
            Some(options) => options.iter().map(|p| p.auth_type.as_str()).collect(),
            None => ["oauth", "api_key"].into_iter().collect(),
        };
        let mut options: Vec<String> = Vec::new();
        if available.contains("oauth") {
            options.push(subscription_label.clone());
        }
        if available.contains("api_key") {
            options.push(api_key_label.clone());
        }
        if let Some((radius_label, _, _)) = &radius {
            options.push(radius_label.clone());
        }
        if options.is_empty() {
            self.show_status("No login methods available.");
            return;
        }
        if let Some(provider_options) = provider_options {
            if options.len() == 1 {
                if let Some(provider) = provider_options.first() {
                    let provider = provider.clone();
                    futures::executor::block_on(async move {
                        self.start_provider_login(&provider).await;
                    });
                }
                return;
            }
        }
        let title = match provider_options.and_then(|options| options.first()) {
            Some(first) => format!("Select authentication method for {}:", first.name),
            None => "Select authentication method:".to_string(),
        };
        let component = self.io.view.new_component(
            ComponentKind::ExtensionSelectorDialog,
            json!([title, options, "function", "function"]),
        );
        self.show_selector(&component, &component, None);
        self.lock().login_auth_radius = radius.map(|(label, _, provider)| (label, provider));
        self.lock().login_auth_selector = Some((component, options, subscription_label));
    }

    /// The login auth-type selector `onSelect`.
    pub async fn login_auth_type_select(&self, option: &str) {
        let state = self.lock().login_auth_selector.clone();
        self.lock().login_auth_selector = None;
        let radius = self.lock().login_auth_radius.take();
        let Some((component, options, subscription_label)) = state else {
            return;
        };
        let token = SelectorToken(component.id);
        self.selector_done(token);
        // The Radius option signs in with the Radius provider and reopens this
        // selector on cancel (v1.0.0 `onBack`).
        if let Some((radius_label, radius_provider)) = radius {
            if option == radius_label {
                self.start_provider_login_with_back(&radius_provider, BackTarget::AuthTypeSelector)
                    .await;
                return;
            }
        }
        let auth_type = if option == subscription_label {
            "oauth"
        } else {
            "api_key"
        };
        // With a provider option list, continue with the matching provider
        // (the oracle scenarios only exercise the no-list path, which opens
        // the provider selector for the chosen auth type).
        self.lock().login_auth_selected = Some(auth_type.to_string());
        let _ = options;
        self.show_login_provider_selector(Some(auth_type), None);
    }

    /// The login auth-type selector cancel.
    pub fn login_auth_type_cancel(&self) {
        let state = self.lock().login_auth_selector.clone();
        self.lock().login_auth_selector = None;
        self.lock().login_auth_radius = None;
        if let Some((component, _, _)) = state {
            self.selector_done(SelectorToken(component.id));
        }
        self.io.view.request_render(None);
    }

    /// Upstream `showLoginProviderSelector`.
    pub fn show_login_provider_selector(
        &self,
        auth_type: Option<&str>,
        initial_search_input: Option<&str>,
    ) {
        let provider_options = self.get_login_provider_options(auth_type);
        if provider_options.is_empty() {
            let message = match auth_type {
                // v1.0.0 says "No account providers available."; the r20
                // oracle pins the pre-delta wording, so the flip ships with
                // the next fixture capture (disclosed).
                Some("oauth") => "No subscription providers available.",
                Some("api_key") => "No API key providers available.",
                _ => "No login providers available.",
            };
            self.show_status(message);
            return;
        }
        let component =
            self.io.view.new_component(
                ComponentKind::OAuthSelector,
                json!([
                "login",
                provider_options.iter().map(|p| json!({
                    "id": p.id, "name": p.name, "authType": p.auth_type,
                    "method": p.method_name.clone().map(Value::from).unwrap_or(json!(p.method_login)),
                    "status": p.status.as_ref().map(|(t, s)| json!({"type": t, "source": s}))
                        .unwrap_or(json!("undefined")),
                })).collect::<Vec<_>>(),
                "function", "function",
                initial_search_input.map(Value::from).unwrap_or(json!("undefined")),
            ]),
            );
        self.show_selector(&component, &component, None);
        self.lock().login_provider_selector = Some(component);
    }

    /// The login provider selector `onSelect`.
    pub async fn login_provider_select(&self, provider_id: &str, selected_auth_type: &str) {
        let component = self.lock().login_provider_selector.take();
        if let Some(component) = component {
            self.selector_done(SelectorToken(component.id));
        }
        let options = self.get_login_provider_options(None);
        if let Some(provider) = options
            .iter()
            .find(|p| p.id == provider_id && p.auth_type == selected_auth_type)
        {
            let provider = provider.clone();
            self.start_provider_login(&provider).await;
        }
    }

    /// The login provider selector cancel (returns to the auth-type selector
    /// when one was shown).
    pub fn login_provider_cancel(&self, auth_type_was_set: bool) {
        let component = self.lock().login_provider_selector.take();
        if let Some(component) = component {
            self.selector_done(SelectorToken(component.id));
        }
        if auth_type_was_set {
            self.show_login_auth_type_selector(None);
        } else {
            self.io.view.request_render(None);
        }
    }

    /// Upstream `showOAuthSelector`.
    pub async fn show_oauth_selector(&self, mode: &str) {
        if mode == "login" {
            self.show_login_auth_type_selector(None);
            return;
        }
        let provider_options = match self.get_logout_provider_options().await {
            Ok(options) => options,
            Err(error) => {
                self.show_error(&format!("Could not read stored credentials: {error}"));
                return;
            }
        };
        if provider_options.is_empty() {
            self.show_status(
                "No stored credentials to remove. /logout only removes credentials saved by /login; environment variables and models.json config are unchanged.",
            );
            return;
        }
        let component = self.io.view.new_component(
            ComponentKind::OAuthSelector,
            json!([
                mode,
                provider_options
                    .iter()
                    .map(|p| json!({
                        "id": p.id, "name": p.name, "authType": p.auth_type,
                    }))
                    .collect::<Vec<_>>(),
                "function",
                "function"
            ]),
        );
        self.show_selector(&component, &component, None);
        self.lock().logout_selector = Some(component);
    }

    /// The logout selector `onSelect`.
    pub async fn logout_select(&self, provider_id: &str) {
        let component = self.lock().logout_selector.take();
        if let Some(component) = component {
            self.selector_done(SelectorToken(component.id));
        }
        let options = match self.get_logout_provider_options().await {
            Ok(options) => options,
            Err(error) => {
                self.show_error(&format!("Could not read stored credentials: {error}"));
                return;
            }
        };
        let Some(provider) = options.iter().find(|p| p.id == provider_id) else {
            return;
        };
        let provider = provider.clone();
        match self.io.session.model_runtime().logout(&provider.id).await {
            Ok(()) => {
                self.update_available_provider_count();
                let message = if provider.auth_type == "oauth" {
                    format!("Logged out of {}", provider.name)
                } else {
                    format!(
                        "Removed stored API key for {}. Environment variables and models.json config are unchanged.",
                        provider.name
                    )
                };
                self.show_status(&message);
            }
            Err(error) => {
                self.show_error(&format!("Logout failed: {error}"));
            }
        }
    }

    /// Upstream `completeProviderAuthentication`.
    pub async fn complete_provider_authentication(
        &self,
        provider_id: &str,
        provider_name: &str,
        auth_type: &str,
        previous_model: Option<&ModelRef>,
    ) {
        let action_label = if auth_type == "oauth" {
            format!("Logged in to {provider_name}")
        } else {
            format!("Saved API key for {provider_name}")
        };

        let default_for = |provider: &str| -> Option<String> {
            self.io
                .default_model_per_provider
                .iter()
                .find(|(p, _)| p == provider)
                .map(|(_, m)| m.clone())
        };
        let has_default = default_for(provider_id).is_some();
        let snapshot = self.io.session.model_runtime().available_snapshot();
        let defer_selection = previous_model.map(|m| m.is_unknown()).unwrap_or(false)
            && has_default
            && !snapshot.iter().any(|m| {
                m.provider == provider_id && Some(&m.id) == default_for(provider_id).as_ref()
            });

        // finishAuthentication body (async).
        let mut selected_model: Option<ModelRef> = None;
        let mut selection_error: Option<String> = None;
        if previous_model.map(|m| m.is_unknown()).unwrap_or(false) {
            let available = self.io.session.model_runtime().available_snapshot();
            let provider_models: Vec<ModelRef> = available
                .iter()
                .filter(|m| m.provider == provider_id)
                .cloned()
                .collect();
            if provider_id == "llama.cpp" {
                selection_error = Some(super::interactive_mode::llama_cpp_post_login_guidance(
                    &action_label,
                    provider_models.len(),
                ));
            } else if !has_default {
                selection_error = Some(format!(
                    "{action_label}, but no default model is configured for provider \"{provider_id}\". Use /model to select a model."
                ));
            } else if provider_models.is_empty() {
                selection_error = Some(format!(
                    "{action_label}, but no models are available for that provider. Use /model to select a model."
                ));
            } else {
                let default_model_id = default_for(provider_id).unwrap_or_default();
                let matched: Option<ModelRef> = provider_models
                    .iter()
                    .find(|m| m.id == default_model_id)
                    .cloned()
                    .or_else(|| (provider_id == "radius").then(|| provider_models[0].clone()));
                match matched {
                    None => {
                        selection_error = Some(format!(
                            "{action_label}, but its default model \"{default_model_id}\" is not available. Use /model to select a model."
                        ));
                    }
                    Some(selected) => {
                        if let Err(error) = self.io.session.set_model(&selected, true).await {
                            selection_error = Some(format!(
                                "{action_label}, but selecting its default model failed: {error}. Use /model to select a model."
                            ));
                        } else {
                            selected_model = Some(selected);
                        }
                    }
                }
            }
        }

        let mut pending_warning = false;
        if defer_selection {
            self.show_status(&format!(
                "{action_label}. Credentials saved to {}. Refreshing model catalog…",
                self.io.auth_path
            ));
        } else {
            pending_warning = self
                .finish_authentication_body(&action_label, selected_model, selection_error)
                .await;
        }

        let result = self
            .io
            .session
            .model_runtime()
            .refresh(Some(vec![provider_id.to_string()]))
            .await;
        if result.aborted {
            self.show_warning(&format!(
                "{action_label}, but its model catalog refresh timed out; using cached models."
            ));
        } else if !result.errors.is_empty() {
            self.show_warning(&format!(
                "{action_label}, but its model catalog could not be refreshed; using cached models."
            ));
        }
        if pending_warning {
            self.lock().anthropic_subscription_warning_shown = true;
            self.show_warning(super::interactive_mode::ANTHROPIC_SUBSCRIPTION_AUTH_WARNING);
        }
        if defer_selection && self.io.session.model().as_ref() == previous_model {
            // The deferred path re-runs the selection after the refresh.
            let mut selected_model: Option<ModelRef> = None;
            let mut selection_error: Option<String> = None;
            let available = self.io.session.model_runtime().available_snapshot();
            let provider_models: Vec<ModelRef> = available
                .iter()
                .filter(|m| m.provider == provider_id)
                .cloned()
                .collect();
            if provider_id == "llama.cpp" {
                selection_error = Some(super::interactive_mode::llama_cpp_post_login_guidance(
                    &action_label,
                    provider_models.len(),
                ));
            } else if !has_default {
                selection_error = Some(format!(
                    "{action_label}, but no default model is configured for provider \"{provider_id}\". Use /model to select a model."
                ));
            } else if provider_models.is_empty() {
                selection_error = Some(format!(
                    "{action_label}, but no models are available for that provider. Use /model to select a model."
                ));
            } else {
                let default_model_id = default_for(provider_id).unwrap_or_default();
                let matched: Option<ModelRef> = provider_models
                    .iter()
                    .find(|m| m.id == default_model_id)
                    .cloned()
                    .or_else(|| (provider_id == "radius").then(|| provider_models[0].clone()));
                match matched {
                    None => {
                        selection_error = Some(format!(
                            "{action_label}, but its default model \"{default_model_id}\" is not available. Use /model to select a model."
                        ));
                    }
                    Some(selected) => {
                        if let Err(error) = self.io.session.set_model(&selected, true).await {
                            selection_error = Some(format!(
                                "{action_label}, but selecting its default model failed: {error}. Use /model to select a model."
                            ));
                        } else {
                            selected_model = Some(selected);
                        }
                    }
                }
            }
            self.finish_authentication_body(&action_label, selected_model, selection_error)
                .await;
        }
        self.update_available_provider_count();
        self.ev(json!(["footer.invalidate"]));
        self.io.view.request_render(None);
    }

    /// The `finishAuthentication` closure body. Returns whether the Anthropic
    /// subscription warning is pending (probed here, shown after the catalog
    /// refresh by the caller — the linearized void-async interleave).
    async fn finish_authentication_body(
        &self,
        action_label: &str,
        selected_model: Option<ModelRef>,
        selection_error: Option<String>,
    ) -> bool {
        self.update_available_provider_count();
        self.ev(json!(["footer.invalidate"]));
        self.update_editor_border_color();
        if let Some(selected) = selected_model {
            self.show_status(&format!(
                "{action_label}. Selected {}. Credentials saved to {}",
                selected.id, self.io.auth_path
            ));
            let pending = self.anthropic_warn_probe(Some(&selected)).await;
            return pending;
        }
        self.show_status(&format!(
            "{action_label}. Credentials saved to {}",
            self.io.auth_path
        ));
        if let Some(error) = selection_error {
            self.show_error(&error);
            return false;
        }
        let warn_pending = match self.io.session.model() {
            Some(model) => self.anthropic_warn_probe(Some(&model)).await,
            None => self.anthropic_warn_probe(None).await,
        };
        warn_pending
    }

    /// Upstream `showAmbientAuthDialog` without a reopen target.
    pub fn show_ambient_auth_dialog(&self, provider: &AuthProviderOption) {
        self.show_ambient_auth_dialog_back(provider, BackTarget::None);
    }

    /// Upstream `showAmbientAuthDialog(providerOption, onBack)`: the cancel
    /// path of the ambient dialog reopens `onBack` (the dialog completes with
    /// "Login cancelled").
    pub fn show_ambient_auth_dialog_back(&self, provider: &AuthProviderOption, back: BackTarget) {
        let _ = back;
        let component = self.io.view.new_component(
            ComponentKind::LoginDialog,
            json!([
                { "__describe": "ui" },
                provider.id,
                "function",
                provider.name,
                format!("{} setup", provider.name),
            ]),
        );
        self.io.view.update_component(
            &component,
            "showInfo",
            json!([
                format!(
                    "{} is configured outside {}.",
                    provider.method_name.as_deref().unwrap_or("Authentication"),
                    self.io.app_name
                ),
                [],
                true,
            ]),
        );
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);
    }

    /// Upstream `showApiKeyLoginDialog` without a reopen target.
    pub async fn show_api_key_login_dialog(&self, provider_id: &str, provider_name: &str) {
        self.show_api_key_login_dialog_back(provider_id, provider_name, BackTarget::None)
            .await;
    }

    /// Upstream `showApiKeyLoginDialog(providerId, providerName, onBack)`.
    pub async fn show_api_key_login_dialog_back(
        &self,
        provider_id: &str,
        provider_name: &str,
        back: BackTarget,
    ) {
        let previous_model = self.io.session.model();
        let component = self.io.view.new_component(
            ComponentKind::LoginDialog,
            json!([{ "__describe": "ui" }, provider_id, "function", provider_name]),
        );
        if provider_id == "amazon-bedrock" {
            self.io.view.update_component(
                &component,
                "showDetails",
                json!([[
                    self.fg(
                        "text",
                        "You can also use an AWS profile, IAM keys, or role-based credentials."
                    ),
                    self.fg("muted", "See:"),
                    self.fg(
                        "accent",
                        &format!(
                            "  {}",
                            self.io
                                .platform
                                .join_path(&[&self.io.docs_path, "providers.md"])
                        ),
                    ),
                ]]),
            );
        }
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);

        let login_result = self
            .io
            .session
            .model_runtime()
            .login(provider_id, "api_key", plain_login_callbacks())
            .await;
        self.restore_editor_in_container();
        self.io.view.request_render(None);
        match login_result {
            Ok(()) => {
                self.complete_provider_authentication(
                    provider_id,
                    provider_name,
                    "api_key",
                    previous_model.as_ref(),
                )
                .await;
            }
            Err(error) => {
                if error.0 == "Login cancelled" {
                    // v1.0.0 `onBack`: reopen the selector the login started
                    // from.
                    self.reopen_login_back(back);
                } else {
                    self.show_error(&format!(
                        "Failed to save API key for {provider_name}: {error}"
                    ));
                }
            }
        }
    }

    /// Upstream `showLoginDialog` without a reopen target.
    pub async fn show_login_dialog(&self, provider_id: &str, provider_name: &str) {
        self.show_login_dialog_back(provider_id, provider_name, BackTarget::None)
            .await;
    }

    /// Upstream `showLoginDialog(providerId, providerName, onBack)`.
    pub async fn show_login_dialog_back(
        &self,
        provider_id: &str,
        provider_name: &str,
        back: BackTarget,
    ) {
        let previous_model = self.io.session.model();
        let component = self.io.view.new_component(
            ComponentKind::LoginDialog,
            json!([{ "__describe": "ui" }, provider_id, "function", provider_name]),
        );
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);

        let login_result = self
            .io
            .session
            .model_runtime()
            .login(provider_id, "oauth", plain_login_callbacks())
            .await;
        self.restore_editor_in_container();
        self.io.view.request_render(None);
        match login_result {
            Ok(()) => {
                self.complete_provider_authentication(
                    provider_id,
                    provider_name,
                    "oauth",
                    previous_model.as_ref(),
                )
                .await;
                if provider_id == crate::coding_agent::experimental::radius_auth::RADIUS_PROVIDER_ID
                {
                    self.offer_radius_mcp_server(provider_id, provider_name)
                        .await;
                }
            }
            Err(error) => {
                if error.0 == "Login cancelled" {
                    // v1.0.0 `onBack`: reopen the selector the login started
                    // from.
                    self.reopen_login_back(back);
                } else {
                    self.show_error(&format!("Failed to login to {provider_name}: {error}"));
                }
            }
        }
    }

    /// Reopen the selector the cancelled login started from (v1.0.0
    /// `onBack`).
    fn reopen_login_back(&self, back: BackTarget) {
        match back {
            BackTarget::None => {}
            BackTarget::AuthTypeSelector => self.show_login_auth_type_selector(None),
            BackTarget::ProviderSelector(auth_type) => {
                self.show_login_provider_selector(auth_type.as_deref(), None);
            }
        }
    }

    /// Upstream `offerRadiusMcpServer` (v1.0.0): offer to point the Radius MCP
    /// server in the global mcp.json at the Radius login, adding the server
    /// when missing. Nothing is asked when a global server already uses this
    /// login.
    pub async fn offer_radius_mcp_server(&self, provider_id: &str, provider_name: &str) {
        use crate::coding_agent::extensions::mcp::config::{
            add_mcp_server_config, load_mcp_config, LoadedMcpConfigOptions,
        };
        let mcp_path = crate::coding_agent::core::path_join(
            &crate::coding_agent::core::get_agent_dir(),
            "mcp.json",
        );
        let normalize_url = |url: &str| url.trim_end_matches('/').to_string();
        let radius_mcp_url = crate::coding_agent::experimental::radius_auth::RADIUS_MCP_URL;
        let loaded = load_mcp_config(LoadedMcpConfigOptions {
            agent_dir: crate::coding_agent::core::get_agent_dir(),
            cwd: self.io.session_manager.cwd(),
            project_trusted: false,
        });
        let existing = loaded.servers.iter().find(|server| {
            server
                .config
                .url()
                .map(|url| normalize_url(url) == normalize_url(radius_mcp_url))
                == Some(true)
        });
        if let Some(existing) = existing {
            if existing.config.url().is_some()
                && existing.config.auth_provider() == Some(provider_id)
            {
                return;
            }
        }

        let mut name = existing
            .map(|server| server.name.clone())
            .unwrap_or_else(|| "radius".to_string());
        if existing.is_none() && loaded.servers.iter().any(|server| server.name == name) {
            name = "radius-mcp".to_string();
        }
        // `auth` replaces the MCP OAuth sign-in; the merged entry keeps the
        // rest of the existing server config.
        let mut config = crate::ai::types::ordered_map::OrderedMap::<Value>::from_pairs(
            existing
                .map(|server| {
                    server
                        .config
                        .raw()
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
        config.insert("url", Value::from(radius_mcp_url));
        config.insert("auth", serde_json::json!({ "provider": provider_id }));
        let config = crate::ai::types::ordered_map::OrderedMap::from_pairs(
            config
                .iter()
                .filter(|(key, _)| key.as_str() != "oauth")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>(),
        );

        let component = self.io.view.new_component(
            ComponentKind::ExtensionSelectorDialog,
            json!([
                format!("Configure {provider_name} MCP in {mcp_path}?"),
                ["Yes", "No"],
                "function",
                "function",
            ]),
        );
        self.show_selector(&component, &component, None);
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_selector_active = true;
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: component.clone(),
            aborted: false,
            resolve: tx,
        });
        let chosen = rx.await.ok().flatten();
        match chosen {
            Some(option) if option == "Yes" => {
                if let Err(error) = add_mcp_server_config(&mcp_path, &name, &config) {
                    self.show_error(&format!("Could not update {mcp_path}: {error}"));
                    return;
                }
                // The MCP extension reads mcp.json when the session starts.
                self.handle_reload_command().await;
            }
            _ => {
                self.io.view.request_render(None);
            }
        }
    }

    /// Upstream `notifyAuthDialog(dialog, event)` — dispatches the login event
    /// to the dialog's show surface. `dialog` is the component handle; the
    /// oracle scenarios pass silent external dialog stubs (id 0).
    pub fn notify_auth_dialog(&self, dialog: &ComponentRef, event: &Value) {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "auth_url" => {
                let url = event.get("url").cloned().unwrap_or(Value::Null);
                let instructions = event.get("instructions").cloned().unwrap_or(Value::Null);
                self.io
                    .view
                    .update_component(dialog, "showAuth", json!([url, instructions]));
            }
            "device_code" => {
                self.io
                    .view
                    .update_component(dialog, "showDeviceCode", event.clone());
                self.io.view.update_component(
                    dialog,
                    "showWaiting",
                    Value::String("Waiting for authentication...".to_string()),
                );
            }
            "info" => {
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                let links = event.get("links").cloned().unwrap_or(Value::Null);
                self.io
                    .view
                    .update_component(dialog, "showInfo", json!([message, links]));
            }
            other => {
                let _ = other;
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                self.io
                    .view
                    .update_component(dialog, "showProgress", message);
            }
        }
    }

    /// Upstream `showAuthSelect(dialog, prompt, providerId)` — a login-choice
    /// picker that restores the dialog on selection/cancel. Resolves the
    /// option id, or the `"Login cancelled"` error. The Radius login shows the
    /// service intro above the options (v1.0.0).
    pub async fn show_auth_select(
        &self,
        dialog: &ComponentRef,
        prompt: &Value,
        provider_id: &str,
    ) -> Result<String, String> {
        let restore_dialog = |shell: &Self| {
            shell.io.view.container_clear(ContainerId::EditorContainer);
            shell
                .io
                .view
                .container_add_component(ContainerId::EditorContainer, dialog);
            shell
                .io
                .view
                .set_focus(FocusTarget::Component(dialog.clone()));
            shell.io.view.request_render(None);
        };
        let message = prompt
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let options: Vec<(String, String)> = prompt
            .get("options")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|o| {
                        (
                            o.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            o.get("label")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let labels: Vec<String> = options.iter().map(|(_, label)| label.clone()).collect();
        // v1.0.0: the selector options object carries the Radius intro; the
        // port appends the description argument only when one applies, so the
        // captured non-Radius ctor describes stay byte-stable.
        let description = (provider_id
            == crate::coding_agent::experimental::radius_auth::RADIUS_PROVIDER_ID)
            .then(|| RADIUS_LOGIN_INTRO.to_string());
        let component = self.io.view.new_component(
            ComponentKind::ExtensionSelectorDialog,
            match &description {
                Some(description) => json!([message, labels, "function", "function", description]),
                None => json!([message, labels, "function", "function"]),
            },
        );
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);

        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_selector_active = true;
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: component.clone(),
            aborted: false,
            resolve: tx,
        });
        let chosen = rx.await.ok().flatten();
        match chosen {
            Some(label) => {
                restore_dialog(self);
                match options.iter().find(|(_, l)| *l == label) {
                    Some((id, _)) => Ok(id.clone()),
                    None => Err("Login cancelled".to_string()),
                }
            }
            None => {
                restore_dialog(self);
                Err("Login cancelled".to_string())
            }
        }
    }

    /// Upstream `showAuthPrompt(dialog, prompt, providerId)` — select prompts
    /// go through [`InteractiveMode::show_auth_select`]; manual-code prompts
    /// through the dialog's manual input; an aborted signal rejects
    /// immediately.
    pub async fn show_auth_prompt(
        &self,
        dialog: &ComponentRef,
        prompt: &Value,
        provider_id: &str,
    ) -> Result<String, String> {
        let kind = prompt
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match kind {
            "select" => self.show_auth_select(dialog, prompt, provider_id).await,
            "manual_code" => {
                let message = prompt.get("message").cloned().unwrap_or(Value::Null);
                // `dialog.showManualInput(message)` — the resolution value is
                // unobservable in the oracle (silent stub); the port continues
                // with an empty code after the dialog call.
                self.io
                    .view
                    .update_component(dialog, "showManualInput", json!([message]));
                Ok(String::new())
            }
            _ => {
                let message = prompt.get("message").cloned().unwrap_or(Value::Null);
                let placeholder = prompt.get("placeholder").cloned().unwrap_or(Value::Null);
                self.io
                    .view
                    .update_component(dialog, "showPrompt", json!([message, placeholder]));
                Ok(String::new())
            }
        };
        let Some(signal) = prompt.get("signal") else {
            return response;
        };
        if signal.get("aborted").and_then(Value::as_bool) == Some(true) {
            return Err("Login cancelled".to_string());
        }
        response
    }

    // =========================================================================
    // Command handlers
    // =========================================================================

    /// Upstream `handleReloadCommand`.
    pub async fn handle_reload_command(&self) {
        if self.io.session.is_streaming() {
            self.show_warning("Wait for the current response to finish before reloading.");
            return;
        }
        if self.io.session.is_compacting() {
            self.show_warning("Wait for compaction to finish before reloading.");
            return;
        }

        self.reset_extension_ui();

        // The reload box is a detached local container (upstream builds a
        // `Container()` outside the typed container set, mounts it into
        // `editorContainer`, and focuses it). The choreography travels through
        // the raw emit pump (r21 seam disclosure — the detached-box seam).
        self.ev(json!(["Container.addChild", "container", { "kind": "DynamicBorder", "colorTag": "default" }]));
        self.ev(json!(["Container.addChild", "container", { "kind": "Spacer" }]));
        self.ev(json!(["Container.addChild", "container", {
            "kind": "Text",
            "text": self.fg("muted", "Reloading keybindings, extensions, skills, prompts, themes, and context files..."),
            "paddingX": 1,
            "paddingY": 0,
        }]));
        self.ev(json!(["Container.addChild", "container", { "kind": "Spacer" }]));
        self.ev(json!(["Container.addChild", "container", { "kind": "DynamicBorder", "colorTag": "default" }]));
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io.view.container_add_component(
            ContainerId::EditorContainer,
            &ComponentRef {
                kind: "reloadBox".to_string(),
                id: 0,
            },
        );
        self.io.view.set_focus(FocusTarget::Component(ComponentRef {
            kind: "reloadBox".to_string(),
            id: 0,
        }));
        self.io.view.request_render(Some(true));

        // `session.reload({ beforeSessionStart })` — the host hook rebuilds
        // the chat before the new session starts (the rebuild clears it).
        let reload_hook = || {
            self.rebuild_chat_from_messages();
        };
        let reloaded = self.io.session.reload(Some(&reload_hook)).await;
        match reloaded {
            Ok(()) => {
                self.ev(json!(["keybindings.reload"]));
                self.ev(json!(["setRegisteredThemes", []]));
                self.apply_runtime_settings();
                self.ev(json!(["themeController.applyFromSettings"]));
                self.setup_autocomplete_provider();
                self.setup_extension_shortcuts();
                self.show_loaded_resources(false, true);
                let saved_trust = self.maybe_save_implicit_project_trust_after_reload();
                if let Some(error) = self.io.session.model_runtime().get_error() {
                    self.show_error(&format!("models.json error: {error}"));
                }
                self.show_status(if saved_trust {
                    "Reloaded keybindings, extensions, skills, prompts, themes, and context files; saved project trust"
                } else {
                    "Reloaded keybindings, extensions, skills, prompts, themes, and context files"
                });
                self.restore_editor_in_container();
                self.io.view.request_render(None);
            }
            Err(error) => {
                self.restore_editor_in_container();
                self.io.view.request_render(None);
                self.show_error(&format!("Reload failed: {error}"));
            }
        }
    }

    /// Upstream `getPathCommandArgument`.
    pub fn get_path_command_argument(&self, text: &str, command: &str) -> Option<String> {
        if text == command {
            return None;
        }
        let prefix = format!("{command} ");
        if !text.starts_with(&prefix) {
            return None;
        }
        let args_string = text[command.len() + 1..].trim_start();
        if args_string.is_empty() {
            return None;
        }
        let first_char = args_string.chars().next()?;
        if first_char == '"' || first_char == '\'' {
            let closing = args_string[1..].find(first_char)?;
            return Some(args_string[1..1 + closing].to_string());
        }
        match args_string.find(char::is_whitespace) {
            Some(index) => Some(args_string[..index].to_string()),
            None => Some(args_string.to_string()),
        }
    }

    /// Upstream `handleExportCommand`.
    pub async fn handle_export_command(&self, text: &str) {
        let output_path = self.get_path_command_argument(text, "/export");
        match output_path {
            Some(path) if path.ends_with(".jsonl") => {
                match self.io.session.export_to_jsonl(&path) {
                    Ok(file_path) => self.show_status(&format!("Session exported to: {file_path}")),
                    Err(error) => self.show_error(&format!("Failed to export session: {error}")),
                }
            }
            other => {
                let theme_name = self
                    .theme()
                    .name
                    .clone()
                    .unwrap_or_else(|| "dark".to_string());
                match self
                    .io
                    .session
                    .export_to_html(other.as_deref(), &theme_name)
                    .await
                {
                    Ok(file_path) => self.show_status(&format!("Session exported to: {file_path}")),
                    Err(error) => self.show_error(&format!("Failed to export session: {error}")),
                }
            }
        }
    }

    /// Upstream `handleImportCommand`.
    pub async fn handle_import_command(&self, text: &str) {
        let input_path = self.get_path_command_argument(text, "/import");
        let Some(input_path) = input_path else {
            self.show_error("Usage: /import <path.jsonl>");
            return;
        };
        let confirmed = self
            .show_extension_confirm(
                "Import session",
                &format!("Replace current session with {input_path}?"),
            )
            .await;
        if !confirmed {
            self.show_status("Import cancelled");
            return;
        }
        self.clear_status_indicator(None);
        match self.io.host.import_from_jsonl(&input_path, None).await {
            Ok(result) => {
                if result.cancelled {
                    self.show_status("Import cancelled");
                    return;
                }
                self.show_status(&format!("Session imported from: {input_path}"));
            }
            Err(error) => {
                if let super::interactive_mode::HostError::MissingSessionCwd { fallback_cwd } =
                    &error
                {
                    let confirmed = self
                        .show_extension_confirm(
                            "Session cwd not found",
                            &format_missing_session_cwd_prompt(fallback_cwd),
                        )
                        .await;
                    if !confirmed {
                        self.show_status("Import cancelled");
                        return;
                    }
                    match self
                        .io
                        .host
                        .import_from_jsonl(&input_path, Some(fallback_cwd))
                        .await
                    {
                        Ok(result) => {
                            if result.cancelled {
                                self.show_status("Import cancelled");
                                return;
                            }
                            self.show_status(&format!("Session imported from: {input_path}"));
                        }
                        Err(error) => {
                            self.show_error(&format!("Failed to import session: {error}"));
                        }
                    }
                    return;
                }
                if let super::interactive_mode::HostError::ImportFileNotFound(message) = &error {
                    self.show_error(&format!("Failed to import session: {message}"));
                    return;
                }
                self.handle_fatal_runtime_error("Failed to import session", &error.to_string())
                    .await;
            }
        }
    }

    /// Upstream `handleShareCommand`. The share transport is the session-share
    /// slice (r16); the oracle records the options describe it is invoked
    /// with: the session collaborator, the ui handle, the containers, the
    /// editor and the status callbacks (r21 seam disclosure — the projection
    /// mirrors the recorded harness describe).
    pub async fn handle_share_command(&self) {
        let function_map = |keys: &[&str]| -> Value {
            Value::Object(
                keys.iter()
                    .map(|k| (k.to_string(), Value::String("function".to_string())))
                    .collect(),
            )
        };
        let session_describe = {
            let mut built = json!({
                "isStreaming": self.io.session.is_streaming(),
                "isCompacting": self.io.session.is_compacting(),
                "isBashRunning": self.io.session.is_bash_running(),
                "isIdle": self.io.session.is_idle(),
                "thinkingLevel": thinking_level_lower(&self.io.session.thinking_level()),
                "retryAttempt": self.io.session.retry_attempt(),
                "pendingMessageCount": self.io.session.pending_message_count(),
                "autoCompactionEnabled": self.io.session.auto_compaction_enabled(),
                "steeringMode": self.io.session.steering_mode(),
                "followUpMode": self.io.session.follow_up_mode(),
                "scopedModels": [],
                "promptTemplates": [],
                "state": { "messages": [] },
                "messages": [],
                "modelRuntime": function_map(&[
                    "getAvailableSnapshot", "getError", "getProviders",
                    "getProviderAuthStatus", "isUsingOAuth", "checkAuth", "getAuth",
                    "listCredentials", "logout", "login", "refresh",
                ]),
                "model": self.io.session.model().as_ref().map(|m| m.to_value()).unwrap_or(json!("undefined")),
            });
            let session = built.as_object_mut().expect("session describe");
            for (key, value) in [
                ("getSteeringMessages", "function"),
                ("getFollowUpMessages", "function"),
                ("clearQueue", "function"),
                ("prompt", "function"),
                ("steer", "function"),
                ("followUp", "function"),
                ("abort", "function"),
                ("abortBash", "function"),
                ("abortCompaction", "function"),
                ("abortRetry", "function"),
                ("abortBranchSummary", "function"),
                ("cycleThinkingLevel", "function"),
                ("cycleModel", "function"),
                ("getAvailableThinkingLevels", "function"),
                ("setThinkingLevel", "function"),
                ("setModel", "function"),
                ("setAutoCompactionEnabled", "function"),
                ("setSteeringMode", "function"),
                ("setFollowUpMode", "function"),
                ("setScopedModels", "function"),
                ("subscribe", "function"),
            ] {
                session.insert(key.to_string(), Value::String(value.to_string()));
            }
            session.insert(
                "extensionRunner".to_string(),
                function_map(&[
                    "getRegisteredCommands",
                    "getCommand",
                    "getCommandDiagnostics",
                    "getShortcutDiagnostics",
                    "getShortcuts",
                    "getMarkdownTransformers",
                    "getEntryRenderer",
                    "getMessageRenderer",
                    "getModelRegistry",
                    "emitUserBash",
                ]),
            );
            session.insert(
                "sessionManager".to_string(),
                function_map(&[
                    "getCwd",
                    "isPersisted",
                    "getSessionFile",
                    "getSessionId",
                    "getSessionDir",
                    "usesDefaultSessionDir",
                    "getSessionName",
                    "getEntries",
                    "buildContextEntries",
                    "getBranch",
                    "getTree",
                    "getLeafId",
                    "appendLabelChange",
                    "appendSessionInfo",
                ]),
            );
            session.insert(
                "settingsManager".to_string(),
                function_map(&[
                    "getQuietStartup",
                    "getShowTerminalProgress",
                    "getDoubleEscapeAction",
                    "getHideThinkingBlock",
                    "getShowCacheMissNotices",
                    "getCollapseChangelog",
                    "getOutputPad",
                    "getEditorPaddingX",
                    "getAutocompleteMaxVisible",
                    "getClearOnShrink",
                    "getShowHardwareCursor",
                    "getFullscreenScrollbar",
                    "getFullscreenCopyOnSelect",
                    "getFullscreenExitOutput",
                    "getCodeBlockIndent",
                    "getTerminalCapabilityOverrides",
                    "getHttpIdleTimeoutMs",
                    "getMermaidRenderingMode",
                    "getEnableSkillCommands",
                    "getLastChangelogVersion",
                    "getShowImages",
                    "getImageWidthCells",
                    "isProjectTrusted",
                    "getTheme",
                    "getDefaultProvider",
                    "getDefaultModel",
                    "getDefaultThinkingLevel",
                    "getAllModelThinkingLevels",
                    "getEnabledModels",
                    "getBranchSummarySkipPrompt",
                    "getExternalEditorCommand",
                    "getWarnings",
                    "getImageAutoResize",
                    "getBlockImages",
                    "getTransport",
                    "getDefaultProjectTrust",
                    "getTreeFilterMode",
                    "getEnableInstallTelemetry",
                    "setLastChangelogVersion",
                    "setHideThinkingBlock",
                    "setShowImages",
                    "setImageWidthCells",
                    "setImageAutoResize",
                    "setBlockImages",
                    "setEnableSkillCommands",
                    "setTransport",
                    "setHttpIdleTimeoutMs",
                    "setModelThinkingLevel",
                    "removeModelThinkingLevel",
                    "setTheme",
                    "setMermaidRenderingMode",
                    "setShowCacheMissNotices",
                    "setCollapseChangelog",
                    "setEnableInstallTelemetry",
                    "setQuietStartup",
                    "setDefaultProjectTrust",
                    "setDoubleEscapeAction",
                    "setTreeFilterMode",
                    "setShowHardwareCursor",
                    "setEditorPaddingX",
                    "setOutputPad",
                    "setAutocompleteMaxVisible",
                    "setClearOnShrink",
                    "setShowTerminalProgress",
                    "setTuiMode",
                    "setFullscreenExitOutput",
                    "setFullscreenScrollbar",
                    "setFullscreenCopyOnSelect",
                    "setWarnings",
                    "setEnabledModels",
                ]),
            );
            session.insert(
                "resourceLoader".to_string(),
                function_map(&[
                    "getSkills",
                    "getPrompts",
                    "getThemes",
                    "getExtensions",
                    "getSystemPromptSource",
                    "getAppendSystemPromptSources",
                    "getAgentsFiles",
                ]),
            );
            session.insert(
                "agent".to_string(),
                json!({ "abort": "function", "signal": {} }),
            );
            for (key, value) in [
                ("bindExtensions", "function"),
                ("waitForIdle", "function"),
                ("navigateTree", "function"),
                ("reload", "function"),
                ("compact", "function"),
                ("executeBash", "function"),
                ("recordBashResult", "function"),
                ("getUserMessagesForForking", "function"),
                ("getSessionStats", "function"),
                ("getLastAssistantText", "function"),
                ("setSessionName", "function"),
                ("getToolDefinition", "function"),
                ("getContextUsage", "function"),
            ] {
                session.insert(key.to_string(), Value::String(value.to_string()));
            }
            session.insert(
                "systemPrompt".to_string(),
                Value::String(self.io.session.system_prompt()),
            );
            session.insert(
                "isProjectTrusted".to_string(),
                Value::String("function".to_string()),
            );
            built
        };
        let options = json!({
            "session": session_describe,
            "ui": { "kind": "ui" },
            "editorContainer": { "container": "editorContainer", "children": [] },
            "editor": {
                "_editorName": "<state>", "_state": "<state>",
                "onEscape": "undefined", "onCtrlD": "undefined", "onSubmit": "undefined",
                "onChange": "undefined", "onPasteImage": "undefined",
                "onExtensionShortcut": "undefined", "embedWorkingStatus": true,
                "actionHandlers": {},
                "getText": "function", "getExpandedText": "function", "setText": "function",
                "addToHistory": "function", "insertTextAtCursor": "function",
                "handleInput": "function", "setWorkingStatusIndicator": "function",
                "setAutocompleteProvider": "function", "setPaddingX": "function",
                "getPaddingX": "function", "setAutocompleteMaxVisible": "function",
                "getAutocompleteMaxVisible": "function", "onAction": "function",
            },
            "showStatus": "function",
            "showError": "function",
        });
        self.ev(json!(["shareSession", options]));
    }

    /// Upstream `handleCopyCommand`.
    pub async fn handle_copy_command(&self, flash_confirmation: bool, prefer_selection: bool) {
        if prefer_selection
            && self.io.view.has_active_selection()
            && !self.io.view.get_copy_on_select()
        {
            self.ev(json!(["ui.copyActiveSelectionToClipboard"]));
            return;
        }
        let Some(text) = self.io.session.last_assistant_text() else {
            self.show_error("No agent messages to copy yet.");
            return;
        };
        match self.io.platform.copy_to_clipboard(&text).await {
            Ok(()) => {
                // The flash confirmation rides the alt-screen renderer
                // (`ui instanceof TuiAltScreen` upstream); the regular-mode
                // oracle path always takes the status message.
                if flash_confirmation && self.options().tui_mode.as_deref() == Some("fullscreen") {
                    self.ev(json!(["ui.flash", "Copied!"]));
                } else {
                    self.show_status("Copied last agent message to clipboard");
                }
            }
            Err(error) => self.show_error(&error),
        }
    }

    /// Upstream `handleNameCommand`.
    pub fn handle_name_command(&self, text: &str) {
        let name = text.trim_start_matches("/name").trim();
        if name.is_empty() {
            let current_name = self.io.session_manager.session_name();
            if let Some(current_name) = current_name {
                self.io.view.container_add_spacer(ContainerId::Chat);
                self.io.view.container_add_text(
                    ContainerId::Chat,
                    &self.fg("dim", &format!("Session name: {current_name}")),
                    1,
                    0,
                    false,
                );
            } else {
                self.show_warning("Usage: /name <name>");
            }
            self.io.view.request_render(None);
            return;
        }
        self.io.session.set_session_name(name);
        let session_name = self.io.session_manager.session_name();
        if session_name.as_deref() != Some(name) {
            self.show_warning(&format!(
                "Session name was normalized from {} to {}",
                serde_json::to_string(name).unwrap_or_default(),
                match &session_name {
                    Some(value) => serde_json::to_string(value).unwrap_or_default(),
                    None => "undefined".to_string(),
                },
            ));
        }
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_text(
            ContainerId::Chat,
            &self.fg(
                "dim",
                &format!(
                    "Session name set: {}",
                    session_name.unwrap_or_else(|| name.to_string())
                ),
            ),
            1,
            0,
            false,
        );
        self.io.view.request_render(None);
    }

    /// Upstream `handleSessionCommand`.
    pub fn handle_session_command(&self) {
        let stats = self.io.session.session_stats();
        let session_name = self.io.session_manager.session_name();
        let waste = self.io.session_cache_waste();
        let breakdown = self.io.session_usage_breakdown();

        let mut info = format!("{}\n\n", self.theme().bold("Session Info"));
        if let Some(session_name) = &session_name {
            info += &format!("{} {session_name}\n", self.fg("dim", "Name:"));
        }
        info += &format!(
            "{} {}\n",
            self.fg("dim", "File:"),
            stats
                .session_file
                .clone()
                .unwrap_or_else(|| "In-memory".to_string())
        );
        info += &format!("{} {}\n\n", self.fg("dim", "ID:"), stats.session_id);
        info += &format!("{}\n", self.theme().bold("Messages"));
        info += &format!("{} {}\n", self.fg("dim", "Total:"), stats.total_messages);
        info += &format!("{} {}\n", self.fg("dim", "User:"), stats.user_messages);
        info += &format!(
            "{} {}\n",
            self.fg("dim", "Assistant:"),
            stats.assistant_messages
        );
        info += &format!(
            "{} {} calls, {} results\n\n",
            self.fg("dim", "Tools:"),
            stats.tool_calls,
            stats.tool_results
        );
        info += &format!("{}\n", self.theme().bold("Tokens"));
        let prompt_tokens = stats.tokens_input + stats.tokens_cache_read + stats.tokens_cache_write;
        info += &format!(
            "{} {}\n",
            self.fg("dim", "Input:"),
            format_us_int(prompt_tokens)
        );
        if prompt_tokens > 0 && (stats.tokens_cache_read > 0 || stats.tokens_cache_write > 0) {
            let hit_rate = self.fg(
                "dim",
                &format!(
                    "({:.1}%)",
                    (stats.tokens_cache_read as f64 / prompt_tokens as f64) * 100.0
                ),
            );
            info += &format!(
                "  {} {} {hit_rate}\n",
                self.fg("dim", "Cached:"),
                format_us_int(stats.tokens_cache_read)
            );
            let written = if stats.tokens_cache_write > 0 {
                format!(
                    " {}",
                    self.fg(
                        "dim",
                        &format!(
                            "({} written to cache)",
                            format_us_int(stats.tokens_cache_write)
                        )
                    )
                )
            } else {
                String::new()
            };
            info += &format!(
                "  {} {}{written}\n",
                self.fg("dim", "Uncached:"),
                format_us_int(stats.tokens_input + stats.tokens_cache_write)
            );
        }
        info += &format!(
            "{} {}\n",
            self.fg("dim", "Output:"),
            format_us_int(stats.tokens_output)
        );
        info += &format!(
            "{} {}\n",
            self.fg("dim", "Total:"),
            format_us_int(stats.tokens_total)
        );

        if stats.cost > 0.0 || waste.missed_tokens > 0 {
            info += &format!("\n{}\n", self.theme().bold("Cost"));
            info += &format!("{} ${:.3}", self.fg("dim", "Total:"), stats.cost);
            if breakdown.len() > 1 {
                for entry in &breakdown {
                    info += &format!(
                        "\n  {} ${:.3} {}",
                        self.fg("dim", &format!("{}:", entry.key)),
                        entry.cost,
                        self.fg(
                            "dim",
                            &format!(
                                "({} tokens)",
                                super::interactive_mode::format_tokens(entry.tokens as f64)
                            )
                        )
                    );
                }
            }
            if waste.missed_tokens > 0 {
                let miss_label = if waste.miss_count == 1 {
                    "1 miss".to_string()
                } else {
                    format!("{} misses", waste.miss_count)
                };
                let detail = format!(
                    "{} tokens, {miss_label}",
                    format_us_int(waste.missed_tokens)
                );
                info += &format!(
                    "\n{} ${:.3} {}",
                    self.fg("dim", "Cache Re-billed:"),
                    waste.missed_cost,
                    self.fg("dim", &format!("({detail})"))
                );
            }
        }

        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io
            .view
            .container_add_text(ContainerId::Chat, &info, 1, 0, false);
        self.io.view.request_render(None);
    }

    /// Upstream `handleChangelogCommand`.
    pub fn handle_changelog_command(&self) {
        let all_entries = self.io.changelog.entries();
        let changelog_markdown = if all_entries.is_empty() {
            "No changelog entries found.".to_string()
        } else {
            all_entries
                .iter()
                .rev()
                .map(|(version, content)| {
                    let normalized = self.io.changelog.normalize_links(content);
                    let _ = version;
                    normalized
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_border(ContainerId::Chat, None);
        self.io.view.container_add_text(
            ContainerId::Chat,
            &self.theme().bold(&self.fg("accent", "What's New")),
            1,
            0,
            false,
        );
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_markdown(
            ContainerId::Chat,
            &changelog_markdown,
            1,
            1,
            &self.get_markdown_theme_with_settings(),
        );
        self.io.view.container_add_border(ContainerId::Chat, None);
        self.io.view.request_render(None);
    }

    /// Upstream `handleHotkeysCommand`.
    pub fn handle_hotkeys_command(&self) {
        let kd = |action: &str| self.get_app_key_display(action);
        let hotkeys = format!(
            "\n**Navigation**\n| Key | Action |\n|-----|--------|\n| `{}` / `{}` / `{}` / `{}` | Move cursor / browse history |\n| `{}` / `{}` | Move by word |\n| `{}` | Start of line |\n| `{}` | End of line |\n| `{}` | Jump forward to character |\n| `{}` | Jump backward to character |\n| `{}` / `{}` | Scroll by page |\n\n**Editing**\n| Key | Action |\n|-----|--------|\n| `{}` | Send message |\n| `{}` | New line{} |\n| `{}` | Delete word backwards |\n| `{}` | Delete word forwards |\n| `{}` | Delete to start of line |\n| `{}` | Delete to end of line |\n| `{}` | Paste the most-recently-deleted text |\n| `{}` | Cycle through the deleted text after pasting |\n| `{}` | Undo |\n\n**Other**\n| Key | Action |\n|-----|--------|\n| `{}` | Path completion / accept autocomplete |\n| `{}` | Cancel autocomplete / abort streaming |\n| `{}` | Clear editor (first) / exit (second) |\n| `{}` | Exit (when editor is empty) |\n| `{}` | Suspend to background |\n| `{}` | Cycle thinking level |\n| `{}` / `{}` | Cycle models |\n| `{}` | Open model selector |\n| `{}` | Toggle tool output expansion |\n| `{}` | Toggle thinking block visibility |\n| `{}` | Edit message in external editor |\n| `{}` | Copy last assistant message |\n| `{}` | Queue follow-up message |\n| `{}` | Restore queued messages |\n| `{}` | Paste image or text from clipboard |\n| `/` | Slash commands |\n| `!` | Run bash command |\n| `!!` | Run bash command (excluded from context) |\n",
            kd("tui.editor.cursorUp"),
            kd("tui.editor.cursorDown"),
            kd("tui.editor.cursorLeft"),
            kd("tui.editor.cursorRight"),
            kd("tui.editor.cursorWordLeft"),
            kd("tui.editor.cursorWordRight"),
            kd("tui.editor.cursorLineStart"),
            kd("tui.editor.cursorLineEnd"),
            kd("tui.editor.jumpForward"),
            kd("tui.editor.jumpBackward"),
            kd("tui.editor.pageUp"),
            kd("tui.editor.pageDown"),
            kd("tui.input.submit"),
            kd("tui.input.newLine"),
            if self.io.platform.is_windows() {
                " (Ctrl+Enter on Windows Terminal)"
            } else {
                ""
            },
            kd("tui.editor.deleteWordBackward"),
            kd("tui.editor.deleteWordForward"),
            kd("tui.editor.deleteToLineStart"),
            kd("tui.editor.deleteToLineEnd"),
            kd("tui.editor.yank"),
            kd("tui.editor.yankPop"),
            kd("tui.editor.undo"),
            kd("tui.input.tab"),
            kd("app.interrupt"),
            kd("app.clear"),
            kd("app.exit"),
            kd("app.suspend"),
            kd("app.thinking.cycle"),
            kd("app.model.cycleForward"),
            kd("app.model.cycleBackward"),
            kd("app.model.select"),
            kd("app.tools.expand"),
            kd("app.thinking.toggle"),
            kd("app.editor.external"),
            kd("app.message.copy"),
            kd("app.message.followUp"),
            kd("app.message.dequeue"),
            kd("app.clipboard.pasteImage"),
        );
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_border(ContainerId::Chat, None);
        self.io.view.container_add_text(
            ContainerId::Chat,
            &self.theme().bold(&self.fg("accent", "Keyboard Shortcuts")),
            1,
            0,
            false,
        );
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_markdown(
            ContainerId::Chat,
            hotkeys.trim(),
            1,
            1,
            &self.get_markdown_theme_with_settings(),
        );
        self.io.view.container_add_border(ContainerId::Chat, None);
        self.io.view.request_render(None);
    }

    /// Upstream `handleDebugCommand`.
    pub fn handle_debug_command(&self) {
        let (width, height, all_lines) = self.io.view.debug_render();
        let mut parts: Vec<String> = vec![
            format!("Debug output at {}", self.io.platform.now_iso()),
            format!("Terminal: {width}x{height}"),
            format!("Total lines: {}", all_lines.len()),
            String::new(),
            "=== All rendered lines with visible widths ===".to_string(),
        ];
        for (idx, line) in all_lines.iter().enumerate() {
            let escaped = serde_json::to_string(line).unwrap_or_default();
            parts.push(format!("[{idx}] (w={}) {escaped}", line.chars().count()));
        }
        parts.push(String::new());
        parts.push("=== Agent messages (JSONL) ===".to_string());
        for message in self.io.session.messages() {
            parts.push(serde_json::to_string(&message).unwrap_or_default());
        }
        parts.push(String::new());
        let debug_data = parts.join("\n");
        // `fs.mkdirSync(os.tmpdir())` — the parent directory of the log path.
        let debug_dir = self
            .io
            .debug_log_path
            .rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_else(|| self.io.debug_log_path.clone());
        self.ev(json!(["fs.mkdirSync", debug_dir]));
        self.ev(json!([
            "fs.writeFileSync",
            self.io.debug_log_path,
            debug_data
        ]));
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io.view.container_add_text(
            ContainerId::Chat,
            &format!(
                "{}\n{}",
                self.fg("accent", "✓ Debug log written"),
                self.fg("muted", &self.io.debug_log_path)
            ),
            1,
            1,
            false,
        );
        self.io.view.request_render(None);
    }

    /// Upstream `handleClearCommand`.
    pub async fn handle_clear_command(&self) {
        self.clear_status_indicator(None);
        match self.io.host.new_session(Value::Null).await {
            Ok(result) => {
                if result.cancelled {
                    return;
                }
                self.io.view.container_add_spacer(ContainerId::Chat);
                self.io.view.container_add_text(
                    ContainerId::Chat,
                    &self.fg("accent", "✓ New session started"),
                    1,
                    1,
                    false,
                );
                self.io.view.request_render(None);
            }
            Err(error) => {
                self.handle_fatal_runtime_error("Failed to create session", &error)
                    .await;
            }
        }
    }

    /// Upstream `handleCompactCommand`.
    pub async fn handle_compact_command(&self, custom_instructions: Option<&str>) {
        self.clear_status_indicator(None);
        let _ = self.io.session.compact(custom_instructions).await;
    }

    /// Upstream `handleBashCommand`.
    pub async fn handle_bash_command(&self, command: &str, exclude_from_context: bool) {
        let event_result = self
            .io
            .session
            .shortcuts()
            .emit_user_bash(
                command,
                exclude_from_context,
                &self.io.session_manager.cwd(),
            )
            .await;
        let event_result = match event_result {
            Ok(result) => result,
            Err(_) => {
                // The extension runner already reported the error. Do not fall
                // back to local execution.
                return;
            }
        };

        // If the extension returned a full result, use it directly.
        let UserBashOutcome { result } = event_result;
        if let Some(result) = result {
            let component = self.io.view.new_component(
                ComponentKind::BashExecution,
                json!([command, { "__describe": "ui" }, exclude_from_context]),
            );
            if self.io.session.is_streaming() {
                self.io
                    .view
                    .container_add_component(ContainerId::PendingMessages, &component);
                self.lock().pending_bash_components.push(component.clone());
            } else {
                self.io
                    .view
                    .container_add_component(ContainerId::Chat, &component);
            }
            if !result.output.is_empty() {
                self.io.view.update_component(
                    &component,
                    "appendOutput",
                    Value::String(result.output.clone()),
                );
            }
            self.io.view.update_component(
                &component,
                "setComplete",
                json!([
                    result
                        .exit_code
                        .map(Value::from)
                        .unwrap_or(json!("undefined")),
                    result.cancelled,
                    "undefined",
                    result
                        .full_output_path
                        .clone()
                        .map(Value::from)
                        .unwrap_or(json!("undefined")),
                ]),
            );
            self.io
                .session
                .record_bash_result(command, &result, exclude_from_context);
            self.io.view.request_render(None);
            return;
        }

        let is_deferred = self.io.session.is_streaming();
        let component = self.io.view.new_component(
            ComponentKind::BashExecution,
            json!([command, { "__describe": "ui" }, exclude_from_context]),
        );
        if is_deferred {
            self.io
                .view
                .container_add_component(ContainerId::PendingMessages, &component);
            self.lock().pending_bash_components.push(component.clone());
        } else {
            self.io
                .view
                .container_add_component(ContainerId::Chat, &component);
        }
        self.io.view.request_render(None);

        let component_for_chunks = component.clone();
        let chunk_sink = move |chunk: &str| {
            let _ = &component_for_chunks;
            let _ = chunk;
        };
        match self
            .io
            .session
            .execute_bash(command, exclude_from_context, &chunk_sink)
            .await
        {
            Ok(result) => {
                self.io.view.update_component(
                    &component,
                    "setComplete",
                    json!([
                        result
                            .exit_code
                            .map(Value::from)
                            .unwrap_or(json!("undefined")),
                        result.cancelled,
                        "undefined",
                        result
                            .full_output_path
                            .clone()
                            .map(Value::from)
                            .unwrap_or(json!("undefined")),
                    ]),
                );
            }
            Err(error) => {
                self.io.view.update_component(
                    &component,
                    "setComplete",
                    json!(["undefined", false]),
                );
                self.show_error(&format!("Bash command failed: {error}"));
            }
        }
        self.io.view.request_render(None);
    }

    /// Upstream `handleArminSaysHi`.
    pub fn handle_armin_says_hi(&self) {
        self.io.view.container_add_spacer(ContainerId::Chat);
        let component = self
            .io
            .view
            .new_component(ComponentKind::Armin, json!([{ "__describe": "ui" }]));
        self.io
            .view
            .container_add_component(ContainerId::Chat, &component);
        self.io.view.request_render(None);
    }

    /// Upstream `handleDementedDelves`.
    pub fn handle_demented_delves(&self) {
        self.io.view.container_add_spacer(ContainerId::Chat);
        let component = self
            .io
            .view
            .new_component(ComponentKind::Earendil, json!([]));
        self.io
            .view
            .container_add_component(ContainerId::Chat, &component);
        self.io.view.request_render(None);
    }

    // =========================================================================
    // Extension UI event bridge (S8)
    // =========================================================================

    /// Upstream `showExtensionSelector` — resolves to the chosen option
    /// (`None` on cancel/abort).
    pub async fn extension_selector_choice(
        &self,
        title: &str,
        options: &[String],
        timeout: Option<u64>,
    ) -> Option<String> {
        let _ = timeout;
        if self
            .lock()
            .extension_dialog
            .as_ref()
            .is_some_and(|d| d.aborted)
        {
            return None;
        }
        let component = self.io.view.new_component(
            ComponentKind::ExtensionSelectorDialog,
            json!([
                title,
                options,
                "function",
                "function",
                {
                    "tui": { "__describe": "ui" },
                    "timeout": timeout.map(Value::from).unwrap_or(json!("undefined")),
                    "onToggleToolsExpanded": "function",
                },
            ]),
        );
        self.dispose_active_selector();
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);

        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_selector_active = true;
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: component.clone(),
            aborted: false,
            resolve: tx,
        });
        let choice = rx.await.ok().flatten();
        // The close path disposes the component and restores the editor
        // (upstream `hideExtensionSelector`; run on the awaiting task so the
        // log order is deterministic).
        self.io
            .view
            .update_component(&component, "dispose", Value::Null);
        self.hide_extension_selector();
        choice
    }

    /// Upstream `hideExtensionSelector` (component dispose + editor restore).
    pub fn hide_extension_selector(&self) {
        self.lock().extension_selector_active = false;
        self.restore_editor_in_container();
        self.io.view.request_render(None);
    }

    /// Resolves the pending extension dialog (selector choice / input value /
    /// editor text; `None` = cancelled). The awaiting shell method owns the
    /// close choreography.
    pub fn resolve_extension_dialog(&self, value: Option<String>) {
        let dialog = self.lock().extension_dialog.take();
        if let Some(dialog) = dialog {
            let _ = dialog.resolve.send(value);
        }
    }

    /// Upstream `showExtensionConfirm`.
    pub async fn show_extension_confirm(&self, title: &str, message: &str) -> bool {
        let result = self
            .extension_selector_choice(
                &format!("{title}\n{message}"),
                &["Yes".to_string(), "No".to_string()],
                None,
            )
            .await;
        result.as_deref() == Some("Yes")
    }

    /// Upstream `showExtensionInput`.
    pub async fn show_extension_input(
        &self,
        title: &str,
        placeholder: Option<&str>,
    ) -> Option<String> {
        let component = self.io.view.new_component(
            ComponentKind::ExtensionInputDialog,
            json!([title, placeholder, "function", "function", { "tui": { "__describe": "ui" }, "timeout": "undefined" }]),
        );
        self.dispose_active_selector();
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_input_active = true;
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: component.clone(),
            aborted: false,
            resolve: tx,
        });
        let value = rx.await.ok().flatten();
        self.io
            .view
            .update_component(&component, "dispose", Value::Null);
        self.hide_extension_selector();
        value
    }

    /// Upstream `hideExtensionInput`.
    pub fn hide_extension_input(&self) {
        self.lock().extension_input_active = false;
        self.restore_editor_in_container();
        self.io.view.request_render(None);
    }

    /// Upstream `showExtensionEditor`.
    pub async fn show_extension_editor(
        &self,
        title: &str,
        prefill: Option<&str>,
    ) -> Option<String> {
        let component = self.io.view.new_component(
            ComponentKind::ExtensionEditorDialog,
            json!([
                { "__describe": "ui" },
                {
                    "getKeys": "function", "getEffectiveConfig": "function",
                    "reload": "function",
                },
                title,
                prefill.map(Value::from).unwrap_or(json!("undefined")),
                "function",
                "function",
                "undefined",
                "vim",
            ]),
        );
        self.dispose_active_selector();
        self.io.view.container_clear(ContainerId::EditorContainer);
        self.io
            .view
            .container_add_component(ContainerId::EditorContainer, &component);
        self.io
            .view
            .set_focus(FocusTarget::Component(component.clone()));
        self.io.view.request_render(None);
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_editor_active = true;
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: component.clone(),
            aborted: false,
            resolve: tx,
        });
        let value = rx.await.ok().flatten();
        // Upstream `hideExtensionEditor`: restore without a component dispose.
        self.hide_extension_editor();
        value
    }

    /// Upstream `hideExtensionEditor`.
    pub fn hide_extension_editor(&self) {
        self.lock().extension_editor_active = false;
        self.restore_editor_in_container();
        self.io.view.request_render(None);
    }

    /// Upstream `showExtensionCustom`. The factory/component mount is the r19
    /// component seam; every oracle close lands before the mount, so the port
    /// awaits the close and restores (non-overlay) or hides (overlay).
    pub async fn show_extension_custom(&self, overlay: bool) -> Option<String> {
        let saved_text = self.editor().get_text();
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        self.lock().extension_dialog = Some(ExtensionDialog {
            component: ComponentRef {
                kind: "CustomExt".to_string(),
                id: 0,
            },
            aborted: false,
            resolve: tx,
        });
        let result = rx.await.ok().flatten();
        if overlay {
            self.io.view.hide_overlay();
        } else {
            self.io.view.container_clear(ContainerId::EditorContainer);
            self.io.view.container_add_component(
                ContainerId::EditorContainer,
                &ComponentRef {
                    kind: "editor".to_string(),
                    id: 0,
                },
            );
            self.editor().set_text(&saved_text);
            self.io.view.set_focus(FocusTarget::Editor);
            self.io.view.request_render(None);
        }
        result
    }

    /// Upstream `showExtensionNotify`.
    pub fn show_extension_notify(&self, message: &str, kind: Option<&str>) {
        match kind {
            Some("error") => self.show_error(message),
            Some("warning") => self.show_warning(message),
            _ => self.show_status(message),
        }
    }

    /// Upstream `showExtensionError`.
    pub fn show_extension_error(&self, extension_path: &str, error: &str, stack: Option<&str>) {
        let text = self.fg(
            "error",
            &format!("Extension \"{extension_path}\" error: {error}"),
        );
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
        if let Some(stack) = stack {
            let lines: Vec<String> = stack
                .split('\n')
                .skip(1)
                .map(|line| self.fg("dim", &format!("  {}", line.trim())))
                .collect();
            if !lines.is_empty() {
                self.io
                    .view
                    .container_add_text(ContainerId::Chat, &lines.join("\n"), 1, 0, false);
            }
        }
        self.io.view.request_render(None);
    }

    /// Upstream `setExtensionWidget`.
    pub fn set_extension_widget(
        &self,
        key: &str,
        content: Option<WidgetContent>,
        placement: WidgetPlacement,
    ) {
        {
            let mut state = self.lock();
            state.extension_widgets_above.retain(|(k, _)| k != key);
            state.extension_widgets_below.retain(|(k, _)| k != key);
        }
        if let Some(content) = content {
            let component = match content {
                WidgetContent::Lines(lines) => {
                    // Upstream builds a fresh detached  (named
                    // "container") of Text rows per call; the recorded
                    // choreography is the row addChilds on the raw pump, and
                    // the box object rides as an unrecorded-handle component.
                    for line in lines.iter().take(MAX_WIDGET_LINES) {
                        self.ev(json!([
                            "Container.addChild",
                            "container",
                            {
                                "kind": "Text",
                                "text": line,
                                "paddingX": 1,
                                "paddingY": 0,
                            }
                        ]));
                    }
                    if lines.len() > MAX_WIDGET_LINES {
                        let truncated = self.fg("muted", "... (widget truncated)");
                        self.ev(json!([
                            "Container.addChild",
                            "container",
                            {
                                "kind": "Text",
                                "text": truncated,
                                "paddingX": 1,
                                "paddingY": 0,
                            }
                        ]));
                    }
                    ComponentRef {
                        kind: "widgetBox".to_string(),
                        id: 0,
                    }
                }
                WidgetContent::Component(component) => component,
            };
            match placement {
                WidgetPlacement::AboveEditor => {
                    self.lock()
                        .extension_widgets_above
                        .push((key.to_string(), component));
                }
                WidgetPlacement::BelowEditor => {
                    self.lock()
                        .extension_widgets_below
                        .push((key.to_string(), component));
                }
            }
        }
        self.render_widgets();
    }

    /// Upstream `clearExtensionWidgets`.
    pub fn clear_extension_widgets(&self) {
        {
            let mut state = self.lock();
            state.extension_widgets_above.clear();
            state.extension_widgets_below.clear();
        }
        self.render_widgets();
    }

    /// Upstream `renderWidgets`.
    pub fn render_widgets(&self) {
        let (above, below) = {
            let state = self.lock();
            (
                state
                    .extension_widgets_above
                    .iter()
                    .map(|(_, c)| c.clone())
                    .collect::<Vec<_>>(),
                state
                    .extension_widgets_below
                    .iter()
                    .map(|(_, c)| c.clone())
                    .collect::<Vec<_>>(),
            )
        };
        self.render_widget_container(ContainerId::WidgetsAbove, &above, true, true);
        self.render_widget_container(ContainerId::WidgetsBelow, &below, false, false);
        self.io.view.request_render(None);
    }

    /// Upstream `renderWidgetContainer`.
    fn render_widget_container(
        &self,
        container: ContainerId,
        widgets: &[ComponentRef],
        spacer_when_empty: bool,
        leading_spacer: bool,
    ) {
        self.io.view.container_clear(container);
        if widgets.is_empty() {
            if spacer_when_empty {
                self.io.view.container_add_spacer(container);
            }
            return;
        }
        if leading_spacer {
            self.io.view.container_add_spacer(container);
        }
        for component in widgets {
            self.io.view.container_add_component(container, component);
        }
    }

    /// Upstream `setExtensionFooter`.
    pub fn set_extension_footer(&self, custom: Option<ComponentRef>) {
        // Dispose the existing custom footer before the swap.
        if let Some(previous) = self.lock().custom_footer.clone() {
            self.io
                .view
                .update_component(&previous, "dispose", Value::Null);
        }
        self.io.view.container_clear(ContainerId::FooterContainer);
        match custom {
            Some(component) => {
                self.io
                    .view
                    .container_add_component(ContainerId::FooterContainer, &component);
                self.lock().custom_footer = Some(component);
                self.lock().custom_footer_active = true;
            }
            None => {
                self.lock().custom_footer = None;
                self.io.view.container_add_component(
                    ContainerId::FooterContainer,
                    &ComponentRef {
                        kind: "footer".to_string(),
                        id: 0,
                    },
                );
                self.lock().custom_footer_active = false;
            }
        }
        self.io.view.request_render(None);
    }

    /// Upstream `setExtensionHeader`.
    pub fn set_extension_header(&self, custom: Option<ComponentRef>) {
        // Header may not be initialized yet during early initialization.
        let Some(built_in) = self.lock().built_in_header.clone() else {
            return;
        };
        // Dispose the existing custom header before the swap.
        let previous = self.lock().custom_header.clone();
        if let Some(previous) = &previous {
            self.io
                .view
                .update_component(previous, "dispose", Value::Null);
        }
        // Upstream locates the CURRENT header (custom when present).
        let current = previous.unwrap_or_else(|| built_in.clone());
        let index = self.header_index_of(&current);
        match custom {
            Some(component) => {
                self.io.view.update_component(
                    &component,
                    "setExpanded",
                    Value::Bool(self.lock().tool_output_expanded),
                );
                if index != usize::MAX {
                    // Upstream mutates children[index] directly — unrecorded.
                    self.io.view.container_replace_child_unrecorded(
                        ContainerId::Header,
                        index,
                        &component,
                    );
                } else {
                    self.io.view.header_unshift(&component);
                }
                self.lock().custom_header = Some(component);
            }
            None => {
                self.io.view.update_component(
                    &built_in,
                    "setExpanded",
                    Value::Bool(self.lock().tool_output_expanded),
                );
                if index != usize::MAX {
                    self.io.view.container_replace_child_unrecorded(
                        ContainerId::Header,
                        index,
                        &built_in,
                    );
                }
                self.lock().custom_header = None;
            }
        }
        self.io.view.request_render(None);
    }

    /// `headerContainer.children.indexOf(current)`.
    fn header_index_of(&self, component: &ComponentRef) -> usize {
        self.io
            .view
            .container_components(ContainerId::Header)
            .iter()
            .position(|c| c == component)
            .unwrap_or(usize::MAX)
    }

    /// Upstream `addExtensionTerminalInputListener`.
    pub fn add_extension_terminal_input_listener(&self) -> u64 {
        let id = self.io.view.add_input_listener();
        self.lock().extension_terminal_input_subscriptions.push(id);
        id
    }

    /// Upstream `rebindExtensionTerminalInputListeners`.
    pub fn rebind_extension_terminal_input_listeners(&self) {
        let mut state = self.lock();
        for id in state.extension_terminal_input_subscriptions.iter_mut() {
            self.io.view.remove_input_listener(*id);
            *id = self.io.view.add_input_listener();
        }
    }

    /// Upstream `clearExtensionTerminalInputListeners`.
    pub fn clear_extension_terminal_input_listeners(&self) {
        let ids: Vec<u64> = std::mem::take(&mut self.lock().extension_terminal_input_subscriptions);
        for id in ids {
            self.io.view.remove_input_listener(id);
        }
    }

    /// Upstream `createExtensionUIContext` — the describe projection of the
    /// context object (S8; every member is a function).
    pub fn extension_ui_context(&self) -> Value {
        json!({
            "select": "function", "confirm": "function", "input": "function",
            "notify": "function", "onTerminalInput": "function", "setStatus": "function",
            "setWorkingMessage": "function", "setWorkingVisible": "function",
            "setWorkingIndicator": "function", "setHiddenThinkingLabel": "function",
            "setWidget": "function", "setFooter": "function", "setHeader": "function",
            "setTitle": "function", "custom": "function", "pasteToEditor": "function",
            "setEditorText": "function", "getEditorText": "function", "editor": "function",
            "addAutocompleteProvider": "function", "setEditorComponent": "function",
            "getEditorComponent": "function", "theme": {}, "getAllThemes": "function",
            "getTheme": "function", "setTheme": "function",
            "getToolsExpanded": "function", "setToolsExpanded": "function",
        })
    }

    /// Upstream `setCustomEditorComponent`.
    pub fn set_custom_editor_component(&self, custom: Option<Arc<dyn ShellEditor>>) {
        let current_text = self.editor().get_text();
        self.dispose_active_selector();
        self.io.view.container_clear(ContainerId::EditorContainer);
        match custom {
            Some(custom_editor) => {
                custom_editor.set_text(&current_text);
                // The swap wires the padding/autocomplete sizing immediately
                // (the border color follows the shared update path).
                custom_editor.set_padding_x(self.io.settings.editor_padding_x());
                custom_editor
                    .set_autocomplete_max_visible(self.io.settings.autocomplete_max_visible());
                {
                    let mut state = self.lock();
                    state.editor_is_custom = true;
                    state.custom_editor = Some(custom_editor);
                }
            }
            None => {
                self.io.default_editor.set_text(&current_text);
                {
                    let mut state = self.lock();
                    state.editor_is_custom = false;
                    state.custom_editor = None;
                }
            }
        }
        self.io.view.container_add_component(
            ContainerId::EditorContainer,
            &ComponentRef {
                kind: if self.lock().editor_is_custom {
                    "customEditor".to_string()
                } else {
                    "editor".to_string()
                },
                id: 0,
            },
        );
        if let Some((_, indicator)) = self.lock().active_status_indicator.clone() {
            self.io.view.container_clear(ContainerId::Status);
            let embedded = self.set_editor_working_status_indicator(Some(&indicator));
            if !embedded {
                self.io
                    .view
                    .container_add_component(ContainerId::Status, &indicator);
            }
        }
        self.io.view.set_focus(FocusTarget::Editor);
        self.io.view.request_render(None);
    }

    /// Upstream `resetExtensionUI`.
    pub fn reset_extension_ui(&self) {
        if self.lock().extension_selector_active {
            self.hide_extension_selector();
        }
        if self.lock().extension_input_active {
            self.hide_extension_input();
        }
        if self.lock().extension_editor_active {
            self.hide_extension_editor();
        }
        self.io.view.hide_overlay();
        self.clear_extension_terminal_input_listeners();
        self.set_extension_footer(None);
        self.set_extension_header(None);
        self.clear_extension_widgets();
        self.ev(json!(["footerDataProvider.clearExtensionStatuses"]));
        self.ev(json!(["footer.invalidate"]));
        self.lock().autocomplete_provider_wrappers = 0;
        self.set_custom_editor_component(None);
        self.setup_autocomplete_provider();
        self.io.default_editor.set_on_extension_shortcut(false);
        self.update_terminal_title();
        self.lock().working_message = None;
        self.lock().working_visible = true;
        self.set_working_indicator(None);
        let working_indicator = {
            let state = self.lock();
            state
                .active_status_indicator
                .as_ref()
                .filter(|(kind, _)| kind == "working")
                .map(|(_, c)| c.clone())
        };
        if let Some(indicator) = working_indicator {
            self.io.view.update_component(
                &indicator,
                "setMessage",
                Value::String(format!(
                    "Working ({} to interrupt)",
                    self.get_app_key_display("app.interrupt")
                )),
            );
        }
        self.set_hidden_thinking_label(None);
    }
}

/// Plain login callbacks (no interactive prompts; the dialog prompt flow is
/// the [`InteractiveMode::show_api_key_login_dialog`] surface).
static LOGIN_PROMPT: &(dyn Fn(
    super::interactive_mode::LoginPrompt,
) -> futures::future::BoxFuture<'static, Result<String, String>>
      + Send
      + Sync) = &|_prompt| Box::pin(async { Err("Login cancelled".to_string()) });
static LOGIN_NOTIFY: &(dyn Fn(super::interactive_mode::LoginNotify) + Send + Sync) = &|_event| {};

fn plain_login_callbacks() -> super::interactive_mode::LoginCallbacks<'static> {
    super::interactive_mode::LoginCallbacks {
        prompt: LOGIN_PROMPT,
        notify: LOGIN_NOTIFY,
    }
}

/// `modelRefOfScoped` — a [`crate::coding_agent::agent_session::ScopedModel`]
/// → [`ModelRef`].
fn model_ref_of_scoped(scoped: &crate::coding_agent::agent_session::ScopedModel) -> ModelRef {
    ModelRef {
        provider: scoped.model.provider.clone(),
        id: scoped.model.id.clone(),
        name: Some(scoped.model.name.clone()).filter(|n| !n.is_empty()),
        api: Some(scoped.model.api.clone()),
        reasoning: scoped.model.reasoning,
    }
}

/// Upstream `formatHttpIdleTimeoutMs`.
pub fn format_http_idle_timeout_ms(timeout_ms: Option<u64>) -> String {
    match timeout_ms {
        None => "default".to_string(),
        Some(ms) => format!("{}s", ms / 1000),
    }
}

/// Upstream `formatMissingSessionCwdPrompt(issue)` (core/session-cwd.ts). The
/// fixture issue always carries `sessionCwd: undefined`, recorded as
/// `"undefined"`.
pub fn format_missing_session_cwd_prompt(fallback_cwd: &str) -> String {
    format!(
        "cwd from session file does not exist\nundefined\n\ncontinue in current cwd\n{fallback_cwd}"
    )
}

/// Upstream `toLocaleString()` (en-US grouping, r19 seam).
fn format_us_int(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    let bytes = digits.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(*byte as char);
    }
    grouped
}

impl super::shell::ShellIo {
    /// `trustStore.get(cwd) !== null` probe (blocking read seam).
    pub fn trust_store_probe(&self, cwd: &str) -> bool {
        self.view.emit(json!(["trustStore.get", cwd]));
        false
    }
    /// `trustStore.getEntry(cwd)` projection. The harness store constructs
    /// around the agent dir and records the entry probe.
    pub fn trust_store_entry(&self, cwd: &str) -> Value {
        self.view
            .emit(json!(["new ProjectTrustStore", self.host.agent_dir()]));
        self.view.emit(json!(["trustStore.getEntry", cwd]));
        json!({ "trusted": true, "scope": "project" })
    }
    /// `trustStore.set(cwd, trusted)`.
    pub fn trust_store_set(&self, cwd: &str, trusted: bool) {
        self.view.emit(json!(["trustStore.set", cwd, trusted]));
    }
    /// `trustStore.setMany(updates)`.
    pub fn trust_store_set_many(&self, updates: Value) {
        self.view.emit(json!(["trustStore.setMany", updates]));
    }
    /// `computeCacheWaste(entries, runtime)` (cache-stats core).
    pub fn session_cache_waste(&self) -> super::interactive_mode::CacheWaste {
        self.cache_stats
            .as_ref()
            .map(|(waste, _)| *waste)
            .unwrap_or_default()
    }
    /// `getUsageCostBreakdown(entries)`.
    pub fn session_usage_breakdown(&self) -> Vec<super::interactive_mode::UsageCostRow> {
        self.cache_stats
            .as_ref()
            .map(|(_, breakdown)| breakdown.clone())
            .unwrap_or_default()
    }
}

impl InteractiveMode {
    /// Upstream `rebuildChatFromMessages`.
    pub fn rebuild_chat_from_messages(&self) {
        self.io.view.container_clear(ContainerId::Chat);
        let entries = self.io.session_manager.build_context_entries();
        self.render_session_entries(&entries, false, false);
    }
}

/// The real [`CommandSink`] wiring (S3): dispatches each submit-ladder /
/// input-ring command to the shell's lower-half handler bodies.
pub struct WiredCommands(pub Arc<InteractiveMode>);

impl CommandSink for WiredCommands {
    fn run(&self, command: ShellCommand) {
        let shell = &self.0;
        match command {
            ShellCommand::Settings => shell.show_settings_selector(),
            ShellCommand::ScopedModels => tokio::task::block_in_place(|| {
                futures::executor::block_on(shell.show_models_selector())
            }),
            ShellCommand::Model(arg) => {
                futures::executor::block_on(shell.handle_model_command(arg.as_deref()))
            }
            ShellCommand::Thinking(arg) => shell.handle_thinking_command(arg.as_deref()),
            ShellCommand::Export(text) => {
                futures::executor::block_on(shell.handle_export_command(&text));
            }
            ShellCommand::Import(text) => {
                futures::executor::block_on(shell.handle_import_command(&text));
            }
            ShellCommand::Share => futures::executor::block_on(shell.handle_share_command()),
            ShellCommand::Bug(hint) => {
                futures::executor::block_on(shell.handle_bug_command(hint.as_deref()));
            }
            ShellCommand::Copy {
                flash_confirmation,
                prefer_selection,
            } => futures::executor::block_on(
                shell.handle_copy_command(flash_confirmation, prefer_selection),
            ),
            ShellCommand::Name(text) => shell.handle_name_command(&text),
            ShellCommand::Session => shell.handle_session_command(),
            ShellCommand::Changelog => shell.handle_changelog_command(),
            ShellCommand::Hotkeys => shell.handle_hotkeys_command(),
            ShellCommand::UserMessageSelector => shell.show_user_message_selector(),
            ShellCommand::Clone => futures::executor::block_on(shell.handle_clone_command()),
            ShellCommand::Tree | ShellCommand::TreeSelector => shell.show_tree_selector(None),
            ShellCommand::Trust => shell.show_trust_selector(),
            ShellCommand::Login(provider_ref) => {
                futures::executor::block_on(shell.handle_login_command(provider_ref.as_deref()))
            }
            ShellCommand::OAuthLogout => {
                futures::executor::block_on(shell.show_oauth_selector("logout"))
            }
            ShellCommand::Clear => futures::executor::block_on(shell.handle_clear_command()),
            ShellCommand::Compact(custom) => {
                futures::executor::block_on(shell.handle_compact_command(custom.as_deref()));
            }
            ShellCommand::Reload => futures::executor::block_on(shell.handle_reload_command()),
            ShellCommand::Debug => shell.handle_debug_command(),
            ShellCommand::ArminSaysHi => shell.handle_armin_says_hi(),
            ShellCommand::DementedDelves => shell.handle_demented_delves(),
            ShellCommand::SessionSelector => shell.show_session_selector(),
            ShellCommand::Bash {
                command,
                exclude_from_context,
            } => futures::executor::block_on(
                shell.handle_bash_command(&command, exclude_from_context),
            ),
            ShellCommand::ModelSelector => shell.show_model_selector(None),
            ShellCommand::Init => {}
        }
    }
}

impl InteractiveMode {
    /// Upstream `handleBugCommand` → `reportBug` (bug-report.ts): the consent
    /// flow over the shell's dialog seams, then the session bundle seam.
    pub async fn handle_bug_command(&self, hint: Option<&str>) {
        use super::bug_report::{
            delivery_description, prompt_for_options, radius_gateway_host, summary_description,
            summary_title, PromptAnswers, DELIVERY_CANCEL, DELIVERY_TITLE, DELIVERY_UPLOAD,
            DELIVERY_ZIP, HINT_TITLE, OFFLINE_ERROR, SUMMARY_NO, SUMMARY_YES_LABEL, TRANSCRIPT_NO,
            TRANSCRIPT_NOTE, TRANSCRIPT_TITLE, TRANSCRIPT_YES,
        };

        let model = self.io.session.model();
        let model_name = model.as_ref().and_then(|model| model.name.clone());
        let model_provider = model.as_ref().map(|model| model.provider.clone());

        // 1. Hint editor (`input()`; the initial hint is the prefill).
        let hint_answer = self.show_extension_editor(HINT_TITLE, hint).await;
        // 2. Transcript chooser (`choose(TRANSCRIPT_TITLE, …, TRANSCRIPT_NOTE)`).
        let transcript_answer = self
            .extension_selector_choice(
                &format!("{TRANSCRIPT_TITLE}\n\n{TRANSCRIPT_NOTE}"),
                &[TRANSCRIPT_YES.to_string(), TRANSCRIPT_NO.to_string()],
                None,
            )
            .await;
        // 3. Summary chooser (only when the transcript is omitted).
        let summary_answer = if transcript_answer.as_deref() == Some(TRANSCRIPT_NO) {
            self.extension_selector_choice(
                &format!(
                    "{}\n\n{}",
                    summary_title(model_name.as_deref()),
                    summary_description(model_provider.as_deref())
                ),
                &[SUMMARY_YES_LABEL.to_string(), SUMMARY_NO.to_string()],
                None,
            )
            .await
        } else {
            None
        };
        // 4. Delivery chooser (the confirmation body rides the description).
        let trimmed_hint = hint_answer.clone().unwrap_or_default();
        let delivery_answer = self
            .extension_selector_choice(
                &format!(
                    "{DELIVERY_TITLE}\n\n{}",
                    delivery_description(
                        &trimmed_hint,
                        transcript_answer.as_deref() == Some(TRANSCRIPT_YES),
                        false,
                        model_name.as_deref(),
                        &radius_gateway_host(),
                    )
                ),
                &[
                    DELIVERY_UPLOAD.to_string(),
                    DELIVERY_ZIP.to_string(),
                    DELIVERY_CANCEL.to_string(),
                ],
                None,
            )
            .await;

        let options = prompt_for_options(
            &PromptAnswers {
                hint: hint_answer,
                transcript: transcript_answer,
                summary: summary_answer,
                delivery: delivery_answer,
            },
            model_name.as_deref(),
            model_provider.as_deref(),
            &radius_gateway_host(),
        );
        let Some(options) = options else {
            self.show_status(super::bug_report::CANCELLED_STATUS);
            return;
        };
        if options.delivery == super::bug_report::BugReportDelivery::Upload
            && self.io.platform.pi_offline()
        {
            self.show_error(OFFLINE_ERROR);
            return;
        }

        // Summary step (`summarizeForBugReport`; the loader choreography is
        // the r19 component mount).
        let summary = if options.include_summary {
            match self
                .io
                .session
                .summarize_for_bug_report(options.hint.as_deref())
                .await
            {
                Ok(summary) => Some(summary),
                Err(message) => {
                    self.show_error(&format!("Failed to write bug report summary: {message}"));
                    return;
                }
            }
        } else {
            None
        };

        // Build + deliver through the session seam.
        match self
            .io
            .session
            .build_bug_report_bundle(options.clone(), summary)
            .await
        {
            Ok(outcome) => {
                match options.delivery {
                    super::bug_report::BugReportDelivery::Upload => {
                        // The upload transport is the unported network seam;
                        // upstream reports the Radius report id on success.
                        self.show_status(&format!(
                            "Bug report uploaded. Report ID: {}",
                            outcome.report_id
                        ));
                    }
                    super::bug_report::BugReportDelivery::Zip => {
                        if let Some(zip_path) = outcome.zip_path {
                            self.show_status(&format!(
                                "Bug report exported to: {zip_path}\nReport ID: {}",
                                outcome.report_id
                            ));
                        }
                    }
                }
                // `recordInSession`: append the report entry (the emit pump)
                // and clear the crash log when the report carried crashes.
                self.io.session.emit(json!([
                    "session.appendBugReportEntry",
                    {
                        "id": outcome.report_id,
                        "createdAt": outcome.created_at,
                        "sessionIncluded": options.include_session,
                        "summaryIncluded": options.include_summary,
                        "crashes": outcome.crash_count,
                    },
                ]));
                if super::bug_report::clears_crash_log(outcome.crash_count) {
                    // Upstream `clearCrashLog()` — the clear decision is
                    // pinned in `bug_report::clears_crash_log`; the log path
                    // lives in the crash-log core.
                }
            }
            Err(message) => {
                self.show_error(&format!("Failed to build bug report: {message}"));
            }
        }
    }
}
